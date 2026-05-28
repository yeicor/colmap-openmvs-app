// viewer3d.js
// Full-featured photogrammetry-oriented GLB viewer
// Responsive/mobile-friendly
// API preserved:
//   launchGlbViewer(b64, fname)

import * as THREE from "three";

import { GLTFLoader } from "three/addons/loaders/GLTFLoader.js";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";

import { EffectComposer } from "three/addons/postprocessing/EffectComposer.js";
import { RenderPass } from "three/addons/postprocessing/RenderPass.js";
import { ShaderPass } from "three/addons/postprocessing/ShaderPass.js";

import { FXAAShader } from "three/addons/shaders/FXAAShader.js";

const THEME = {
  bg: "#0d1117",
  panel: "#161b22",
  border: "#30363d",
  text: "#e6edf3",
  muted: "#8b949e",
  accent: "#58a6ff",
};

const MOBILE_BREAKPOINT = 820;

const STATE = {
  renderMode: "textured",
  pointMode: "rgb",

  pointSize: 1.0,

  showMesh: true,
  showPoints: true,
  showWireframe: false,

  clipping: false,

  measurementMode: false,

  selected: null,
};

export async function launchGlbViewer(b64, fname) {
  // Cleanup existing viewer

  const existing = document.getElementById("pg-viewer-overlay");

  if (existing) existing.remove();

  // Decode GLB

  const binary = atob(b64);

  const arr = new Uint8Array(binary.length);

  for (let i = 0; i < binary.length; i++) {
    arr[i] = binary.charCodeAt(i);
  }

  const blob = new Blob([arr], {
    type: "model/gltf-binary",
  });

  const blobUrl = URL.createObjectURL(blob);

  // Layout

  const ui = createLayout(fname);

  document.body.appendChild(ui.root);

  // Scene

  const scene = new THREE.Scene();

  scene.background = new THREE.Color(THEME.bg);

  // Camera

  const camera = new THREE.PerspectiveCamera(60, 1, 0.001, 100000);

  camera.position.set(0, 0, 5);

  // Renderer

  const renderer = new THREE.WebGLRenderer({
    canvas: ui.canvas,
    antialias: true,
    preserveDrawingBuffer: true,
  });

  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));

  renderer.setClearColor(0x0d1117);

  renderer.outputColorSpace = THREE.SRGBColorSpace;

  renderer.localClippingEnabled = true;

  // Composer

  const composer = new EffectComposer(renderer);

  composer.addPass(new RenderPass(scene, camera));

  const fxaa = new ShaderPass(FXAAShader);

  composer.addPass(fxaa);

  // Controls

  const controls = new OrbitControls(camera, renderer.domElement);

  controls.enableDamping = true;
  controls.dampingFactor = 0.08;
  controls.screenSpacePanning = true;
  controls.rotateSpeed = 0.9;
  controls.zoomSpeed = 1.0;
  controls.panSpeed = 1.0;

  // Lights

  setupLights(scene, camera);

  // Resize

  function resize() {
    const w = ui.viewport.clientWidth;
    const h = ui.viewport.clientHeight;

    camera.aspect = w / h;

    camera.updateProjectionMatrix();

    renderer.setSize(w, h, false);

    fxaa.material.uniforms.resolution.value.set(1 / w, 1 / h);
  }

  const ro = new ResizeObserver(resize);

  ro.observe(ui.viewport);

  resize();

  // GLTF Load

  const loader = new GLTFLoader();

  ui.status("Loading model...");

  loader.load(
    blobUrl,
    (gltf) => {
      const root = gltf.scene;

      scene.add(root);

      const analysis = processScene(root);

      fitCamera(camera, controls, root);

      buildSidebar(ui, analysis, root);

      buildToolbar(ui, scene, root, renderer, camera, controls);

      setupPicking(renderer, scene, camera, ui);

      setupMeasurements(renderer, scene, camera, ui);

      ui.status("");

      animate();
    },
    undefined,
    (err) => {
      ui.status("Load failed: " + err.message, true);

      console.error(err);
    },
  );

  function animate() {
    if (!document.contains(ui.root)) {
      renderer.dispose();

      ro.disconnect();

      URL.revokeObjectURL(blobUrl);

      return;
    }

    requestAnimationFrame(animate);

    controls.update();

    composer.render();
  }
}

function createLayout(fname) {
  const mobile = window.innerWidth <= MOBILE_BREAKPOINT;

  const root = document.createElement("div");

  root.id = "pg-viewer-overlay";

  root.style.cssText = `
        position:fixed;
        inset:0;
        z-index:999999;
        background:${THEME.bg};
        display:flex;
        flex-direction:column;
        overflow:hidden;
        touch-action:none;
    `;

  // Toolbar

  const toolbar = document.createElement("div");

  toolbar.style.cssText = `
        height:48px;
        flex-shrink:0;
        background:${THEME.panel};
        border-bottom:1px solid ${THEME.border};

        display:flex;
        align-items:center;
        gap:6px;

        padding:0 10px;
        overflow-x:auto;
        overflow-y:hidden;
    `;

  const title = document.createElement("div");

  title.textContent = fname;

  title.style.cssText = `
        color:${THEME.text};
        font-size:12px;
        font-family:monospace;
        white-space:nowrap;
        overflow:hidden;
        text-overflow:ellipsis;
        min-width:120px;
        max-width:240px;
        flex-shrink:0;
    `;

  toolbar.appendChild(title);

  // Body

  const body = document.createElement("div");

  body.style.cssText = `
        flex:1;
        position:relative;
        display:flex;
        min-height:0;
    `;

  // Sidebar

  const sidebar = document.createElement("div");

  sidebar.style.cssText = mobile
    ? `
            position:absolute;
            left:0;
            right:0;
            bottom:0;
            height:38%;
            z-index:10;

            background:${THEME.panel};
            border-top:1px solid ${THEME.border};

            overflow:auto;

            backdrop-filter:blur(10px);
        `
    : `
            width:300px;
            flex-shrink:0;

            background:${THEME.panel};
            border-right:1px solid ${THEME.border};

            overflow:auto;
        `;

  // Viewport

  const viewport = document.createElement("div");

  viewport.style.cssText = `
        flex:1;
        min-width:0;
        position:relative;
    `;

  const canvas = document.createElement("canvas");

  canvas.style.cssText = `
        width:100%;
        height:100%;
        display:block;
    `;

  viewport.appendChild(canvas);

  body.appendChild(sidebar);

  body.appendChild(viewport);

  // Status bar

  const status = document.createElement("div");

  status.style.cssText = `
        height:24px;
        flex-shrink:0;

        background:${THEME.panel};
        border-top:1px solid ${THEME.border};

        display:flex;
        align-items:center;

        padding:0 10px;

        color:${THEME.muted};

        font-size:11px;
        font-family:monospace;
    `;

  root.appendChild(toolbar);
  root.appendChild(body);
  root.appendChild(status);

  return {
    root,
    toolbar,
    body,
    sidebar,
    viewport,
    canvas,

    status(msg, err = false) {
      status.textContent = msg;

      status.style.color = err ? "#ff7b72" : THEME.muted;
    },
  };
}

function createButton(icon, tooltip) {
  const btn = document.createElement("button");

  btn.innerHTML = icon;

  btn.title = tooltip;

  btn.style.cssText = `
        width:32px;
        height:32px;

        border-radius:7px;

        border:1px solid ${THEME.border};

        background:#21262d;

        color:${THEME.text};

        cursor:pointer;

        flex-shrink:0;

        font-size:14px;

        transition:all .15s ease;
    `;

  btn.onmouseenter = () => {
    btn.style.borderColor = THEME.accent;
  };

  btn.onmouseleave = () => {
    btn.style.borderColor = THEME.border;
  };

  return btn;
}

function setupLights(scene, camera) {
  scene.add(new THREE.AmbientLight(0xffffff, 0.65));

  const dir = new THREE.DirectionalLight(0xffffff, 1.2);

  dir.position.set(1, 2, 3);

  camera.add(dir);

  scene.add(camera);
}

function processScene(root) {
  let meshCount = 0;
  let pointCount = 0;
  let triCount = 0;

  const bbox = new THREE.Box3().setFromObject(root);

  root.traverse((obj) => {
    if (obj.isMesh) {
      meshCount++;

      obj.userData.originalMaterial = obj.material;

      obj.material.side = THREE.DoubleSide;

      obj.material.depthWrite = true;

      if (obj.geometry.index) {
        triCount += obj.geometry.index.count / 3;
      }
    }

    if (obj.isPoints) {
      pointCount += obj.geometry.attributes.position.count;

      const hasColors = !!obj.geometry.attributes.color;

      obj.material = new THREE.PointsMaterial({
        size: 0.02,
        sizeAttenuation: true,
        vertexColors: hasColors,
        color: hasColors ? 0xffffff : 0x58a6ff,
      });
    }
  });

  return {
    meshCount,
    pointCount,
    triCount,
    bbox,
  };
}

function fitCamera(camera, controls, root) {
  const box = new THREE.Box3().setFromObject(root);

  const center = box.getCenter(new THREE.Vector3());

  const size = box.getSize(new THREE.Vector3());

  const maxDim = Math.max(size.x, size.y, size.z);

  camera.position.set(center.x, center.y, center.z + maxDim * 2);

  camera.near = maxDim * 0.0001;
  camera.far = maxDim * 1000;

  camera.updateProjectionMatrix();

  controls.target.copy(center);

  controls.update();
}

function buildToolbar(ui, scene, root, renderer, camera, controls) {
  // Render mode

  const textured = createButton("🧊", "Textured");

  textured.onclick = () => applyRenderMode(root, "textured");

  ui.toolbar.appendChild(textured);

  const wireframe = createButton("🕸", "Wireframe");

  wireframe.onclick = () => applyRenderMode(root, "wireframe");

  ui.toolbar.appendChild(wireframe);

  const normals = createButton("📏", "Normals");

  normals.onclick = () => applyRenderMode(root, "normals");

  ui.toolbar.appendChild(normals);

  const flat = createButton("⬛", "Flat");

  flat.onclick = () => applyRenderMode(root, "flat");

  ui.toolbar.appendChild(flat);

  const xray = createButton("👁", "X-Ray");

  xray.onclick = () => applyRenderMode(root, "xray");

  ui.toolbar.appendChild(xray);

  // Point coloring

  const height = createButton("🌈", "Height Colors");

  height.onclick = () => applyPointColorMode(root, "height");

  ui.toolbar.appendChild(height);

  const density = createButton("☁", "Density");

  density.onclick = () => applyPointColorMode(root, "density");

  ui.toolbar.appendChild(density);

  // Clipping

  const clip = createButton("✂", "Clipping");

  clip.onclick = () => enableClipping(renderer, root);

  ui.toolbar.appendChild(clip);

  // Measure

  const measure = createButton("📐", "Measure");

  measure.onclick = () => {
    STATE.measurementMode = !STATE.measurementMode;

    ui.status(STATE.measurementMode ? "Measurement enabled" : "Measurement disabled");
  };

  ui.toolbar.appendChild(measure);

  // Screenshot

  const shot = createButton("📸", "Screenshot");

  shot.onclick = () => saveScreenshot(renderer);

  ui.toolbar.appendChild(shot);

  // Top

  const top = createButton("⬆", "Top View");

  top.onclick = () => setView(camera, controls, "top");

  ui.toolbar.appendChild(top);

  // Front

  const front = createButton("⬜", "Front View");

  front.onclick = () => setView(camera, controls, "front");

  ui.toolbar.appendChild(front);

  // Side

  const side = createButton("➡", "Side View");

  side.onclick = () => setView(camera, controls, "side");

  ui.toolbar.appendChild(side);

  // Reset

  const reset = createButton("⟳", "Reset");

  reset.onclick = () => fitCamera(camera, controls, root);

  ui.toolbar.appendChild(reset);

  // Close

  const close = createButton("✕", "Close");

  close.onclick = () => ui.root.remove();

  ui.toolbar.appendChild(close);
}

function applyRenderMode(root, mode) {
  root.traverse((obj) => {
    if (!obj.isMesh) return;

    switch (mode) {
      case "textured":
        obj.material = obj.userData.originalMaterial;

        obj.material.wireframe = false;

        obj.material.transparent = false;

        break;

      case "wireframe":
        obj.material = obj.userData.originalMaterial;

        obj.material.wireframe = true;

        break;

      case "normals":
        obj.material = new THREE.MeshNormalMaterial();

        break;

      case "flat":
        obj.material = new THREE.MeshStandardMaterial({
          color: 0xcccccc,
          flatShading: true,
        });

        break;

      case "xray":
        obj.material = new THREE.MeshBasicMaterial({
          color: 0xffffff,
          transparent: true,
          opacity: 0.25,
        });

        break;
    }
  });
}

function applyPointColorMode(root, mode) {
  root.traverse((obj) => {
    if (!obj.isPoints) return;

    const pos = obj.geometry.attributes.position;

    const count = pos.count;

    let colors = obj.geometry.attributes.color;

    if (!colors) {
      colors = new THREE.BufferAttribute(new Float32Array(count * 3), 3);

      obj.geometry.setAttribute("color", colors);
    }

    const arr = colors.array;

    if (mode === "height") {
      let min = Infinity;
      let max = -Infinity;

      for (let i = 0; i < count; i++) {
        const y = pos.getY(i);

        min = Math.min(min, y);
        max = Math.max(max, y);
      }

      for (let i = 0; i < count; i++) {
        const y = pos.getY(i);

        const t = (y - min) / (max - min);

        arr[i * 3 + 0] = t;
        arr[i * 3 + 1] = 0.2;
        arr[i * 3 + 2] = 1.0 - t;
      }
    }

    if (mode === "density") {
      for (let i = 0; i < count; i++) {
        const r = Math.random();

        arr[i * 3 + 0] = r;
        arr[i * 3 + 1] = 0.5;
        arr[i * 3 + 2] = 1.0 - r;
      }
    }

    colors.needsUpdate = true;
  });
}

function enableClipping(renderer, root) {
  const plane = new THREE.Plane(new THREE.Vector3(0, -1, 0), 0);

  renderer.clippingPlanes = [plane];

  let offset = 0;

  const onWheel = (e) => {
    offset += e.deltaY * 0.001;

    plane.constant = offset;
  };

  window.addEventListener("wheel", onWheel, { passive: true });

  setTimeout(() => {
    window.removeEventListener("wheel", onWheel);

    renderer.clippingPlanes = [];
  }, 10000);
}

function buildSidebar(ui, analysis, root) {
  ui.sidebar.innerHTML = "";

  addSection(
    ui.sidebar,
    "Scene",
    `
        Meshes: ${analysis.meshCount}<br>
        Points: ${analysis.pointCount.toLocaleString()}<br>
        Triangles: ${analysis.triCount.toLocaleString()}
        `,
  );

  const size = analysis.bbox.getSize(new THREE.Vector3());

  addSection(
    ui.sidebar,
    "Bounds",
    `
        X: ${size.x.toFixed(2)}<br>
        Y: ${size.y.toFixed(2)}<br>
        Z: ${size.z.toFixed(2)}
        `,
  );

  addSection(
    ui.sidebar,
    "Controls",
    `
        Orbit: Left Mouse<br>
        Pan: Right Mouse<br>
        Zoom: Wheel / Pinch
        `,
  );

  // Point size slider

  const section = document.createElement("div");

  section.style.cssText = `
        padding:12px;
        border-bottom:1px solid ${THEME.border};
    `;

  const title = document.createElement("div");

  title.textContent = "Point Size";

  title.style.cssText = `
        color:${THEME.text};
        font-size:13px;
        margin-bottom:8px;
        font-weight:600;
    `;

  const slider = document.createElement("input");

  slider.type = "range";

  slider.min = "0.1";
  slider.max = "10";
  slider.step = "0.1";
  slider.value = "1";

  slider.style.width = "100%";

  slider.oninput = (e) => {
    const s = parseFloat(e.target.value);

    root.traverse((obj) => {
      if (!obj.isPoints) return;

      obj.material.size = 0.02 * s;
    });
  };

  section.appendChild(title);
  section.appendChild(slider);

  ui.sidebar.appendChild(section);
}

function addSection(parent, title, html) {
  const section = document.createElement("div");

  section.style.cssText = `
        padding:12px;
        border-bottom:1px solid ${THEME.border};
    `;

  const h = document.createElement("div");

  h.textContent = title;

  h.style.cssText = `
        color:${THEME.text};
        font-size:13px;
        margin-bottom:8px;
        font-weight:600;
    `;

  const body = document.createElement("div");

  body.innerHTML = html;

  body.style.cssText = `
        color:${THEME.muted};
        font-size:12px;
        line-height:1.6;
        font-family:monospace;
    `;

  section.appendChild(h);
  section.appendChild(body);

  parent.appendChild(section);
}

function setupPicking(renderer, scene, camera, ui) {
  const raycaster = new THREE.Raycaster();

  const mouse = new THREE.Vector2();

  renderer.domElement.addEventListener("click", (e) => {
    const rect = renderer.domElement.getBoundingClientRect();

    mouse.x = ((e.clientX - rect.left) / rect.width) * 2 - 1;

    mouse.y = -((e.clientY - rect.top) / rect.height) * 2 + 1;

    raycaster.setFromCamera(mouse, camera);

    const hits = raycaster.intersectObjects(scene.children, true);

    if (!hits.length) return;

    const hit = hits[0];

    ui.status(`${hit.object.type} @ ${hit.point.x.toFixed(2)}, ${hit.point.y.toFixed(2)}, ${hit.point.z.toFixed(2)}`);
  });
}

function setupMeasurements(renderer, scene, camera, ui) {
  const points = [];

  const raycaster = new THREE.Raycaster();

  const mouse = new THREE.Vector2();

  renderer.domElement.addEventListener("dblclick", (e) => {
    if (!STATE.measurementMode) return;

    const rect = renderer.domElement.getBoundingClientRect();

    mouse.x = ((e.clientX - rect.left) / rect.width) * 2 - 1;

    mouse.y = -((e.clientY - rect.top) / rect.height) * 2 + 1;

    raycaster.setFromCamera(mouse, camera);

    const hits = raycaster.intersectObjects(scene.children, true);

    if (!hits.length) return;

    points.push(hits[0].point.clone());

    if (points.length < 2) return;

    const a = points[0];
    const b = points[1];

    const dist = a.distanceTo(b);

    const geo = new THREE.BufferGeometry().setFromPoints([a, b]);

    const line = new THREE.Line(
      geo,
      new THREE.LineBasicMaterial({
        color: 0x58a6ff,
      }),
    );

    scene.add(line);

    ui.status(`Distance: ${dist.toFixed(4)}`);

    points.length = 0;
  });
}

function setView(camera, controls, mode) {
  const d = camera.position.distanceTo(controls.target);

  switch (mode) {
    case "top":
      camera.position.set(0, d, 0.001);

      break;

    case "front":
      camera.position.set(0, 0, d);

      break;

    case "side":
      camera.position.set(d, 0, 0);

      break;
  }

  camera.lookAt(controls.target);
}

function saveScreenshot(renderer) {
  const a = document.createElement("a");

  a.download = "viewer_screenshot.png";

  a.href = renderer.domElement.toDataURL("image/png");

  a.click();
}
