use crate::server::{get_project_output, get_project_output_for_viewer, list_project_outputs};
use base64::Engine as _;
use dioxus::document::eval;
use dioxus::prelude::*;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Escape a string so it is safe to embed inside a JS single-quoted string
/// literal (used in eval calls).
fn js_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

#[component]
pub fn OutputsTab(project_name: String) -> Element {
    // ── State ────────────────────────────────────────────────────────────
    let mut refresh_counter = use_signal(|| 0u32);
    let mut error_msg = use_signal(String::new);
    // relative_path of the file currently being downloaded, if any
    let mut downloading = use_signal(|| Option::<String>::None);
    // relative_path of the file currently being loaded for viewing, if any
    let mut viewing = use_signal(|| Option::<String>::None);

    // ── Data ─────────────────────────────────────────────────────────────
    let project_name_res = project_name.clone();
    let files = use_resource(move || {
        let pn = project_name_res.clone();
        async move {
            let _ = refresh_counter(); // subscribe so refresh button re-runs this
            list_project_outputs(pn).await
        }
    });

    // ── Render ───────────────────────────────────────────────────────────
    rsx! {
        div {
            class: "tab-content outputs-tab",
            style: "display:flex;flex-direction:column;gap:0.75rem;padding:0.75rem;height:100%;min-height:0;",

            // ── Toolbar ──────────────────────────────────────────────────
            div {
                style: "display:flex;align-items:center;gap:0.75rem;flex-wrap:wrap;padding:0.5rem 0.75rem;background:var(--primary-color-5);border:1px solid var(--primary-color-6);border-radius:0.5rem;",

                span {
                    style: "font-size:0.9rem;font-weight:600;color:var(--secondary-color);",
                    "Output Files"
                }

                // Spacer
                div { style: "flex:1;" }

                button {
                    style: "padding:0.35rem 0.85rem;border:1px solid var(--primary-color-6);border-radius:0.375rem;background:var(--primary-color-4);color:var(--secondary-color);font-size:0.82rem;cursor:pointer;",
                    onclick: move |_| { refresh_counter += 1; },
                    "↻ Refresh"
                }
            }

            // ── Error banner ──────────────────────────────────────────────
            if !error_msg().is_empty() {
                div {
                    style: "display:flex;align-items:center;gap:0.5rem;padding:0.5rem 0.75rem;background:var(--primary-error-color);color:var(--secondary-error-color);border:1px solid var(--secondary-error-color);border-radius:0.375rem;font-size:0.88rem;",
                    span { style: "flex:1;", "{error_msg}" }
                    button {
                        style: "background:transparent;border:none;color:var(--secondary-error-color);cursor:pointer;font-size:1rem;padding:0.1rem 0.3rem;",
                        onclick: move |_| error_msg.set(String::new()),
                        "✕"
                    }
                }
            }

            // ── File list ─────────────────────────────────────────────────
            match files() {
                None => rsx! {
                    div {
                        style: "text-align:center;padding:3rem 1rem;color:var(--secondary-color-5);font-size:0.95rem;",
                        "Loading output files…"
                    }
                },
                Some(Err(e)) => rsx! {
                    div {
                        style: "text-align:center;padding:3rem 1rem;color:var(--secondary-error-color);font-size:0.95rem;",
                        "Failed to list output files: {e}"
                    }
                },
                Some(Ok(file_list)) if file_list.is_empty() => rsx! {
                    div {
                        style: "text-align:center;padding:3rem 1rem;color:var(--secondary-color-5);font-size:0.95rem;",
                        "No output files yet. Run the pipeline to generate results."
                    }
                },
                Some(Ok(file_list)) => rsx! {
                    div {
                        style: "display:flex;flex-direction:column;gap:0.4rem;overflow-y:auto;flex:1;min-height:0;",

                        for file in file_list.iter() {
                            {
                                let rel_path = file.relative_path.clone();
                                let rel_path_dl = rel_path.clone();
                                let rel_path_view = rel_path.clone();
                                let fname = file.name.clone();
                                let fname_dl = fname.clone();
                                let fname_view = fname.clone();
                                let size_str = format_size(file.size);
                                let is_viewable = file.is_viewable;
                                let pn_dl = project_name.clone();
                                let pn_view = project_name.clone();

                                let is_downloading = downloading() == Some(rel_path.clone());
                                let is_viewing = viewing() == Some(rel_path.clone());

                                rsx! {
                                    div {
                                        key: "{rel_path}",
                                        style: "display:flex;align-items:center;gap:0.5rem;padding:0.5rem 0.75rem;background:var(--primary-color-5);border:1px solid var(--primary-color-6);border-radius:0.5rem;flex-wrap:wrap;",

                                        // File icon + name
                                        span {
                                            style: "font-size:0.88rem;color:var(--secondary-color-5);flex-shrink:0;",
                                            "📄"
                                        }
                                        div {
                                            style: "flex:1;min-width:0;",
                                            div {
                                                style: "font-size:0.88rem;font-weight:600;color:var(--secondary-color);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;",
                                                "{fname}"
                                            }
                                            div {
                                                style: "font-size:0.75rem;color:var(--secondary-color-5);margin-top:0.1rem;",
                                                "{rel_path}  ·  {size_str}"
                                            }
                                        }

                                        // Download button
                                        button {
                                            style: "padding:0.35rem 0.85rem;border:1px solid var(--primary-color-6);border-radius:0.375rem;background:var(--primary-color-4);color:var(--secondary-color);font-size:0.82rem;cursor:pointer;white-space:nowrap;flex-shrink:0;",
                                            disabled: is_downloading,
                                            onclick: move |_| {
                                                let pn = pn_dl.clone();
                                                let rp = rel_path_dl.clone();
                                                let fn_ = fname_dl.clone();
                                                downloading.set(Some(rp.clone()));
                                                let mut err = error_msg;
                                                spawn(async move {
                                                    match get_project_output(pn, rp.clone()).await {
                                                        Ok(bytes) => {
                                                            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                                            let fname_safe = js_escape(&fn_);
                                                            let js = format!(r#"
(function() {{
    const b64 = '{b64}';
    const binary = atob(b64);
    const arr = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) arr[i] = binary.charCodeAt(i);
    const blob = new Blob([arr], {{type: 'application/octet-stream'}});
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = '{fname_safe}';
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
}})();
"#);
                                                            let _ = eval(&js);
                                                        }
                                                        Err(e) => {
                                                            err.set(format!("Download failed: {e}"));
                                                        }
                                                    }
                                                    downloading.set(None);
                                                });
                                            },
                                            if is_downloading { "⏳ Downloading…" } else { "⬇ Download" }
                                        }

                                        // 3D View button (only for viewable files)
                                        if is_viewable {
                                            button {
                                                style: "padding:0.35rem 0.85rem;border:1px solid var(--focused-border-color);border-radius:0.375rem;background:var(--primary-color-4);color:var(--focused-border-color);font-size:0.82rem;cursor:pointer;white-space:nowrap;flex-shrink:0;",
                                                disabled: is_viewing,
                                                onclick: move |_| {
                                                    let pn = pn_view.clone();
                                                    let rp = rel_path_view.clone();
                                                    let fn_ = fname_view.clone();
                                                    viewing.set(Some(rp.clone()));
                                                    let mut err = error_msg;
                                                    spawn(async move {
                                                        match get_project_output_for_viewer(pn, rp.clone()).await {
                                                            Ok(bytes) => {
                                                                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                                                let fname_safe = js_escape(&fn_);
                                                                launch_ply_viewer(&b64, &fname_safe);
                                                            }
                                                            Err(e) => {
                                                                err.set(format!("Failed to load viewer data: {e}"));
                                                            }
                                                        }
                                                        viewing.set(None);
                                                    });
                                                },
                                                if is_viewing { "⏳ Loading…" } else { "🔳 View 3D" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Three.js viewer launcher
// ---------------------------------------------------------------------------

fn launch_ply_viewer(b64: &str, fname_safe: &str) {
    let js = format!(
        r#"
(async function() {{
    // --- Decode base64 to bytes and create a blob URL ---
    const b64 = '{b64}';
    const binary = atob(b64);
    const arr = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) arr[i] = binary.charCodeAt(i);
    const blob = new Blob([arr], {{type: 'application/octet-stream'}});
    const blobUrl = URL.createObjectURL(blob);

    // --- Remove any existing viewer overlay ---
    const existing = document.getElementById('ply-viewer-overlay');
    if (existing) existing.remove();

    // --- Build overlay ---
    const overlay = document.createElement('div');
    overlay.id = 'ply-viewer-overlay';
    overlay.style.cssText = 'position:fixed;inset:0;background:#0d1117;z-index:9999;display:flex;flex-direction:column;align-items:stretch;';
    document.body.appendChild(overlay);

    // Header bar
    const header = document.createElement('div');
    header.style.cssText = 'display:flex;align-items:center;padding:8px 16px;background:#161b22;border-bottom:1px solid #30363d;gap:12px;flex-shrink:0;';
    const title = document.createElement('span');
    title.style.cssText = 'color:#e6edf3;font-family:monospace;font-size:14px;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;';
    title.textContent = '3D Viewer \u2014 {fname_safe}';
    const closeBtn = document.createElement('button');
    closeBtn.textContent = '\u2715 Close';
    closeBtn.style.cssText = 'padding:6px 14px;background:#21262d;color:#e6edf3;border:1px solid #30363d;border-radius:6px;cursor:pointer;font-size:13px;flex-shrink:0;';
    closeBtn.onclick = () => {{ URL.revokeObjectURL(blobUrl); overlay.remove(); }};
    header.appendChild(title);
    header.appendChild(closeBtn);
    overlay.appendChild(header);

    // Loading indicator
    const loading = document.createElement('div');
    loading.style.cssText = 'color:#8b949e;font-family:monospace;font-size:13px;padding:24px;text-align:center;flex-shrink:0;';
    loading.textContent = 'Loading Three.js\u2026';
    overlay.appendChild(loading);

    // Canvas fills remaining space
    const canvas = document.createElement('canvas');
    canvas.style.cssText = 'flex:1;display:block;min-height:0;width:100%;';
    overlay.appendChild(canvas);

    try {{
        const THREE = await import('https://cdn.jsdelivr.net/npm/three@0.169.0/build/three.module.js');
        const {{ PLYLoader }} = await import('https://cdn.jsdelivr.net/npm/three@0.169.0/examples/jsm/loaders/PLYLoader.js');
        const {{ OrbitControls }} = await import('https://cdn.jsdelivr.net/npm/three@0.169.0/examples/jsm/controls/OrbitControls.js');

        loading.remove();

        // Scene
        const scene = new THREE.Scene();
        scene.background = new THREE.Color(0x0d1117);

        const w = canvas.clientWidth || 800;
        const h = canvas.clientHeight || 600;
        const camera = new THREE.PerspectiveCamera(60, w / h, 0.001, 10000);
        camera.position.set(0, 0, 5);

        const renderer = new THREE.WebGLRenderer({{ canvas, antialias: true }});
        renderer.setSize(w, h, false);
        renderer.setPixelRatio(window.devicePixelRatio || 1);

        const controls = new OrbitControls(camera, renderer.domElement);
        controls.enableDamping = true;
        controls.dampingFactor = 0.05;

        // Lights
        scene.add(new THREE.AmbientLight(0xffffff, 0.6));
        const dl = new THREE.DirectionalLight(0xffffff, 0.8);
        dl.position.set(1, 2, 3);
        scene.add(dl);

        // Load PLY
        loading.style.cssText = 'color:#8b949e;font-family:monospace;font-size:13px;padding:8px 24px;text-align:center;flex-shrink:0;';
        loading.textContent = 'Parsing PLY geometry\u2026';
        overlay.insertBefore(loading, canvas);

        const loader = new PLYLoader();
        const geometry = await new Promise((res, rej) => loader.load(blobUrl, res, undefined, rej));
        geometry.computeVertexNormals();

        loading.remove();

        let mesh;
        const hasIndex = geometry.index !== null;
        const hasColor = !!geometry.attributes.color;

        if (hasIndex) {{
            // Mesh geometry
            const mat = hasColor
                ? new THREE.MeshPhongMaterial({{ vertexColors: true, side: THREE.DoubleSide }})
                : new THREE.MeshPhongMaterial({{ color: 0x7a8fa6, side: THREE.DoubleSide }});
            mesh = new THREE.Mesh(geometry, mat);
        }} else {{
            // Point cloud
            const mat = hasColor
                ? new THREE.PointsMaterial({{ vertexColors: true, size: 0.015 }})
                : new THREE.PointsMaterial({{ color: 0x4fc3f7, size: 0.015 }});
            mesh = new THREE.Points(geometry, mat);
        }}
        scene.add(mesh);

        // Frame the object
        const box = new THREE.Box3().setFromObject(mesh);
        const center = box.getCenter(new THREE.Vector3());
        const size = box.getSize(new THREE.Vector3());
        const maxDim = Math.max(size.x, size.y, size.z) || 1;
        mesh.position.sub(center);
        camera.position.set(0, 0, maxDim * 2.5);
        camera.near = maxDim * 0.0005;
        camera.far = maxDim * 200;
        camera.updateProjectionMatrix();
        controls.update();

        // Respond to canvas resize
        const ro = new ResizeObserver(() => {{
            const nw = canvas.clientWidth;
            const nh = canvas.clientHeight;
            if (nw > 0 && nh > 0) {{
                camera.aspect = nw / nh;
                camera.updateProjectionMatrix();
                renderer.setSize(nw, nh, false);
            }}
        }});
        ro.observe(canvas);

        // Wire close button to also dispose renderer
        const origClose = closeBtn.onclick;
        closeBtn.onclick = () => {{ ro.disconnect(); renderer.dispose(); origClose(); }};

        // Render loop
        function animate() {{
            if (!document.contains(overlay)) {{ ro.disconnect(); renderer.dispose(); return; }}
            requestAnimationFrame(animate);
            controls.update();
            renderer.render(scene, camera);
        }}
        animate();
    }} catch (err) {{
        loading.style.color = '#f85149';
        loading.textContent = 'Error loading viewer: ' + err.message;
        // Make sure loading element is visible
        if (!overlay.contains(loading)) overlay.insertBefore(loading, canvas);
    }}
}})();
"#
    );
    let _ = eval(&js);
}
