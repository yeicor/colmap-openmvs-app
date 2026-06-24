//! Full-page 3D viewer route.
//!
//! Route: `/viewer/:name/:file_encoded/:cfg`
//!
//!   :file_encoded  — base64-url-safe-encoded output-file path
//!                    (e.g. Y29sbGFtYXAvZGVuc2Uvc3BhcnNlL3BvaW50czNELmJpbg== for
//!                     colmap/dense/sparse/points3D.bin)
//!   :cfg           — optional base64-encoded JSON blob combining camera + config.
//!                    Omit to use defaults.  The blob shape:
//!                    {"cam":{"position":[...],"target":[...],"up":[...]},"config":{...}}

use base64::Engine;
use dioxus::core::use_drop;
use dioxus::prelude::*;
use tracing::{debug, error, info};

#[cfg(feature = "demo")]
#[cfg(target_arch = "wasm32")]
use js_sys::Uint8Array;
#[cfg(feature = "demo")]
#[cfg(target_arch = "wasm32")]
use web_sys::{Blob, BlobPropertyBag, Url};

use crate::server::get_project_output_glb;

/// Decode the cfg URL parameter: base64 → raw JSON string.
///
/// Returns `None` when the parameter is empty/`-` (no persisted state).
/// Unlike the previous implementation, this function does NOT parse and
/// re-serialise through `serde_json::Value`, which could produce subtly
/// different number representations depending on the serde_json version
/// or feature flags (`preserve_order`, `arbitrary_precision`, etc.)
/// available in a given build environment.
///
/// The raw JSON string is passed directly to the JS side, which performs
/// `JSON.parse` itself — guaranteeing that the numeric values stored in
/// the URL are never altered during a Rust round-trip.
fn parse_cfg_blob_raw(raw: &str) -> Option<String> {
    if raw.is_empty() || raw == "-" {
        return None;
    }
    // Try standard base64 first (legacy), then URL-safe (newer scripts).
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(raw))
        .ok()?;
    String::from_utf8(bytes).ok()
}

// ── Viewer page component ───────────────────────────────────────────────────

#[component]
pub fn Viewer(name: String, file_encoded: String, cfg: String) -> Element {
    info!(project_name = %name, "Viewer page mounted");

    // Decode the output-file path from the route segment.
    //
    // Two formats are supported:
    //   1. Pipe-separated:  `colmap|sparse|0|points3D.bin`  (avoids / in URLs)
    //   2. Base64 URL-safe: `Y29sbGFtYXAvc3BhcnNlLzAvcG9pbnRzM0QuYmlu`
    //
    // Format 1 is preferred for screenshot URLs because `/` in route parameters
    // breaks routing — the browser / Dioxus may URL-decode %2F back to `/`,
    // creating extra path segments.
    let file_decoded = if file_encoded.contains('|') {
        // Pipe-separated: translate | → /
        file_encoded.replace('|', "/")
    } else {
        // Legacy: base64-URL-safe-encoded path
        String::from_utf8(
            base64::engine::general_purpose::URL_SAFE
                .decode(file_encoded.as_bytes())
                .unwrap_or_default(),
        )
        .unwrap_or_default()
    };
    let initial_cfg_raw = parse_cfg_blob_raw(&cfg);

    let mut loading: Signal<bool> = use_signal(|| !file_decoded.is_empty());
    let mut error_msg: Signal<Option<String>> = use_signal(|| None);
    let cancelled: Signal<bool> = use_signal(|| false);

    let container_id = format!("v3d-embedded-{}", name);
    let container_id_for_effect = container_id.clone();
    let name_for_effect = name.clone();
    let file_for_effect = file_decoded.clone();

    // ── Guard against duplicate spawns (component may re-render while
    //     the async fetch is in flight, e.g. when toggling the info
    //     overlay, which would cause use_effect to re-run and spawn
    //     another task, ultimately mounting duplicate viewers).
    let mut spawn_guard: Signal<bool> = use_signal(|| false);

    // ── Fetch model & mount viewer ──────────────────────────────────
    use_effect(move || {
        let fp = file_for_effect.clone();
        if fp.is_empty() {
            return;
        }
        if !loading() {
            return;
        }
        // Prevent duplicate spawns if use_effect re-runs while a
        // previous fetch is still in flight.
        if spawn_guard() {
            return;
        }
        spawn_guard.set(true);

        let pn = name_for_effect.clone();
        let fp_clone = fp.clone();
        let mut loading = loading.clone();
        #[cfg_attr(not(feature = "demo"), allow(unused_mut, unused_variables))]
        let mut err = error_msg.clone();
        let mut guard = spawn_guard.clone();
        let container_id = container_id_for_effect.clone();
        let icfg_raw = initial_cfg_raw.clone();
        let cancelled = cancelled.clone();

        spawn(async move {
            if cancelled() {
                guard.set(false);
                return;
            }

            debug!(file = %fp_clone, "Mounting viewer");

            let project_name_esc = js_escape(&pn);
            let file_path_esc = js_escape(&fp_clone);

            // Pass the raw JSON blob to JS so the numeric values in the
            // URL are never reinterpreted by Rust's serde_json (which could
            // change floating-point representation across serde_json versions
            // or feature-flag configurations).
            // JS does `JSON.parse(_cfgRaw)` itself.
            let cfg_raw_json_esc = js_escape(icfg_raw.as_deref().unwrap_or(""));

            let viewer_url = asset!(
                "/assets/viewer3d.bundle.js",
                AssetOptions::js().with_minify(false)
            )
            .to_string();

            // Non-demo builds: pass a server URL directly to JS so the
            // GLTFLoader fetches via HTTP.  This avoids moving potentially
            // hundreds of MB of GLB data through the Rust ↔ JS bridge
            // (base64 + eval on desktop, WASM memory copies on web).
            //
            // We use `get_server_url()` (not `window.location.origin`) as the
            // base URL so that this works correctly in desktop mode where the
            // embedded server runs on a different origin (e.g. localhost:port)
            // while `window.location.origin` is `dioxus://index.html`.
            //
            // When the fullstack feature is not available (e.g. desktop-only
            // without server), we use an empty string and fall back to
            // `window.location.origin` in JS.
            //
            // If the JS-side HTTP fetch fails (e.g. on Android where no TCP
            // server is running), we fall back to fetching through the Rust
            // server function and passing the data as base64.
            #[cfg(not(feature = "demo"))]
            let js = {
                let server_url = {
                    #[cfg(feature = "fullstack")]
                    {
                        dioxus::fullstack::get_server_url()
                    }
                    #[cfg(not(feature = "fullstack"))]
                    {
                        String::new()
                    }
                };
                let server_url_esc = js_escape(&server_url);
                format!(
                    r#"
	var _viewerAbsUrl = new URL('{viewer_url}', window.location.href).href;
	import(_viewerAbsUrl).then(async function(mod) {{
	try {{
	    var container = document.getElementById('{container_id}');
	    if (!container) {{ console.error('Viewer container not found'); return; }}
	    var baseUrl = '{server_url_esc}' || (/^https?:/.test(window.location.origin) ? window.location.origin : 'http://localhost:8080');
	    var glbUrl = baseUrl + '/api/projects/' + encodeURIComponent('{project_name_esc}') + '/outputs/glb?relative_path=' + encodeURIComponent('{file_path_esc}');
	    var _cfgRaw = '{cfg_raw_json_esc}' || null;
	    var viewer = await mod.mountViewer3d(container, {{
	        projectName: '{project_name_esc}',
	        filePath: '{file_path_esc}',
	        glbUrl: glbUrl,
	        initialCamera: _cfgRaw ? JSON.parse(_cfgRaw).cam : null,
	        initialConfig: _cfgRaw ? JSON.parse(_cfgRaw).config : null,
	    }});
	    window.__viewer3d_instance = viewer;
	    var el = document.getElementById('{container_id}');
	    if (el) el.dataset.ready = 'true';
	    dioxus.send('loaded');
	}} catch(err) {{
	    dioxus.send('error:' + (err.message || 'unknown'));
	}}
	}});
"#
                )
            };

            // Demo mode: fetch bytes through the RPC and create a Blob
            // URL so the viewer can load them (no HTTP endpoint exists).
            #[cfg(feature = "demo")]
            let js = {
                let result = fetch_glb_demo(&pn, &fp_clone).await;
                match result {
                    Ok(glb_bytes) => {
                        #[cfg(target_arch = "wasm32")]
                        {
                            let uint8 = Uint8Array::new_with_length(glb_bytes.len() as u32);
                            uint8.copy_from(&glb_bytes);
                            let parts = js_sys::Array::new();
                            parts.push(&uint8);
                            let opts = BlobPropertyBag::new();
                            opts.set_type("model/gltf-binary");
                            let blob =
                                Blob::new_with_buffer_source_sequence_and_options(&parts, &opts)
                                    .expect("Failed to create GLB blob");
                            let glb_url = Url::create_object_url_with_blob(&blob)
                                .expect("Failed to create object URL");
                            let glb_url_esc = js_escape(&glb_url);
                            format!(
                                r#"
window.__viewer3d_glb_url = '{glb_url_esc}';
var _viewerAbsUrl = new URL('{viewer_url}', window.location.href).href;
import(_viewerAbsUrl).then(async function(mod) {{
try {{
    var container = document.getElementById('{container_id}');
    if (!container) {{ console.error('Viewer container not found'); return; }}
	    var _cfgRaw = '{cfg_raw_json_esc}' || null;
	    var viewer = await mod.mountViewer3d(container, {{
	        projectName: '{project_name_esc}',
	        filePath: '{file_path_esc}',
	        glbUrl: '{glb_url_esc}',
        initialCamera: _cfgRaw ? JSON.parse(_cfgRaw).cam : null,
        initialConfig: _cfgRaw ? JSON.parse(_cfgRaw).config : null,
    }});
    window.__viewer3d_instance = viewer;
    var el = document.getElementById('{container_id}');
    if (el) el.dataset.ready = 'true';
    dioxus.send('loaded');
}} catch(err) {{
    console.error('[Viewer] mount error:', err.stack || err);
    dioxus.send('error:' + (err.message || 'unknown'));
}}
}});
"#
                            )
                        }
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&glb_bytes);
                            let b64_esc = js_escape(&b64);
                            format!(
                                r#"
var _viewerAbsUrl = new URL('{viewer_url}', window.location.href).href;
import(_viewerAbsUrl).then(async function(mod) {{
try {{
    var container = document.getElementById('{container_id}');
    if (!container) {{ console.error('Viewer container not found'); return; }}
	    var _cfgRaw = '{cfg_raw_json_esc}' || null;
	    var viewer = await mod.mountViewer3d(container, {{
	        projectName: '{project_name_esc}',
	        filePath: '{file_path_esc}',
	        glbBase64: '{b64_esc}',
	        initialCamera: _cfgRaw ? JSON.parse(_cfgRaw).cam : null,
	        initialConfig: _cfgRaw ? JSON.parse(_cfgRaw).config : null,
	    }});
	    window.__viewer3d_instance = viewer;
	    var el = document.getElementById('{container_id}');
	    if (el) el.dataset.ready = 'true';
	    dioxus.send('loaded');
	}} catch(err) {{
	    console.error('[Viewer] mount error:', err.stack || err);
	    dioxus.send('error:' + (err.message || 'unknown'));
	}}
	}});
	"#
                            )
                        }
                    }
                    Err(e) => {
                        error!(file = %fp_clone, error = %e, "Failed to load output for viewer");
                        loading.set(false);
                        err.set(Some(format!("Failed to load model: {e}")));
                        guard.set(false);
                        return;
                    }
                }
            };

            if cancelled() {
                loading.set(false);
                guard.set(false);
                return;
            }

            let mut eval_handle = dioxus::document::eval(&js);
            let res = eval_handle.recv::<String>().await;

            if let Ok(msg) = res {
                if msg == "loaded" {
                    // Success — viewer mounted.
                } else if let Some(err_msg) = msg.strip_prefix("error:") {
                    tracing::warn!(
                        "JS viewer fetch via glbUrl failed ({}), falling back to Rust server function with base64 — performance may be reduced",
                        err_msg
                    );
                    // Fallback: fetch GLB bytes via Rust server function, then
                    // mount the viewer via base64.
                    #[cfg(not(feature = "demo"))]
                    {
                        let result = fetch_glb_demo(&pn, &fp_clone).await;
                        match result {
                            Ok(glb_bytes) => {
                                let b64 =
                                    base64::engine::general_purpose::STANDARD.encode(&glb_bytes);
                                let b64_esc = js_escape(&b64);
                                let js_fb = format!(
                                    r#"
	var _viewerAbsUrl = new URL('{viewer_url}', window.location.href).href;
	import(_viewerAbsUrl).then(async function(mod) {{
	try {{
	    var container = document.getElementById('{container_id}');
	    if (!container) {{ console.error('Viewer container not found'); return; }}
	    var _cfgRaw = '{cfg_raw_json_esc}' || null;
	    var viewer = await mod.mountViewer3d(container, {{
	        projectName: '{project_name_esc}',
	        filePath: '{file_path_esc}',
	        glbBase64: '{b64_esc}',
	        initialCamera: _cfgRaw ? JSON.parse(_cfgRaw).cam : null,
	        initialConfig: _cfgRaw ? JSON.parse(_cfgRaw).config : null,
	    }});
	    window.__viewer3d_instance = viewer;
	    var el = document.getElementById('{container_id}');
	    if (el) el.dataset.ready = 'true';
	    dioxus.send('loaded2');
	}} catch(err2) {{
	    dioxus.send('error2:' + (err2.message || 'unknown'));
	}}
	}});
"#
                                );
                                let mut handle_fb = dioxus::document::eval(&js_fb);
                                let res_fb = handle_fb.recv::<String>().await;
                                if let Ok(msg_fb) = res_fb {
                                    if let Some(e2) = msg_fb.strip_prefix("error2:") {
                                        error!(file = %fp_clone, error = %e2, "Fallback viewer mount error");
                                        err.set(Some(format!(
                                            "Failed to load model via fallback: {e2}"
                                        )));
                                    }
                                }
                            }
                            Err(e) => {
                                error!(file = %fp_clone, error = %e, "Failed to load output for viewer fallback");
                                err.set(Some(format!("Failed to load model (fallback): {e}")));
                            }
                        }
                    }
                }
            }
            loading.set(false);
            guard.set(false);
        });
    });

    // ── Cleanup on unmount ──────────────────────────────────────────
    use_drop({
        let container_id = container_id.clone();
        move || {
            let _ = dioxus::document::eval(&format!(
                r#"var inst = window.__viewer3d_instance;
if (inst) {{ try {{ inst.dispose(); }} catch(e) {{}} window.__viewer3d_instance = null; }}
// Revoke the Blob URL created for WASM builds
var glbUrl = window.__viewer3d_glb_url;
if (glbUrl) {{ try {{ URL.revokeObjectURL(glbUrl); }} catch(e) {{}} window.__viewer3d_glb_url = null; }}
var el = document.getElementById('{container_id}');
if (el) el.innerHTML = '';"#
            ));
        }
    });

    // ── Derived display values ──────────────────────────────────────
    let file_display = file_decoded
        .rsplit('/')
        .next()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();

    let no_file = file_decoded.is_empty();

    let title = if file_display.is_empty() {
        format!("3D Viewer \u{2014} {}", name)
    } else {
        format!("{} \u{2014} {}", file_display, name)
    };

    let name_for_back = name.clone();
    let name_for_nav = name.clone();
    let file_for_overlay = file_decoded.clone();
    let file_display_for_overlay = file_display.clone();
    let container_id_for_back = container_id.clone();
    let mut cancelled_for_back = cancelled.clone();

    // Info overlay toggle
    let mut show_info = use_signal(|| false);

    rsx! {
        document::Title { "{title}" }

        div {
            id: "viewer-page",
            class: "viewer-page",

            div {
                class: "viewer-topbar",

                div {
                    id: "viewer-toolbar",
                    class: "viewer-toolbar",
                }

                // Info toggle — shows model details in an overlay
                button {
                    class: "viewer-info-btn",
                    title: "Model info",
                    onclick: move |_| show_info.set(!show_info()),
                    "\u{2139}\u{FE0F}"
                }

                // Back to outputs — placed right-most, consistent with the
                // rest of the app's navigation pattern
                button {
                    class: "viewer-back-btn",
                    onclick: move |_| {
                        cancelled_for_back.set(true);
                        let _ = dioxus::document::eval(&format!(
                            r#"var inst = window.__viewer3d_instance;
if (inst) {{ try {{ inst.dispose(); }} catch(e) {{}} window.__viewer3d_instance = null; }}
// Revoke the Blob URL created for WASM builds
var glbUrl = window.__viewer3d_glb_url;
if (glbUrl) {{ try {{ URL.revokeObjectURL(glbUrl); }} catch(e) {{}} window.__viewer3d_glb_url = null; }}
var el = document.getElementById('{container_id_for_back}');
if (el) el.innerHTML = '';"#
                        ));
                        let _ = navigator().push(crate::Route::ProjectOutputs { name: name_for_back.clone() });
                    },
                    span { class: "viewer-back-arrow", "\u{1F519}" }
                    span { class: "viewer-back-label", "Back" }
                }
            }

            // ── Info overlay (file details) ───────────────────────────
            if show_info() {
                div {
                    class: "viewer-info-overlay",
                    div {
                        class: "viewer-info-close",
                        onclick: move |_| show_info.set(false),
                        "\u{2715}"
                    }
                    div { class: "viewer-info-row",
                        span { class: "viewer-info-label", "Project" }
                        span { class: "viewer-info-value", "{name}" }
                    }
                    div { class: "viewer-info-row",
                        span { class: "viewer-info-label", "File" }
                        span { class: "viewer-info-value", "{file_display_for_overlay}" }
                    }
                    div { class: "viewer-info-row",
                        span { class: "viewer-info-label", "Path" }
                        span { class: "viewer-info-value viewer-info-path", "{file_for_overlay}" }
                    }
                }
            }

            if loading() {
                div {
                    class: "viewer-loading",
                    div { class: "viewer-spinner" }
                    span { "Loading model\u{2026}" }
                }
            }

            if let Some(ref msg) = error_msg() {
                div {
                    class: "viewer-error",
                    h3 { "Failed to load model" }
                    p { "{msg}" }
                    button {
                        class: "viewer-retry-btn",
                        onclick: move |_| {
                            let fp = &file_decoded;
                            if !fp.is_empty() {
                                error_msg.set(None);
                                loading.set(true);
                            }
                        },
                        "Retry"
                    }
                }
            }

            if no_file {
                div {
                    class: "viewer-empty",
                    h3 { "No file selected" }
                    p {
                        "Select a model from "
                        a {
                            href: "#",
                            onclick: move |_| {
                                let _ = navigator().push(crate::Route::ProjectOutputs { name: name_for_nav.clone() });
                            },
                            "project outputs"
                        }
                        "."
                    }
                }
            }

            div {
                id: "{container_id}",
                class: "viewer-canvas-container",
            }
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Fetch GLB bytes through the Rust server function.
/// Used as the primary path in demo mode and as a fallback when the JS HTTP
/// fetch fails (e.g. on Android with no TCP server).
async fn fetch_glb_demo(project_name: &str, file_path: &str) -> Result<Vec<u8>, String> {
    let stream = get_project_output_glb(project_name.to_string(), file_path.to_string())
        .await
        .map_err(|e| format!("Failed to fetch GLB: {e}"))?;

    let mut glb_bytes = Vec::new();
    let mut s = stream;
    while let Some(chunk) = s.next().await {
        match chunk {
            Ok(data) => glb_bytes.extend_from_slice(&data),
            Err(e) => return Err(format!("Stream error: {e}")),
        }
    }
    Ok(glb_bytes)
}

fn js_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
