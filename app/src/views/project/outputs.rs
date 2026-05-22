use crate::server::{delete_project_output, get_project_output_for_viewer, list_project_outputs};
use base64::Engine as _;
use dioxus::document::eval;
use dioxus::prelude::*;
use tracing::{debug, error, info};
use urlencoding::encode as url_encode;

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
    debug!(project_name = %project_name, "Initializing outputs tab");
    // State
    let mut refresh_counter = use_signal(|| 0u32);
    let mut error_msg = use_signal(String::new);
    // relative_path of the file currently being loaded for viewing, if any
    let mut viewing = use_signal(|| Option::<String>::None);
    // relative_path of the file pending delete confirmation, if any
    let mut confirming_delete = use_signal(|| Option::<String>::None);
    // relative_path of the file currently being deleted, if any
    let mut deleting_path = use_signal(|| Option::<String>::None);

    // Data
    let project_name_res = project_name.clone();
    let files = use_resource(move || {
        let pn = project_name_res.clone();
        async move {
            let _ = refresh_counter(); // subscribe so refresh button re-runs this
            debug!(project_name = %pn, "Fetching output files list");
            match list_project_outputs(pn.clone()).await {
                Ok(files) => {
                    let file_count = files.len();
                    info!(project_name = %pn, file_count = file_count, "Successfully loaded output files");
                    Ok(files)
                }
                Err(e) => {
                    error!(project_name = %pn, error = %e, "Failed to load output files");
                    Err(e)
                }
            }
        }
    });

    // ── Render ───────────────────────────────────────────────────────────
    rsx! {
        document::Link { rel: "stylesheet", href: asset!("/assets/views/project/outputs.css") }
        div {
            class: "tab-content outputs-tab",

            // ── Toolbar ──────────────────────────────────────────────────
            div {
                class: "outputs-toolbar",

                span {
                    class: "outputs-toolbar-title",
                    "Output Files"
                }

                // Spacer
                div { class: "outputs-toolbar-spacer" }

                button {
                    class: "outputs-btn outputs-refresh-btn",
                    onclick: move |_| {
                        debug!(project_name = %project_name, "User clicked refresh output files");
                        refresh_counter += 1;
                    },
                    "↺"
                    span { class: "btn-label", " Refresh" }
                }
            }

            // ── Error banner ──────────────────────────────────────────────
            if !error_msg().is_empty() {
                div {
                    class: "outputs-error-banner",
                    span { "{error_msg}" }
                    button {
                        class: "outputs-error-dismiss",
                        onclick: move |_| error_msg.set(String::new()),
                        "✕"
                    }
                }
            }

            // ── File list ─────────────────────────────────────────────────
            match files() {
                None => rsx! {
                    div {
                        class: "outputs-placeholder",
                        "Loading output files…"
                    }
                },
                Some(Err(e)) => rsx! {
                    div {
                        class: "outputs-placeholder error",
                        "Failed to list output files: {e}"
                    }
                },
                Some(Ok(file_list)) if file_list.is_empty() => rsx! {
                    div {
                        class: "outputs-placeholder",
                        "No output files yet. Run the pipeline to generate results."
                    }
                },
                Some(Ok(file_list)) => rsx! {
                    div {
                        class: "outputs-file-list",

                        for file in file_list.iter() {
                            {
                                let rel_path = file.relative_path.clone();
                                let fname = file.name.clone();
                                let size_str = format_size(file.size);
                                let is_viewable = file.is_viewable;
                                let pn = project_name.clone();
                                let rel_path_enc = url_encode(&rel_path).to_string();

                                // Per-item view clone captures
                                let pn_view = pn.clone();
                                let rp_view = rel_path.clone();
                                let fn_view = fname.clone();

                                // Per-item delete clone captures
                                let rp_confirm = rel_path.clone();
                                let pn_del = pn.clone();
                                let rp_del = rel_path.clone();

                                // Current reactive states for this item
                                let is_viewing_this = viewing() == Some(rel_path.clone());
                                let is_confirming = confirming_delete() == Some(rel_path.clone());
                                let is_deleting = deleting_path() == Some(rel_path.clone());

                                rsx! {
                                    div {
                                        key: "{rel_path}",
                                        class: "outputs-file-item",

                                        // File icon
                                        span {
                                            class: "outputs-file-icon",
                                            "📄"
                                        }

                                        // Name + meta
                                        div {
                                            class: "outputs-file-info-wrapper",
                                            div {
                                                class: "outputs-file-name",
                                                "{fname}"
                                            }
                                            div {
                                                class: "outputs-file-meta",
                                                "{rel_path}  ·  {size_str}"
                                            }
                                        }

                                        // Download button
                                        a {
                                            href: "/projects/{pn}/outputs/file?relative_path={rel_path_enc}",
                                            download: "{fname}",
                                            class: "outputs-btn outputs-download-link",
                                            "⬇"
                                            span { class: "btn-label", " Download" }
                                        }

                                        // 3D View button (only for viewable files)
                                        if is_viewable {
                                            button {
                                                class: "outputs-btn outputs-view-3d-btn",
                                                disabled: is_viewing_this,
                                                onclick: move |_| {
                                                    let pn = pn_view.clone();
                                                    let rp = rp_view.clone();
                                                    let fn_ = fn_view.clone();
                                                    debug!(project_name = %pn, file_name = %fn_, "User clicked view 3D");
                                                    viewing.set(Some(rp.clone()));
                                                    let mut err = error_msg;
                                                    spawn(async move {
                                                        debug!(project_name = %pn, file_path = %rp, "Loading file for 3D viewer");
                                                        match get_project_output_for_viewer(pn.clone(), rp.clone()).await {
                                                            Ok(bytes) => {
                                                                info!(project_name = %pn, file_name = %fn_, bytes_loaded = bytes.len(), "Successfully loaded file for 3D viewer");
                                                                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                                                let fname_safe = js_escape(&fn_);
                                                                launch_glb_viewer(&b64, &fname_safe).await;
                                                            }
                                                            Err(e) => {
                                                                error!(project_name = %pn, file_name = %fn_, error = %e, "Failed to load file for 3D viewer");
                                                                err.set(format!("Failed to load viewer data: {e}"));
                                                            }
                                                        }
                                                        viewing.set(None);
                                                    });
                                                },
                                                if is_viewing_this { "⏳" } else { "🔳" }
                                                span {
                                                    class: "btn-label",
                                                    if is_viewing_this { " Loading…" } else { " View 3D" }
                                                }
                                            }
                                        }

                                        // Delete area
                                        if is_deleting {
                                            span {
                                                class: "outputs-file-icon",
                                                title: "Deleting…",
                                                "⏳"
                                            }
                                        } else if is_confirming {
                                            button {
                                                class: "outputs-btn outputs-confirm-del-btn",
                                                title: "Confirm delete",
                                                onclick: move |_| {
                                                    let pn = pn_del.clone();
                                                    let rp = rp_del.clone();
                                                    deleting_path.set(Some(rp.clone()));
                                                    confirming_delete.set(None);
                                                    spawn(async move {
                                                        match delete_project_output(pn.clone(), rp.clone()).await {
                                                            Ok(()) => {
                                                                info!(project_name = %pn, file_path = %rp, "Output file deleted");
                                                                refresh_counter += 1;
                                                            }
                                                            Err(e) => {
                                                                error!(project_name = %pn, file_path = %rp, error = %e, "Failed to delete output file");
                                                                error_msg.set(format!("Failed to delete: {e}"));
                                                            }
                                                        }
                                                        deleting_path.set(None);
                                                    });
                                                },
                                                "✓"
                                                span { class: "btn-label", " Sure?" }
                                            }
                                            button {
                                                class: "outputs-btn outputs-cancel-del-btn",
                                                title: "Cancel delete",
                                                onclick: move |_| confirming_delete.set(None),
                                                "✗"
                                            }
                                        } else {
                                            button {
                                                class: "outputs-btn outputs-del-btn",
                                                title: "Delete file",
                                                onclick: move |_| confirming_delete.set(Some(rp_confirm.clone())),
                                                "🗑"
                                                span { class: "btn-label", " Delete" }
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
// 3-D Viewer (launched via eval'd JavaScript)
// ---------------------------------------------------------------------------

async fn launch_glb_viewer(b64: &str, fname_safe: &str) {
    info!(file_name = %fname_safe, "Launching 3D GLB viewer");
    let b64_esc = js_escape(b64);
    let fname_esc = js_escape(fname_safe);

    let js = format!(
        r#"(async () => {{
    console.log('[3D Viewer] Starting viewer setup...');
    try {{
        const THREE = await import('https://esm.sh/three@0.169.0');
        const {{ GLTFLoader }} = await import('https://esm.sh/three@0.169.0/examples/jsm/loaders/GLTFLoader.js');
        const {{ TrackballControls }} = await import('https://esm.sh/three@0.169.0/examples/jsm/controls/TrackballControls.js');
        console.log('[3D Viewer] Libraries loaded');

        const b64 = '{}';
        const fname = '{}';

        // Decode GLB bytes and create a blob URL
        const binary = atob(b64);
        const arr = new Uint8Array(binary.length);
        for (let i = 0; i < binary.length; i++) arr[i] = binary.charCodeAt(i);
        const blob = new Blob([arr], {{type: 'model/gltf-binary'}});
        const blobUrl = URL.createObjectURL(blob);

        // Remove any existing overlay
        const existing = document.getElementById('ply-viewer-overlay');
        if (existing) existing.remove();

        // Overlay container
        const overlay = document.createElement('div');
        overlay.id = 'ply-viewer-overlay';
        overlay.style.cssText = 'position:fixed;inset:0;background:#0d1117;z-index:9999;display:flex;flex-direction:column;align-items:stretch;';
        document.body.appendChild(overlay);

        // Header bar
        const headerDiv = document.createElement('div');
        headerDiv.style.cssText = 'display:flex;align-items:center;padding:8px 16px;background:#161b22;border-bottom:1px solid #30363d;gap:12px;flex-shrink:0;';
        const titleSpan = document.createElement('span');
        titleSpan.style.cssText = 'color:#e6edf3;font-family:monospace;font-size:14px;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;';
        titleSpan.textContent = '3D Viewer — ' + fname;
        const closeBtn = document.createElement('button');
        closeBtn.textContent = '✕ Close';
        closeBtn.style.cssText = 'padding:6px 14px;background:#21262d;color:#e6edf3;border:1px solid #30363d;border-radius:6px;cursor:pointer;font-size:13px;flex-shrink:0;';
        closeBtn.onclick = () => {{ URL.revokeObjectURL(blobUrl); overlay.remove(); }};
        headerDiv.appendChild(titleSpan);
        headerDiv.appendChild(closeBtn);
        overlay.appendChild(headerDiv);

        // Controls bar — reset button always present; dynamic slider added after PLY loads
        const controlsDiv = document.createElement('div');
        controlsDiv.style.cssText = 'display:flex;align-items:center;gap:12px;padding:8px 16px;background:#161b22;border-bottom:1px solid #30363d;flex-wrap:wrap;flex-shrink:0;';
        const resetBtn = document.createElement('button');
        resetBtn.textContent = 'Reset View';
        resetBtn.style.cssText = 'padding:4px 12px;background:#21262d;color:#e6edf3;border:1px solid #30363d;border-radius:4px;cursor:pointer;font-size:12px;';
        controlsDiv.appendChild(resetBtn);
        overlay.appendChild(controlsDiv);

        // Loading message
        const loadingDiv = document.createElement('div');
        loadingDiv.id = 'ply-viewer-overlay-loading';
        loadingDiv.style.cssText = 'color:#8b949e;font-family:monospace;font-size:13px;padding:24px;text-align:center;flex-shrink:0;';
        loadingDiv.textContent = 'Initializing 3D viewer...';
        overlay.appendChild(loadingDiv);

        // Canvas
        const canvas = document.createElement('canvas');
        canvas.id = 'ply-viewer-canvas';
        canvas.style.cssText = 'flex:1;display:block;min-height:0;width:100%;height:100%;';
        overlay.appendChild(canvas);

        await new Promise(r => setTimeout(r, 0));
        let w = canvas.clientWidth || window.innerWidth;
        let h = canvas.clientHeight || (window.innerHeight - 100);

        // Scene
        const scene = new THREE.Scene();
        scene.background = new THREE.Color(0x0d1117);
        const camera = new THREE.PerspectiveCamera(60, w / h, 0.001, 10000);
        camera.position.set(0, 0, 5);
        scene.add(camera);

        const renderer = new THREE.WebGLRenderer({{ canvas, antialias: true, preserveDrawingBuffer: true }});
        renderer.setSize(w, h, false);
        renderer.setPixelRatio(window.devicePixelRatio || 1);
        renderer.setClearColor(0x0d1117);
        if (THREE.SRGBColorSpace) renderer.outputColorSpace = THREE.SRGBColorSpace;

        const controls = new TrackballControls(camera, renderer.domElement);
        controls.rotateSpeed = 2.5; controls.zoomSpeed = 1.2; controls.panSpeed = 0.8;

        // Inertia
        let rotVel = new THREE.Vector3();
        let isRotating = false, lastX = 0, lastY = 0;
        renderer.domElement.addEventListener('mousedown', () => {{ isRotating = true; rotVel.set(0,0,0); }});
        renderer.domElement.addEventListener('mouseup',   () => {{ isRotating = false; }});
        renderer.domElement.addEventListener('mousemove', e => {{
            if (isRotating) {{ rotVel.x = (e.clientY-lastY)*0.001; rotVel.y = (e.clientX-lastX)*0.001; }}
            lastX = e.clientX; lastY = e.clientY;
        }});

        // Lighting: ambient + directional attached to camera
        scene.add(new THREE.AmbientLight(0xffffff, 0.4));
        const dl = new THREE.DirectionalLight(0xffffff, 1.0);
        dl.position.set(0, 0.5, 1);
        camera.add(dl);
        const dlTarget = new THREE.Object3D();
        dlTarget.position.set(0, 0, -1);
        camera.add(dlTarget);
        dl.target = dlTarget;

        loadingDiv.textContent = 'Loading 3D model...';
        const loader = new GLTFLoader();
        const initialCamPos = new THREE.Vector3();

        loader.load(blobUrl, (gltf) => {{
            try {{
                loadingDiv.remove();
                const model = gltf.scene;

                // Post-process meshes and detect point clouds
                let hasPoints = false;
                let hasMesh = false;
                model.traverse(child => {{
                    if (child.isPoints) {{
                        hasPoints = true;
                        // Override with a PointsMaterial that respects vertex colors
                        const hasVCol = !!child.geometry.attributes.color;
                        child.material = new THREE.PointsMaterial({{
                            vertexColors: hasVCol,
                            color: hasVCol ? 0xffffff : 0x4fc3f7,
                            size: 0.1,
                            sizeAttenuation: true,
                        }});
                    }}
                    if (child.isMesh) {{
                        hasMesh = true;
                        child.material.side = THREE.DoubleSide;
                    }}
                }});

                scene.add(model);
                console.log('[3D Viewer] Model added — hasPoints:', hasPoints, 'hasMesh:', hasMesh);

                // Fit camera to bounding box
                const box = new THREE.Box3().setFromObject(model);
                const center = box.getCenter(new THREE.Vector3());
                const size = box.getSize(new THREE.Vector3());
                const maxDim = Math.max(size.x, size.y, size.z) || 1;
                model.position.sub(center);
                camera.position.set(0, 0, maxDim * 2.5);
                initialCamPos.copy(camera.position);
                camera.near = maxDim * 0.0005;
                camera.far  = maxDim * 200;
                camera.updateProjectionMatrix();

                // Dynamic slider
                if (hasPoints) {{
                    const scaleLabel = document.createElement('label');
                    scaleLabel.style.cssText = 'color:#e6edf3;font-family:monospace;font-size:12px;display:flex;align-items:center;gap:8px;';
                    scaleLabel.textContent = 'Point Scale:';
                    const scaleSlider = document.createElement('input');
                    scaleSlider.type='range'; scaleSlider.min='0.1'; scaleSlider.max='5'; scaleSlider.step='0.1'; scaleSlider.value='1';
                    scaleSlider.style.cssText = 'width:120px;cursor:pointer;';
                    const scaleVal = document.createElement('span');
                    scaleVal.style.cssText = 'color:#8b949e;font-family:monospace;font-size:12px;min-width:30px;';
                    scaleVal.textContent = '1.0x';
                    scaleSlider.oninput = e => {{
                        const s = parseFloat(e.target.value);
                        scaleVal.textContent = s.toFixed(1)+'x';
                        model.traverse(c => {{ if (c.isPoints) c.material.size = 0.1 * s; }});
                    }};
                    scaleLabel.appendChild(scaleSlider);
                    scaleLabel.appendChild(scaleVal);
                    controlsDiv.insertBefore(scaleLabel, resetBtn);
                }} else {{
                    const yawLabel = document.createElement('label');
                    yawLabel.style.cssText = 'color:#e6edf3;font-family:monospace;font-size:12px;display:flex;align-items:center;gap:8px;';
                    yawLabel.textContent = 'Light Angle:';
                    const yawSlider = document.createElement('input');
                    yawSlider.type='range'; yawSlider.min='-180'; yawSlider.max='180'; yawSlider.step='5'; yawSlider.value='0';
                    yawSlider.style.cssText = 'width:120px;cursor:pointer;';
                    const yawVal = document.createElement('span');
                    yawVal.style.cssText = 'color:#8b949e;font-family:monospace;font-size:12px;min-width:36px;';
                    yawVal.textContent = '0\u00b0';
                    yawSlider.oninput = e => {{
                        const yaw = parseFloat(e.target.value) * Math.PI / 180;
                        yawVal.textContent = e.target.value + '\u00b0';
                        dl.position.set(Math.sin(yaw), 0.5, Math.cos(yaw));
                    }};
                    yawLabel.appendChild(yawSlider);
                    yawLabel.appendChild(yawVal);
                    controlsDiv.insertBefore(yawLabel, resetBtn);
                }}

                resetBtn.onclick = () => {{
                    camera.position.copy(initialCamPos);
                    controls.reset();
                    rotVel.set(0,0,0);
                }};

                const ro = new ResizeObserver(() => {{
                    const nw = canvas.clientWidth, nh = canvas.clientHeight;
                    if (nw > 0 && nh > 0) {{
                        camera.aspect = nw / nh;
                        camera.updateProjectionMatrix();
                        renderer.setSize(nw, nh, false);
                    }}
                }});
                ro.observe(canvas);

                const origClose = closeBtn.onclick;
                closeBtn.onclick = () => {{ URL.revokeObjectURL(blobUrl); ro.disconnect(); renderer.dispose(); origClose(); }};

                const animate = () => {{
                    if (!document.contains(overlay)) {{ URL.revokeObjectURL(blobUrl); ro.disconnect(); renderer.dispose(); return; }}
                    requestAnimationFrame(animate);
                    if (!isRotating && (Math.abs(rotVel.x) > 0.001 || Math.abs(rotVel.y) > 0.001)) {{
                        const qy = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0,1,0), rotVel.y);
                        const qx = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(1,0,0), rotVel.x);
                        camera.position.applyQuaternion(new THREE.Quaternion().multiplyQuaternions(qy, qx));
                        rotVel.multiplyScalar(0.95);
                    }}
                    controls.update();
                    renderer.render(scene, camera);
                }};
                animate();
            }} catch (innerErr) {{
                console.error('[3D Viewer] Error processing model:', innerErr);
                const ld = document.getElementById('ply-viewer-overlay-loading');
                if (ld) {{ ld.style.color='#f85149'; ld.textContent='Error: '+(innerErr.message||'Failed to process model'); }}
            }}
        }}, undefined, err => {{
            console.error('[3D Viewer] Error loading GLB:', err);
            const ld = document.getElementById('ply-viewer-overlay-loading');
            if (ld) {{ ld.style.color='#f85149'; ld.textContent='Error: '+(err.message||'Failed to load model'); }}
        }});

    }} catch (err) {{
        console.error('[3D Viewer] Fatal error:', err.stack || err);
        const ld = document.getElementById('ply-viewer-overlay-loading');
        if (ld) {{ ld.style.color='#f85149'; ld.textContent='Error: '+(err.message||'Failed to initialize'); }}
    }}
}})();"#,
        b64_esc, fname_esc
    );

    if let Err(e) = eval(&js).await {
        let err_msg = format!("Failed to execute 3D viewer: {:?}", e);
        let _ = eval(&format!(
            "console.error('{}');",
            err_msg.replace('"', "\\\"")
        ))
        .await;
    }
}
