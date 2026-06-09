//! Full-page 3D viewer route.
//!
//! Route: `/viewer/:name/:file_encoded/:cfg`
//!
//!   :file_encoded  — base64-url-safe-encoded output-file path
//!                    (e.g. Y29sbWFwL2RlbnNlL3NwYXJzZS9wb2ludHMzRC5iaW4= for
//!                     colmap/dense/sparse/points3D.bin)
//!   :cfg           — optional base64-encoded JSON blob combining camera + config.
//!                    Omit to use defaults.  The blob shape:
//!                    {"cam":{"position":[...],"target":[...],"up":[...]},"config":{...}}

use base64::Engine;
use dioxus::prelude::*;
use tracing::{debug, error, info};

use crate::server::get_project_output_bytes;
use crate::viewer_conversion;

/// Parse the combined cfg blob (base64 JSON with cam + config).
fn parse_cfg_blob(raw: &str) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
    if raw.is_empty() || raw == "-" {
        return (None, None);
    }
    // Try standard base64 first (legacy), then URL-safe (newer scripts).
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(raw));
    if let Ok(bytes) = bytes {
        if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            let cam = val.get("cam").cloned();
            let config = val.get("config").cloned();
            return (cam, config);
        }
    }
    (None, None)
}

// ── Viewer page component ───────────────────────────────────────────────────

#[component]
pub fn Viewer(name: String, file_encoded: String, cfg: String) -> Element {
    info!(project_name = %name, "Viewer page mounted");

    // Decode the output-file path from the route segment.
    //
    // Two formats are supported:
    //   1. Pipe-separated:  `colmap|sparse|0|points3D.bin`  (avoids / in URLs)
    //   2. Base64 URL-safe: `Y29sbWFwL3NwYXJzZS8wL3BvaW50czNELmJpbg==`
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
    let (initial_cam, initial_cfg) = parse_cfg_blob(&cfg);

    let mut loading: Signal<bool> = use_signal(|| !file_decoded.is_empty());
    let mut error_msg: Signal<Option<String>> = use_signal(|| None);

    let container_id = format!("v3d-embedded-{}", name);
    let container_id_for_effect = container_id.clone();
    let name_for_effect = name.clone();
    let file_for_effect = file_decoded.clone();

    // ── Fetch model & mount viewer ──────────────────────────────────
    use_effect(move || {
        let fp = file_for_effect.clone();
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
        let ic_val = initial_cam.clone();
        let icfg_val = initial_cfg.clone();

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

                    let js = format!(
                        r#"
import('{viewer_url}').then(async function(mod) {{
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
}});
"#
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
    let file_display = file_decoded
        .rsplit('/')
        .next()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();

    let no_file = file_decoded.is_empty();

    let title = if file_display.is_empty() {
        format!("3D Viewer — {}", name)
    } else {
        format!("{} — {}", file_display, name)
    };

    let name_for_back = name.clone();
    let name_for_nav = name.clone();
    let file_for_overlay = file_decoded.clone();
    let file_display_for_overlay = file_display.clone();

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
                    "ⓘ"
                }

                // Back to outputs — placed right-most, consistent with the
                // rest of the app's navigation pattern
                button {
                    class: "viewer-back-btn",
                    onclick: move |_| {
                        let _ = navigator().push(crate::Route::ProjectOutputs { name: name_for_back.clone() });
                    },
                    span { class: "viewer-back-arrow", "←" }
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
                        "✕"
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
