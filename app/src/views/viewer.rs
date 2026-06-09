//! Full-page 3D viewer route.
//!
//! Parameters (`file`, `cam`, `cfg`) come from two sources:
//!
//! 1. **Navigation from outputs tab** – written into a Dioxus context signal
//!    (`ViewerPendingParams`) before `navigator().push()`, so the viewer can
//!    read them synchronously without any JS↔Rust bridge.
//!
//! 2. **Direct URL access** (bookmark / link sharing) – on WASM the fragment
//!    is parsed directly via `web_sys::window().location().hash()`.  On
//!    non-WASM (desktop without web_sys) the empty-state hint is shown
//!    because there is no reliable way to read the fragment without eval.
//!
//! Two URL layouts are supported in the fragment:
//!
//!   Path-based routing (fullstack / desktop):
//!     `/viewer/MyProject#file=...&cam=...&cfg=...`
//!
//!   Hash-based routing (static web without fullstack):
//!     `/#/viewer/MyProject?file=...&cam=...&cfg=...`

use base64::Engine;
use dioxus::prelude::*;
use tracing::{debug, error, info};

use crate::server::get_project_output_bytes;
use crate::viewer_conversion;
use crate::ViewerPendingParams;

// ── URL-fragment parser (WASM only – synchronous) ──────────────────────────

/// Extract a URL fragment param by reading `window.location.hash` via
/// `js_sys::eval` (synchronous, works in all WASM contexts).
///
/// Handles both fragment layouts:
///   `/viewer/MyProject#file=...`  → hash = `#file=...`
///   `/#/viewer/MyProject?file=...` → hash = `#/viewer/...?file=...`
#[cfg(target_arch = "wasm32")]
fn read_hash_param(key: &str) -> Option<String> {
    let js = "window.location.hash || ''";
    let hash = js_sys::eval(js).ok()?.as_string()?;
    let hash = hash.trim_start_matches('#').to_string();
    let search = if let Some(q) = hash.find('?') {
        hash[q + 1..].to_string()
    } else {
        hash
    };
    for part in search.split('&') {
        let mut kv = part.splitn(2, '=');
        let k = kv.next()?;
        let v = kv.next()?;
        if k == key && !v.is_empty() {
            let decoded = js_sys::decode_uri_component(v).ok()?;
            return decoded.as_string();
        }
    }
    None
}

/// Decode a base64-encoded JSON value (platform-independent).
fn decode_b64_any(val: &str) -> Option<serde_json::Value> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(val).ok()?;
    serde_json::from_slice(&bytes).ok()
}

// ── Viewer page component ───────────────────────────────────────────────────

#[component]
pub fn Viewer(name: String) -> Element {
    info!(project_name = %name, "Viewer page mounted");

    // ── Signals (initially empty, populated below) ──────────────────
    let mut file_path: Signal<String> = use_signal(|| String::new());
    let mut initial_camera: Signal<Option<serde_json::Value>> = use_signal(|| None);
    let mut initial_config: Signal<Option<serde_json::Value>> = use_signal(|| None);
    let mut params_loaded: Signal<bool> = use_signal(|| false);
    let mut loading: Signal<bool> = use_signal(|| false);
    let mut error_msg: Signal<Option<String>> = use_signal(|| None);

    let container_id = format!("v3d-embedded-{}", name);
    let container_id_for_effect = container_id.clone();
    let name_for_effect = name.clone();

    // ── Phase 1: read params from context or URL ───────────────────
    //
    // Path A (all targets): context signal set by outputs.rs before
    //   navigation — synchronous, no DOM needed.
    //
    // Path B (all targets): read `window.location.hash` once the
    //   component is mounted, via a use_effect.  On WASM we use
    //   web_sys; on non-WASM (desktop) we read once through the
    //   unreliable eval bridge.  If that fails, show the empty hint.
    {
        // Read context signal first (always available after component creation).
        let mut pending = use_context::<Signal<Option<ViewerPendingParams>>>();
        let context_file = pending.read().clone();
        if let Some(p) = &context_file {
            file_path.set(p.file.clone());
            pending.set(None);
            loading.set(true);
            params_loaded.set(true);

            // Write the file param into the URL fragment immediately so
            // the address bar is correct for sharing/bookmarking, even
            // before _persistUrlState is called by the JS viewer.
            #[cfg(target_arch = "wasm32")]
            {
                let js = format!(
                    r#"var qs = 'file=' + encodeURIComponent('{file}');
                    if (window.location.hash.startsWith('#/')) {{
                        history.replaceState(null, '', '#/viewer/' + encodeURIComponent('{pn}') + '?' + qs);
                    }} else {{
                        history.replaceState(null, '', '/viewer/' + encodeURIComponent('{pn}') + '#' + qs);
                    }}"#,
                    file = js_escape(&p.file),
                    pn = js_escape(&name),
                );
                let _ = js_sys::eval(&js);
            }
        }
    }

    // Path B: deferred hash reading (only if context was empty).
    let mut fp2 = file_path;
    let mut ic2 = initial_camera;
    let mut icfg2 = initial_config;
    let mut pl2 = params_loaded;
    let mut ld2 = loading;
    use_effect(move || {
        // Only run once, and only if the context signal was empty.
        if pl2() || !fp2().is_empty() {
            return;
        }

        // Try to read the hash via the best available mechanism.
        #[cfg(target_arch = "wasm32")]
        if let Some(f) = read_hash_param("file") {
            fp2.set(f);
            ld2.set(true);
            if let Some(cam) = read_hash_param("cam").and_then(|s| decode_b64_any(&s)) {
                ic2.set(Some(cam));
            }
            if let Some(cfg) = read_hash_param("cfg").and_then(|s| decode_b64_any(&s)) {
                icfg2.set(Some(cfg));
            }
        }

        pl2.set(true);
    });

    // ── Fetch model & mount viewer ──────────────────────────────────
    use_effect(move || {
        let fp = file_path();
        if fp.is_empty() {
            return;
        }
        if !loading() {
            return;
        }

        let pn = name_for_effect.clone();
        let fp_clone = fp.clone();
        let mut loading = loading.clone();
        let mut err = error_msg.clone();
        let container_id = container_id_for_effect.clone();
        let ic_val = initial_camera();
        let icfg_val = initial_config();

        spawn(async move {
            debug!(file = %fp_clone, "Fetching output file for viewer");

            let result = fetch_and_convert(&pn, &fp_clone).await;

            match result {
                Ok(glb_bytes) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&glb_bytes);
                    let project_name_esc = js_escape(&pn);
                    let file_path_esc = js_escape(&fp_clone);
                    let b64_esc = js_escape(&b64);

                    let cam_json = match ic_val {
                        Some(ref v) => {
                            serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())
                        }
                        None => "null".to_string(),
                    };
                    let cfg_json = match icfg_val {
                        Some(ref v) => {
                            serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())
                        }
                        None => "null".to_string(),
                    };

                    let viewer_url = asset!(
                        "/assets/viewer3d.bundle.js",
                        AssetOptions::js().with_minify(false)
                    )
                    .to_string();

                    // Phase 2: create a <script> element directly (avoids the
                    // unreliable dioxus::document::eval bridge entirely).
                    let js = format!(
                        r#"import('{viewer_url}').then(async function(mod) {{
                            try {{
                                var container = document.getElementById('{container_id}');
                                if (!container) {{ console.error('Viewer container not found'); return; }}
                                var viewer = await mod.mountViewer3d(container, {{
                                    projectName: '{project_name_esc}',
                                    filePath: '{file_path_esc}',
                                    glbBase64: '{b64_esc}',
                                    initialCamera: {cam_json},
                                    initialConfig: {cfg_json},
                                }});
                                window.__viewer3d_instance = viewer;
                                var el = document.getElementById('{container_id}');
                                if (el) el.dataset.ready = 'true';
                            }} catch(err) {{
                                console.error('[Viewer] mount error:', err.stack || err);
                            }}
                        }});"#
                    );

                    #[cfg(target_arch = "wasm32")]
                    if let Some(window) = web_sys::window() {
                        if let Some(doc) = window.document() {
                            if let Ok(script) = doc.create_element("script") {
                                script.set_text_content(Some(&js));
                                if let Some(body) = doc.body() {
                                    let _ = body.append_child(&script);
                                    loading.set(false);
                                }
                            }
                        }
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        if let Err(e) = dioxus::document::eval(&js).await {
                            error!(error = ?e, "Failed to mount viewer JS");
                            err.set(Some(format!("Failed to launch viewer: {e}")));
                        } else {
                            loading.set(false);
                        }
                    }
                }
                Err(e) => {
                    error!(file = %fp_clone, error = %e, "Failed to load output for viewer");
                    loading.set(false);
                    err.set(Some(format!("Failed to load model: {e}")));
                }
            }
        });
    });

    // ── Derived display values ──────────────────────────────────────
    let file_display = file_path()
        .rsplit('/')
        .next()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();

    let no_file = file_path().is_empty() && params_loaded();

    let title = if file_display.is_empty() {
        format!("3D Viewer — {}", name)
    } else {
        format!("{} — {}", file_display, name)
    };

    let name_for_back = name.clone();
    let name_for_nav = name.clone();

    rsx! {
        document::Title { "{title}" }

        div {
            id: "viewer-page",
            class: "viewer-page",

            div {
                class: "viewer-topbar",

                button {
                    class: "viewer-back-btn",
                    onclick: move |_| {
                        let _ = navigator().push(crate::Route::ProjectOutputs { name: name_for_back.clone() });
                    },
                    "← Back to Outputs"
                }

                span {
                    class: "viewer-file-label",
                    title: "{file_path()}",
                    "{file_display}"
                }
                div {
                    id: "viewer-toolbar",
                    class: "viewer-toolbar",
                }
            }

            if loading() {
                div {
                    class: "viewer-loading",
                    div { class: "viewer-spinner" }
                    span { "Loading model…" }
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
                            let fp = file_path();
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

async fn fetch_and_convert(project_name: &str, file_path: &str) -> Result<Vec<u8>, String> {
    let stream = get_project_output_bytes(project_name.to_string(), file_path.to_string())
        .await
        .map_err(|e| format!("Failed to fetch output: {e}"))?;

    let mut raw_bytes = Vec::new();
    let mut s = stream;
    while let Some(chunk) = s.next().await {
        match chunk {
            Ok(data) => raw_bytes.extend_from_slice(&data),
            Err(e) => return Err(format!("Stream error: {e}")),
        }
    }

    let companion_png = if let Some(tex_name) = viewer_conversion::ply_texture_file_name(&raw_bytes)
    {
        let tex_rp = std::path::Path::new(file_path)
            .parent()
            .map(|d| d.join(&tex_name).to_string_lossy().to_string())
            .unwrap_or(tex_name);
        debug!(texture = %tex_rp, "Fetching companion texture");
        match get_project_output_bytes(project_name.to_string(), tex_rp).await {
            Ok(tex_stream) => {
                let mut tex_bytes = Vec::new();
                let mut ts = tex_stream;
                while let Some(chunk) = ts.next().await {
                    match chunk {
                        Ok(data) => tex_bytes.extend_from_slice(&data),
                        Err(_) => break,
                    }
                }
                if tex_bytes.is_empty() {
                    None
                } else {
                    Some(tex_bytes)
                }
            }
            Err(_) => None,
        }
    } else {
        None
    };

    let file_name = std::path::Path::new(file_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_string());

    let glb = viewer_conversion::convert_output_for_viewer(&file_name, &raw_bytes, companion_png)
        .map_err(|e| format!("Conversion failed: {e}"))?;

    Ok(glb)
}

fn js_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
