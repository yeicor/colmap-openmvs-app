// 3D GLB viewer — bundled by esbuild at dx build time into a single self-contained ESM.
// All three.js dependencies (core + GLTFLoader + TrackballControls) are inlined;
// the output module has no external imports and can be loaded via a plain dynamic import().
import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { TrackballControls } from 'three/addons/controls/TrackballControls.js';

/**
 * Launch a full-screen overlay 3D viewer for a GLB file given as a base64 string.
 * @param {string} b64  - Base64-encoded GLB data
 * @param {string} fname - Display filename
 */
export async function launchGlbViewer(b64, fname) {
    console.log('[3D Viewer] Starting viewer setup...');
    try {
        console.log('[3D Viewer] Libraries loaded');

        // Decode GLB bytes and create a blob URL
        const binary = atob(b64);
        const arr = new Uint8Array(binary.length);
        for (let i = 0; i < binary.length; i++) arr[i] = binary.charCodeAt(i);
        const blob = new Blob([arr], {type: 'model/gltf-binary'});
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
        titleSpan.textContent = '3D Viewer \u2014 ' + fname;
        const closeBtn = document.createElement('button');
        closeBtn.textContent = '\u2715 Close';
        closeBtn.style.cssText = 'padding:6px 14px;background:#21262d;color:#e6edf3;border:1px solid #30363d;border-radius:6px;cursor:pointer;font-size:13px;flex-shrink:0;';
        closeBtn.onclick = () => { URL.revokeObjectURL(blobUrl); overlay.remove(); };
        headerDiv.appendChild(titleSpan);
        headerDiv.appendChild(closeBtn);
        overlay.appendChild(headerDiv);

        // Controls bar — reset button always present; dynamic slider added after model loads
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

        const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, preserveDrawingBuffer: true });
        renderer.setSize(w, h, false);
        renderer.setPixelRatio(window.devicePixelRatio || 1);
        renderer.setClearColor(0x0d1117);
        if (THREE.SRGBColorSpace) renderer.outputColorSpace = THREE.SRGBColorSpace;

        const controls = new TrackballControls(camera, renderer.domElement);
        controls.rotateSpeed = 2.5; controls.zoomSpeed = 1.2; controls.panSpeed = 0.8;

        // Inertia
        let rotVel = new THREE.Vector3();
        let isRotating = false, lastX = 0, lastY = 0;
        renderer.domElement.addEventListener('mousedown', () => { isRotating = true; rotVel.set(0,0,0); });
        renderer.domElement.addEventListener('mouseup',   () => { isRotating = false; });
        renderer.domElement.addEventListener('mousemove', e => {
            if (isRotating) { rotVel.x = (e.clientY-lastY)*0.001; rotVel.y = (e.clientX-lastX)*0.001; }
            lastX = e.clientX; lastY = e.clientY;
        });

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

        loader.load(blobUrl, (gltf) => {
            try {
                loadingDiv.remove();
                const model = gltf.scene;

                // Post-process meshes and detect point clouds
                let hasPoints = false;
                let hasMesh = false;
                model.traverse(child => {
                    if (child.isPoints) {
                        hasPoints = true;
                        const hasVCol = !!child.geometry.attributes.color;
                        child.material = new THREE.PointsMaterial({
                            vertexColors: hasVCol,
                            color: hasVCol ? 0xffffff : 0x4fc3f7,
                            size: 0.1,
                            sizeAttenuation: true,
                        });
                    }
                    if (child.isMesh) {
                        hasMesh = true;
                        child.material.side = THREE.DoubleSide;
                    }
                });

                scene.add(model);
                console.log('[3D Viewer] Model added \u2014 hasPoints:', hasPoints, 'hasMesh:', hasMesh);

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

                // Dynamic slider based on content type
                if (hasPoints) {
                    const scaleLabel = document.createElement('label');
                    scaleLabel.style.cssText = 'color:#e6edf3;font-family:monospace;font-size:12px;display:flex;align-items:center;gap:8px;';
                    scaleLabel.textContent = 'Point Scale:';
                    const scaleSlider = document.createElement('input');
                    scaleSlider.type='range'; scaleSlider.min='0.1'; scaleSlider.max='5'; scaleSlider.step='0.1'; scaleSlider.value='1';
                    scaleSlider.style.cssText = 'width:120px;cursor:pointer;';
                    const scaleVal = document.createElement('span');
                    scaleVal.style.cssText = 'color:#8b949e;font-family:monospace;font-size:12px;min-width:30px;';
                    scaleVal.textContent = '1.0x';
                    scaleSlider.oninput = e => {
                        const s = parseFloat(e.target.value);
                        scaleVal.textContent = s.toFixed(1)+'x';
                        model.traverse(c => { if (c.isPoints) c.material.size = 0.1 * s; });
                    };
                    scaleLabel.appendChild(scaleSlider);
                    scaleLabel.appendChild(scaleVal);
                    controlsDiv.insertBefore(scaleLabel, resetBtn);
                } else {
                    const yawLabel = document.createElement('label');
                    yawLabel.style.cssText = 'color:#e6edf3;font-family:monospace;font-size:12px;display:flex;align-items:center;gap:8px;';
                    yawLabel.textContent = 'Light Angle:';
                    const yawSlider = document.createElement('input');
                    yawSlider.type='range'; yawSlider.min='-180'; yawSlider.max='180'; yawSlider.step='5'; yawSlider.value='0';
                    yawSlider.style.cssText = 'width:120px;cursor:pointer;';
                    const yawVal = document.createElement('span');
                    yawVal.style.cssText = 'color:#8b949e;font-family:monospace;font-size:12px;min-width:36px;';
                    yawVal.textContent = '0\u00b0';
                    yawSlider.oninput = e => {
                        const yaw = parseFloat(e.target.value) * Math.PI / 180;
                        yawVal.textContent = e.target.value + '\u00b0';
                        dl.position.set(Math.sin(yaw), 0.5, Math.cos(yaw));
                    };
                    yawLabel.appendChild(yawSlider);
                    yawLabel.appendChild(yawVal);
                    controlsDiv.insertBefore(yawLabel, resetBtn);
                }

                resetBtn.onclick = () => {
                    camera.position.copy(initialCamPos);
                    controls.reset();
                    rotVel.set(0,0,0);
                };

                const ro = new ResizeObserver(() => {
                    const nw = canvas.clientWidth, nh = canvas.clientHeight;
                    if (nw > 0 && nh > 0) {
                        camera.aspect = nw / nh;
                        camera.updateProjectionMatrix();
                        renderer.setSize(nw, nh, false);
                    }
                });
                ro.observe(canvas);

                const origClose = closeBtn.onclick;
                closeBtn.onclick = () => { URL.revokeObjectURL(blobUrl); ro.disconnect(); renderer.dispose(); origClose(); };

                const animate = () => {
                    if (!document.contains(overlay)) { URL.revokeObjectURL(blobUrl); ro.disconnect(); renderer.dispose(); return; }
                    requestAnimationFrame(animate);
                    if (!isRotating && (Math.abs(rotVel.x) > 0.001 || Math.abs(rotVel.y) > 0.001)) {
                        const qy = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0,1,0), rotVel.y);
                        const qx = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(1,0,0), rotVel.x);
                        camera.position.applyQuaternion(new THREE.Quaternion().multiplyQuaternions(qy, qx));
                        rotVel.multiplyScalar(0.95);
                    }
                    controls.update();
                    renderer.render(scene, camera);
                };
                animate();
            } catch (innerErr) {
                console.error('[3D Viewer] Error processing model:', innerErr);
                const ld = document.getElementById('ply-viewer-overlay-loading');
                if (ld) { ld.style.color='#f85149'; ld.textContent='Error: '+(innerErr.message||'Failed to process model'); }
            }
        }, undefined, err => {
            console.error('[3D Viewer] Error loading GLB:', err);
            const ld = document.getElementById('ply-viewer-overlay-loading');
            if (ld) { ld.style.color='#f85149'; ld.textContent='Error: '+(err.message||'Failed to load model'); }
        });

    } catch (err) {
        console.error('[3D Viewer] Fatal error:', err.stack || err);
        const ld = document.getElementById('ply-viewer-overlay-loading');
        if (ld) { ld.style.color='#f85149'; ld.textContent='Error: '+(err.message||'Failed to initialize'); }
    }
}
