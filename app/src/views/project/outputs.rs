use crate::server::{get_project_output_for_viewer, list_project_outputs};
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
                    class: "outputs-refresh-btn",
                    onclick: move |_| {
                        debug!(project_name = %project_name, "User clicked refresh output files");
                        refresh_counter += 1;
                    },
                    "Refresh"
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
                                let rel_path_dl = rel_path.clone();
                                let rel_path_view = rel_path.clone();
                                let fname = file.name.clone();
                                let fname_dl = fname.clone();
                                let fname_view = fname.clone();
                                let size_str = format_size(file.size);
                                let is_viewable = file.is_viewable;
                                let pn_dl = project_name.clone();
                                let pn_view = project_name.clone();

                                let is_viewing = viewing() == Some(rel_path.clone());

                                rsx! {
                                    div {
                                        key: "{rel_path}",
                                        class: "outputs-file-item",

                                        // File icon + name
                                        span {
                                            class: "outputs-file-icon",
                                            "📄"
                                        }
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

                                        // Download button (direct URL for streaming)
                                        a {
                                            href: "/projects/{pn_dl}/outputs/file?relative_path={url_encode(&rel_path_dl)}",
                                            download: "{fname_dl}",
                                            class: "outputs-download-link",
                                            "⬇ Download"
                                        }

                                        // 3D View button (only for viewable files)
                                        if is_viewable {
                                            button {
                                                class: "outputs-view-3d-btn",
                                                disabled: is_viewing,
                                                onclick: move |_| {
                                                    let pn = pn_view.clone();
                                                    let rp = rel_path_view.clone();
                                                    let fn_ = fname_view.clone();
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
                                                                launch_ply_viewer(&b64, &fname_safe).await;
                                                            }
                                                            Err(e) => {
                                                                error!(project_name = %pn, file_name = %fn_, error = %e, "Failed to load file for 3D viewer");
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

// Three.js viewer launcher using CDN with dynamic imports and TrackballControls
// ---------------------------------------------------------------------------

async fn launch_ply_viewer(b64: &str, fname_safe: &str) {
    info!(file_name = %fname_safe, "Launching 3D PLY viewer");
    debug!("Encoding and preparing PLY data for viewer");
    let b64_esc = js_escape(b64);
    let fname_esc = js_escape(fname_safe);

    let js = format!(
        r#"(async () => {{
    console.log('[3D Viewer] Starting viewer setup...');

    try {{
        console.log('[3D Viewer] Loading libraries from esm.sh CDN...');
        const THREE = await import('https://esm.sh/three@0.169.0');
        const PLYLoaderMod = await import('https://esm.sh/three@0.169.0/examples/jsm/loaders/PLYLoader.js');
        const TrackballControlsMod = await import('https://esm.sh/three@0.169.0/examples/jsm/controls/TrackballControls.js');

        const PLYLoader = PLYLoaderMod.PLYLoader;
        const TrackballControls = TrackballControlsMod.TrackballControls;

        console.log('[3D Viewer] Three.js, PLYLoader and TrackballControls loaded');

        const b64 = '{}';
        const fname = '{}';

        const binary = atob(b64);
        const arr = new Uint8Array(binary.length);
        for (let i = 0; i < binary.length; i++) arr[i] = binary.charCodeAt(i);
        const blob = new Blob([arr], {{type: 'application/octet-stream'}});
        const blobUrl = URL.createObjectURL(blob);

        const existing = document.getElementById('ply-viewer-overlay');
        if (existing) existing.remove();

        const overlay = document.createElement('div');
        overlay.id = 'ply-viewer-overlay';
        overlay.style.cssText = 'position:fixed;inset:0;background:#0d1117;z-index:9999;display:flex;flex-direction:column;align-items:stretch;';
        document.body.appendChild(overlay);

        const header = document.createElement('div');
        header.style.cssText = 'display:flex;align-items:center;padding:8px 16px;background:#161b22;border-bottom:1px solid #30363d;gap:12px;flex-shrink:0;';
        const title = document.createElement('span');
        title.style.cssText = 'color:#e6edf3;font-family:monospace;font-size:14px;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;';
        title.textContent = '3D Viewer — ' + fname;
        const closeBtn = document.createElement('button');
        closeBtn.textContent = '✕ Close';
        closeBtn.style.cssText = 'padding:6px 14px;background:#21262d;color:#e6edf3;border:1px solid #30363d;border-radius:6px;cursor:pointer;font-size:13px;flex-shrink:0;';
        closeBtn.onclick = () => {{ URL.revokeObjectURL(blobUrl); overlay.remove(); }};
        header.appendChild(title);
        header.appendChild(closeBtn);
        overlay.appendChild(header);

        const controlsDiv = document.createElement('div');
        controlsDiv.style.cssText = 'display:flex;align-items:center;gap:12px;padding:8px 16px;background:#161b22;border-bottom:1px solid #30363d;flex-wrap:wrap;flex-shrink:0;';

        const scaleLabel = document.createElement('label');
        scaleLabel.style.cssText = 'color:#e6edf3;font-family:monospace;font-size:12px;display:flex;align-items:center;gap:8px;';
        scaleLabel.textContent = 'Point Scale:';

        const scaleSlider = document.createElement('input');
        scaleSlider.type = 'range';
        scaleSlider.min = '0.1';
        scaleSlider.max = '5';
        scaleSlider.step = '0.1';
        scaleSlider.value = '1';
        scaleSlider.style.cssText = 'width:120px;cursor:pointer;';

        const scaleValue = document.createElement('span');
        scaleValue.style.cssText = 'color:#8b949e;font-family:monospace;font-size:12px;min-width:30px;';
        scaleValue.textContent = '1.0x';

        scaleLabel.appendChild(scaleSlider);
        scaleLabel.appendChild(scaleValue);
        controlsDiv.appendChild(scaleLabel);

        const resetBtn = document.createElement('button');
        resetBtn.textContent = 'Reset View';
        resetBtn.style.cssText = 'padding:4px 12px;background:#21262d;color:#e6edf3;border:1px solid #30363d;border-radius:4px;cursor:pointer;font-size:12px;';
        controlsDiv.appendChild(resetBtn);

        overlay.appendChild(controlsDiv);

        const loading = document.createElement('div');
        loading.id = 'ply-viewer-overlay-loading';
        loading.style.cssText = 'color:#8b949e;font-family:monospace;font-size:13px;padding:24px;text-align:center;flex-shrink:0;';
        loading.textContent = 'Initializing 3D viewer...';
        overlay.appendChild(loading);

        const canvas = document.createElement('canvas');
        canvas.id = 'ply-viewer-canvas';
        canvas.style.cssText = 'flex:1;display:block;min-height:0;width:100%;height:100%;';
        overlay.appendChild(canvas);

        console.log('[3D Viewer] Canvas dimensions:', canvas.clientWidth, 'x', canvas.clientHeight);

        await new Promise(resolve => setTimeout(resolve, 0));

        let w = canvas.clientWidth;
        let h = canvas.clientHeight;
        console.log('[3D Viewer] After reflow - Canvas dimensions:', w, 'x', h);

        if (w === 0) w = window.innerWidth;
        if (h === 0) h = window.innerHeight - 100;

        console.log('[3D Viewer] Using dimensions:', w, 'x', h);
        console.log('[3D Viewer] Initializing Three.js scene...');

        const scene = new THREE.Scene();
        scene.background = new THREE.Color(0x0d1117);

        const camera = new THREE.PerspectiveCamera(60, w / h, 0.001, 10000);
        camera.position.set(0, 0, 5);

        const renderer = new THREE.WebGLRenderer({{ canvas, antialias: true, preserveDrawingBuffer: true }});
        renderer.setSize(w, h, false);
        renderer.setPixelRatio(window.devicePixelRatio || 1);
        renderer.setClearColor(0x0d1117);
        console.log('[3D Viewer] Renderer initialized with size:', w, 'x', h);

        const controls = new TrackballControls(camera, renderer.domElement);
        controls.rotateSpeed = 2.5;
        controls.zoomSpeed = 1.2;
        controls.panSpeed = 0.8;

        // Inertia parameters
        let rotationVelocity = new THREE.Vector3();
        let isRotating = false;
        let lastX = 0, lastY = 0;
        const inertiaFriction = 0.95;
        const inertiaThreshold = 0.001;

        renderer.domElement.addEventListener('mousedown', () => {{
            isRotating = true;
            rotationVelocity.set(0, 0, 0);
        }});

        renderer.domElement.addEventListener('mouseup', () => {{
            isRotating = false;
        }});

        renderer.domElement.addEventListener('mousemove', (e) => {{
            if (isRotating) {{
                const deltaX = e.clientX - lastX;
                const deltaY = e.clientY - lastY;
                rotationVelocity.x = deltaY * 0.001;
                rotationVelocity.y = deltaX * 0.001;
            }}
            lastX = e.clientX;
            lastY = e.clientY;
        }});

        scene.add(new THREE.AmbientLight(0xffffff, 0.6));
        const dl = new THREE.DirectionalLight(0xffffff, 0.8);
        dl.position.set(1, 2, 3);
        scene.add(dl);

        loading.textContent = 'Loading PLY file...';
        const loader = new PLYLoader();
        console.log('[3D Viewer] Loading PLY geometry...');

        let mesh = null;
        let originalGeometry = null;
        const initialCameraPos = new THREE.Vector3();

        loader.load(blobUrl, (geometry) => {{
            console.log('[3D Viewer] PLY loaded, rendering geometry...');
            console.log('[3D Viewer] Geometry vertices:', geometry.attributes.position?.count || 0);

            geometry.computeVertexNormals();
            originalGeometry = geometry.clone();
            loading.remove();

            const hasIndex = geometry.index !== null;
            const hasColor = !!geometry.attributes.color;
            console.log('[3D Viewer] Has index:', hasIndex, 'Has color:', hasColor);

            if (hasIndex) {{
                const mat = hasColor ? new THREE.MeshPhongMaterial({{ vertexColors: true, side: THREE.DoubleSide }}) : new THREE.MeshPhongMaterial({{ color: 0x7a8fa6, side: THREE.DoubleSide }});
                mesh = new THREE.Mesh(geometry, mat);
            }} else {{
                const mat = hasColor ? new THREE.PointsMaterial({{ vertexColors: true, size: 0.1 }}) : new THREE.PointsMaterial({{ color: 0x4fc3f7, size: 0.1 }});
                mesh = new THREE.Points(geometry, mat);
            }}
            scene.add(mesh);
            console.log('[3D Viewer] Mesh added to scene');

            const box = new THREE.Box3().setFromObject(mesh);
            const center = box.getCenter(new THREE.Vector3());
            const size = box.getSize(new THREE.Vector3());
            const maxDim = Math.max(size.x, size.y, size.z) || 1;
            console.log('[3D Viewer] Geometry bounds - center:', center, 'size:', size, 'maxDim:', maxDim);

            mesh.position.sub(center);
            camera.position.set(0, 0, maxDim * 2.5);
            initialCameraPos.copy(camera.position);
            camera.near = maxDim * 0.0005;
            camera.far = maxDim * 200;
            camera.updateProjectionMatrix();

            console.log('[3D Viewer] Camera position:', camera.position);

            scaleSlider.oninput = (e) => {{
                const scale = parseFloat(e.target.value);
                scaleValue.textContent = scale.toFixed(1) + 'x';

                if (mesh) {{
                    if (mesh instanceof THREE.Points) {{
                        mesh.material.size = 0.1 * scale;
                    }}
                }}
            }};

            resetBtn.onclick = () => {{
                camera.position.copy(initialCameraPos);
                controls.reset();
                rotationVelocity.set(0, 0, 0);
            }};

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

            const origClose = closeBtn.onclick;
            closeBtn.onclick = () => {{ ro.disconnect(); renderer.dispose(); origClose(); }};

            const animate = () => {{
                if (!document.contains(overlay)) {{ ro.disconnect(); renderer.dispose(); return; }}
                requestAnimationFrame(animate);

                // Apply inertia rotation when not actively rotating
                if (!isRotating && (Math.abs(rotationVelocity.x) > inertiaThreshold || Math.abs(rotationVelocity.y) > inertiaThreshold)) {{
                    // Apply rotation using quaternions for smooth momentum
                    const qx = new THREE.Quaternion();
                    const qy = new THREE.Quaternion();
                    const axis = new THREE.Vector3(0, 1, 0);
                    const perpAxis = new THREE.Vector3(1, 0, 0);

                    qy.setFromAxisAngle(axis, rotationVelocity.y);
                    qx.setFromAxisAngle(perpAxis, rotationVelocity.x);

                    const combinedQ = new THREE.Quaternion();
                    combinedQ.multiplyQuaternions(qy, qx);

                    // Apply rotation to camera position
                    const camPos = camera.position;
                    camPos.applyQuaternion(combinedQ);

                    // Decay velocity
                    rotationVelocity.multiplyScalar(inertiaFriction);
                }}

                controls.update();
                renderer.render(scene, camera);
            }};
            console.log('[3D Viewer] Starting animation loop...');
            animate();
        }}, undefined, (err) => {{
            console.error('[3D Viewer] Error loading PLY:', err);
            loading.style.color = '#f85149';
            loading.textContent = 'Error: ' + (err.message || 'Failed to load PLY');
        }});
    }} catch (err) {{
        console.error('[3D Viewer] Error:', err.stack || err);
        const loading = document.getElementById('ply-viewer-overlay-loading');
        if (loading) {{
            loading.style.color = '#f85149';
            loading.textContent = 'Error: ' + (err.message || 'Failed to initialize');
        }}
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
