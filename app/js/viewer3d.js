/* viewer3d.final.js
 *
 * Production-ready 3D viewer overhaul:
 * - Orbit-style camera (spherical coordinates, fixed up-vector)
 * - Dynamic capability-aware toolbar
 * - Popover-based UX replacing bottom panel
 * - Dismissible measurement / pick cards
 * - Robust interaction cleanup
 * - Mesh / pointcloud adaptive rendering
 * - Persistent viewer state
 * - Improved accessibility and shortcuts
 */

import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";

const STORAGE_KEY = "viewer3d.preferences.v3";

const DEFAULT_STATE = {
  background: "#111318",
  showGrid: true,
  showAxes: false,
  wireframe: false,
  backfaces: false,
  raytracing: false,
  lighting: true,
  lightingOffset: 0,
  pointsSize: 1.5,
  toneMapping: true,
  measurementVisible: true,
  inertia: true,
  exposure: 1.0,
  renderMode: "solid",
};

function loadPrefs() {
  try {
    const parsed = JSON.parse(localStorage.getItem(STORAGE_KEY));
    return { ...DEFAULT_STATE, ...(parsed || {}) };
  } catch {
    return { ...DEFAULT_STATE };
  }
}

function savePrefs(state) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  } catch {}
}

function clamp(v, a, b) {
  return Math.max(a, Math.min(b, v));
}

function distance(a, b) {
  return a.distanceTo(b);
}

function createButton(icon, label, onClick) {
  const btn = document.createElement("button");
  btn.className = "v3d-btn";
  btn.type = "button";
  btn.title = label;
  btn.setAttribute("aria-label", label);
  btn.innerHTML = `
        <span class="v3d-btn-icon">${icon}</span>
        <span class="v3d-btn-label">${label}</span>
    `;
  btn.addEventListener("click", onClick);
  return btn;
}

function createPopover(anchor, title, contentNode) {
  const pop = document.createElement("div");
  pop.className = "v3d-popover hidden";
  pop.innerHTML = `
        <div class="v3d-popover-header">
            <div class="v3d-popover-title">${title}</div>
            <button class="v3d-close">×</button>
        </div>
    `;
  pop.appendChild(contentNode);
  document.body.appendChild(pop);

  const closeBtn = pop.querySelector(".v3d-close");

  function position() {
    const r = anchor.getBoundingClientRect();
    pop.style.left = `${r.left}px`;
    pop.style.top = `${r.bottom + 10}px`;
  }

  function open() {
    position();
    pop.classList.remove("hidden");
  }

  function close() {
    pop.classList.add("hidden");
  }

  anchor.addEventListener("click", (e) => {
    e.stopPropagation();
    if (pop.classList.contains("hidden")) {
      closeAllPopovers();
      open();
    } else {
      close();
    }
  });

  closeBtn.addEventListener("click", close);

  function outside(e) {
    if (!pop.contains(e.target) && !anchor.contains(e.target)) {
      close();
    }
  }

  function keyHandler(e) {
    if (e.key === "Escape") {
      close();
    }
  }

  document.addEventListener("pointerdown", outside);
  document.addEventListener("keydown", keyHandler);

  registerPopover(close);
  return pop;
}

const activePopovers = [];

function registerPopover(closeFn) {
  activePopovers.push(closeFn);
}

function closeAllPopovers() {
  activePopovers.forEach((f) => f());
}

class ToastStack {
  constructor(root) {
    this.root = root;
    this.el = document.createElement("div");
    this.el.className = "v3d-toast-stack";
    root.appendChild(this.el);
  }

  push(title, html) {
    const card = document.createElement("div");
    card.className = "v3d-toast";
    card.innerHTML = `
            <div class="v3d-toast-header">
                <div>${title}</div>
                <button>×</button>
            </div>
            <div class="v3d-toast-content"></div>
        `;
    card.querySelector(".v3d-toast-content").innerHTML = html;
    card.querySelector("button").addEventListener("click", () => {
      card.remove();
    });
    this.el.appendChild(card);
    return card;
  }
}

// ── Orbit controls (spherical coordinates, fixed up-vector) ──────────────

class OrbitControls {
  constructor(camera, domElement) {
    this.camera = camera;
    this.domElement = domElement;

    this.enabled = true;

    this.rotateSpeed = 0.006;
    this.zoomSpeed = 1.0;
    this.panSpeed = 0.5;

    this.target = new THREE.Vector3();

    this.minDistance = 0.001;
    this.maxDistance = Infinity;
    this.minPolarAngle = 0;
    this.maxPolarAngle = Math.PI;

    this.state = "none";
    this.last = new THREE.Vector2();

    this.spherical = new THREE.Spherical();
    this.sphericalDelta = new THREE.Spherical();
    this.panOffset = new THREE.Vector3();

    this.inertia = true;
    this.velocityTheta = 0;
    this.velocityPhi = 0;
    this.velocityPan = new THREE.Vector3();

    this.bind();
  }

  bind() {
    this.domElement.addEventListener("pointerdown", this.onPointerDown);
    window.addEventListener("pointermove", this.onPointerMove);
    window.addEventListener("pointerup", this.onPointerUp);
    this.domElement.addEventListener("wheel", this.onWheel, { passive: false });
  }

  dispose() {
    this.domElement.removeEventListener("pointerdown", this.onPointerDown);
    window.removeEventListener("pointermove", this.onPointerMove);
    window.removeEventListener("pointerup", this.onPointerUp);
    this.domElement.removeEventListener("wheel", this.onWheel);
  }

  saveSpherical() {
    const offset = new THREE.Vector3().copy(this.camera.position).sub(this.target);
    this.spherical.setFromVector3(offset);
  }

  applySpherical() {
    const offset = new THREE.Vector3().setFromSpherical(this.spherical);
    this.camera.position.copy(this.target).add(offset);
    this.camera.lookAt(this.target);
  }

  onPointerDown = (e) => {
    if (!this.enabled) return;
    this.last.set(e.clientX, e.clientY);

    if (e.button === 0) {
      this.state = "rotate";
    } else if (e.button === 1 || e.button === 2) {
      this.state = "pan";
    }
  };

  onPointerMove = (e) => {
    if (!this.enabled) return;
    if (this.state === "none") return;

    const dx = e.clientX - this.last.x;
    const dy = e.clientY - this.last.y;
    this.last.set(e.clientX, e.clientY);

    if (this.state === "rotate") {
      this.rotate(dx, dy);
    } else if (this.state === "pan") {
      this.pan(dx, dy);
    }
  };

  onPointerUp = () => {
    this.state = "none";
  };

  onWheel = (e) => {
    e.preventDefault();
    const factor = e.deltaY > 0 ? 1.05 : 0.95;
    this.zoom(factor);
  };

  rotate(dx, dy) {
    const theta = -dx * this.rotateSpeed;
    const phi = -dy * this.rotateSpeed;

    this.sphericalDelta.theta += theta;
    this.sphericalDelta.phi += phi;

    this.velocityTheta += theta * 0.3;
    this.velocityPhi += phi * 0.3;
  }

  pan(dx, dy) {
    const offset = new THREE.Vector3().copy(this.camera.position).sub(this.target);
    const dist = offset.length();
    const factor = (2 * dist) / this.domElement.clientHeight;

    const right = new THREE.Vector3();
    right.crossVectors(this.camera.up, offset.normalize()).normalize();

    const pan = new THREE.Vector3()
      .copy(right).multiplyScalar(-dx * factor * this.panSpeed)
      .add(new THREE.Vector3().copy(this.camera.up).multiplyScalar(dy * factor * this.panSpeed));

    this.panOffset.add(pan);
    this.velocityPan.copy(pan);
  }

  zoom(scale) {
    const offset = new THREE.Vector3().copy(this.camera.position).sub(this.target);
    const len = Math.max(this.minDistance, Math.min(this.maxDistance, offset.length() * scale));
    offset.setLength(len);
    this.camera.position.copy(this.target).add(offset);
    this.camera.lookAt(this.target);

    this.saveSpherical();
  }

  update() {
    if (!this.enabled) return;

    let needsUpdate = false;

    if (this.state === "none") {
      if (this.inertia) {
        if (Math.abs(this.velocityTheta) > 1e-6 || Math.abs(this.velocityPhi) > 1e-6) {
          this.sphericalDelta.theta += this.velocityTheta;
          this.sphericalDelta.phi += this.velocityPhi;
          this.velocityTheta *= 0.9;
          this.velocityPhi *= 0.9;
          needsUpdate = true;
        }

        if (this.velocityPan.lengthSq() > 1e-8) {
          this.panOffset.add(this.velocityPan);
          this.velocityPan.multiplyScalar(0.9);
          needsUpdate = true;
        }
      }
    } else {
      this.velocityTheta = 0;
      this.velocityPhi = 0;
      this.velocityPan.set(0, 0, 0);
    }

    if (this.sphericalDelta.theta !== 0 || this.sphericalDelta.phi !== 0) {
      this.saveSpherical();

      this.spherical.theta += this.sphericalDelta.theta;
      this.spherical.phi = clamp(this.spherical.phi + this.sphericalDelta.phi, this.minPolarAngle, this.maxPolarAngle);

      this.applySpherical();
      this.sphericalDelta.set(0, 0, 0);
      needsUpdate = true;
    }

    if (this.panOffset.lengthSq() > 0) {
      this.target.add(this.panOffset);
      this.camera.position.add(this.panOffset);
      this.panOffset.set(0, 0, 0);
      needsUpdate = true;
    }

    if (needsUpdate) {
      this.camera.lookAt(this.target);
    }
  }
}

// ── Safe property transfer between material types ─────────────────────────

function copyMaterialProps(src, dst) {
  const props = [
    "alphaMap", "alphaTest", "blendDst", "blendEquation", "blendSrc",
    "blending", "colorWrite", "depthFunc", "depthTest", "depthWrite",
    "name", "opacity", "polygonOffset", "polygonOffsetFactor",
    "polygonOffsetUnits", "premultipliedAlpha", "side", "toneMapped",
    "transparent", "visible", "wireframe",
  ];
  for (const p of props) {
    if (p in src) dst[p] = src[p];
  }
  dst.color.copy(src.color);
  dst.map = src.map;
  if (src.userData) dst.userData = { ...src.userData };
  dst.needsUpdate = true;
}

// ── Main viewer class ────────────────────────────────────────────────────

export class Viewer3D {
  constructor(container, opts = {}) {
    this.container = container;
    this.state = loadPrefs();

    this.scene = new THREE.Scene();
    this.scene.background = new THREE.Color(this.state.background);

    this.renderer = new THREE.WebGLRenderer({
      antialias: true,
      alpha: false,
    });
    this.renderer.setPixelRatio(window.devicePixelRatio);
    this.renderer.setSize(container.clientWidth, container.clientHeight);
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    this.renderer.toneMapping = this.state.toneMapping ? THREE.ACESFilmicToneMapping : THREE.NoToneMapping;
    this.renderer.toneMappingExposure = this.state.exposure;

    const canvas = this.renderer.domElement;
    canvas.style.display = "block";
    container.appendChild(canvas);

    this.camera = new THREE.PerspectiveCamera(60, container.clientWidth / container.clientHeight, 0.01, 10000);
    this.camera.position.set(2, 2, 2);

    this.controls = new OrbitControls(this.camera, canvas);
    this.controls.inertia = this.state.inertia;

    this.timer = new THREE.Timer();

    this.mixers = [];

    this.raycaster = new THREE.Raycaster();
    this.pointer = new THREE.Vector2();

    this.modelRoot = null;

    this.capabilities = {
      mesh: false,
      points: false,
      texture: false,
      normals: false,
      vertexColors: false,
    };

    this.stats = {
      triangles: 0,
      vertices: 0,
      drawCalls: 0,
      materials: 0,
      textures: 0,
    };

    this.measurementMode = false;
    this.measurePoints = [];

    this.toastStack = new ToastStack(container);

    this.createEnvironment();
    this.createToolbar();
    this.injectStyles();
    this.bind();
    this.animate();
  }

  // ── Environment ──────────────────────────────────────────────────────────

  createEnvironment() {
    this.hemiLight = new THREE.HemisphereLight(0xffffff, 0x222233, 1.4);
    this.scene.add(this.hemiLight);

    this.dirLight = new THREE.DirectionalLight(0xffffff, 1.1);
    this.dirLight.position.set(4, 8, 6);
    this.scene.add(this.dirLight);

    this.grid = new THREE.GridHelper(10, 20, 0x444444, 0x222222);
    this.axes = new THREE.AxesHelper(1.5);
    this.grid.visible = this.state.showGrid;
    this.axes.visible = this.state.showAxes;
    this.scene.add(this.grid);
    this.scene.add(this.axes);
  }

  // ── Toolbar ──────────────────────────────────────────────────────────────

  createToolbar() {
    this.toolbar = document.createElement("div");
    this.toolbar.className = "v3d-toolbar";
    this.container.appendChild(this.toolbar);

    const sections = [
      this.makeSection("View"),
      this.makeSection("Render"),
      this.makeSection("Tools"),
      this.makeSection("Info"),
    ];
    sections.forEach((s) => this.toolbar.appendChild(s));

    this.sections = {
      view: sections[0],
      render: sections[1],
      tools: sections[2],
      info: sections[3],
    };

    this.buildButtons();
  }

  makeSection(label) {
    const sec = document.createElement("div");
    sec.className = "v3d-toolbar-section";
    sec.dataset.label = label;
    return sec;
  }

  buildButtons() {
    const homeBtn = createButton("⌂", "Home", () => this.homeCamera(true));

    const gridBtn = createButton("▦", "Grid", () => {
      this.state.showGrid = !this.state.showGrid;
      this.grid.visible = this.state.showGrid;
      savePrefs(this.state);
    });

    const axesBtn = createButton("╋", "Axes", () => {
      this.state.showAxes = !this.state.showAxes;
      this.axes.visible = this.state.showAxes;
      savePrefs(this.state);
    });

    const measureBtn = createButton("📏", "Measure", () => {
      this.measurementMode = !this.measurementMode;
      measureBtn.classList.toggle("active", this.measurementMode);
    });

    const statsBtn = createButton("ℹ", "Stats", () => {});
    const helpBtn = createButton("?", "Help", () => {});

    this.sections.view.append(homeBtn, gridBtn, axesBtn);
    this.sections.tools.append(measureBtn);
    this.sections.info.append(statsBtn, helpBtn);

    // Stats popover
    const statsContent = document.createElement("div");
    statsContent.className = "v3d-popover-content";
    statsContent.innerHTML = `
            <div class="v3d-stat-row">
                <span>Triangles</span>
                <span id="v3d-stat-tris">0</span>
            </div>
            <div class="v3d-stat-row">
                <span>Vertices</span>
                <span id="v3d-stat-verts">0</span>
            </div>
            <div class="v3d-stat-row">
                <span>Materials</span>
                <span id="v3d-stat-mats">0</span>
            </div>
            <div class="v3d-stat-row">
                <span>Textures</span>
                <span id="v3d-stat-tex">0</span>
            </div>
        `;
    this.statsPopover = createPopover(statsBtn, "Scene Statistics", statsContent);

    // Help popover
    const helpContent = document.createElement("div");
    helpContent.className = "v3d-popover-content";
    helpContent.innerHTML = `
            <div class="v3d-help-item">
                <b>Rotate</b>
                <span>Left drag</span>
            </div>
            <div class="v3d-help-item">
                <b>Pan</b>
                <span>Middle / right drag</span>
            </div>
            <div class="v3d-help-item">
                <b>Zoom</b>
                <span>Mouse wheel</span>
            </div>
            <div class="v3d-help-item">
                <b>Measure</b>
                <span>Press <kbd>M</kbd></span>
            </div>
            <div class="v3d-help-item">
                <b>Home</b>
                <span>Press <kbd>F</kbd></span>
            </div>
            <div class="v3d-help-item">
                <b>Grid</b>
                <span>Press <kbd>G</kbd></span>
            </div>
            <div class="v3d-help-item">
                <b>Stats</b>
                <span>Press <kbd>I</kbd></span>
            </div>
            <div class="v3d-help-item">
                <b>Help</b>
                <span>Press <kbd>?</kbd></span>
            </div>
        `;
    this.helpPopover = createPopover(helpBtn, "Controls", helpContent);
  }

  // ── Styles ───────────────────────────────────────────────────────────────

  injectStyles() {
    if (document.getElementById("viewer3d-style")) return;

    const style = document.createElement("style");
    style.id = "viewer3d-style";
    style.textContent = `
            .v3d-toolbar {
                position: absolute; top: 14px; left: 14px;
                display: flex; gap: 10px; z-index: 50;
                flex-wrap: wrap; max-width: calc(100% - 28px);
                pointer-events: none;
            }
            .v3d-toolbar-section {
                display: flex; gap: 6px; padding: 6px;
                border-radius: 14px;
                background: rgba(18,20,26,0.88);
                backdrop-filter: blur(12px);
                border: 1px solid rgba(255,255,255,0.08);
                pointer-events: auto;
                box-shadow: 0 10px 30px rgba(0,0,0,0.24);
            }
            .v3d-btn {
                border: none; background: transparent;
                color: #f3f4f7;
                display: flex; align-items: center; gap: 8px;
                border-radius: 10px; padding: 9px 12px;
                cursor: pointer;
                transition: background 120ms ease, transform 120ms ease,
                            opacity 120ms ease;
                font-size: 13px; font-weight: 500;
            }
            .v3d-btn:hover { background: rgba(255,255,255,0.09); }
            .v3d-btn:active { transform: scale(0.98); }
            .v3d-btn.active { background: rgba(78,132,255,0.22); color: #a8c4ff; }
            .v3d-btn-icon { font-size: 15px; }
            .v3d-popover {
                position: fixed;
                min-width: 260px; max-width: 340px;
                border-radius: 16px;
                background: rgba(16,18,24,0.96);
                color: #eef2ff;
                border: 1px solid rgba(255,255,255,0.08);
                box-shadow: 0 16px 50px rgba(0,0,0,0.4);
                z-index: 100; overflow: hidden;
                backdrop-filter: blur(18px);
            }
            .v3d-popover.hidden { display: none; }
            .v3d-popover-header {
                display: flex; align-items: center;
                justify-content: space-between;
                padding: 14px 16px;
                border-bottom: 1px solid rgba(255,255,255,0.06);
            }
            .v3d-popover-title { font-weight: 700; font-size: 14px; }
            .v3d-close {
                border: none; background: transparent;
                color: inherit; cursor: pointer; font-size: 18px;
            }
            .v3d-popover-content {
                padding: 14px 16px;
                display: flex; flex-direction: column; gap: 12px;
            }
            .v3d-stat-row, .v3d-help-item {
                display: flex; justify-content: space-between;
                gap: 20px; font-size: 13px;
            }
            .v3d-stat-row span:last-child { font-weight: 600; }
            .v3d-toast-stack {
                position: absolute; right: 16px; bottom: 16px;
                display: flex; flex-direction: column; gap: 10px;
                z-index: 40; pointer-events: none;
            }
            .v3d-toast {
                min-width: 240px; max-width: 360px;
                border-radius: 14px;
                background: rgba(18,20,26,0.92);
                border: 1px solid rgba(255,255,255,0.08);
                color: #f1f4ff; overflow: hidden;
                pointer-events: auto;
                box-shadow: 0 14px 34px rgba(0,0,0,0.35);
                backdrop-filter: blur(16px);
            }
            .v3d-toast-header {
                display: flex; justify-content: space-between;
                align-items: center; padding: 10px 12px;
                border-bottom: 1px solid rgba(255,255,255,0.06);
                font-size: 13px; font-weight: 600;
            }
            .v3d-toast-header button {
                border: none; background: transparent;
                color: inherit; cursor: pointer; font-size: 16px;
            }
            .v3d-toast-content { padding: 12px; font-size: 13px; line-height: 1.45; }
            .v3d-measure-line { color: #9cc2ff; }
            canvas { touch-action: none; }

            @media (max-width: 720px) {
                .v3d-toolbar { gap: 8px; }
                .v3d-btn-label { display: none; }
                .v3d-toolbar-section { padding: 4px; }
                .v3d-btn { padding: 10px; }
                .v3d-popover {
                    width: calc(100vw - 24px);
                    left: 12px !important; right: 12px;
                    max-width: none;
                }
            }
        `;
    document.head.appendChild(style);
  }

  // ── Event binding ────────────────────────────────────────────────────────

  bind() {
    this._resizeObs = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const { width, height } = entry.contentRect;
      const w = Math.floor(width);
      const h = Math.floor(height);
      if (w < 1 || h < 1) return;
      this.camera.aspect = w / h;
      this.camera.updateProjectionMatrix();
      this.renderer.setSize(w, h);
    });
    this._resizeObs.observe(this.container);

    this.renderer.domElement.addEventListener("pointerdown", this.onPointerDown);
    window.addEventListener("keydown", this.onKeyDown);
  }

  dispose() {
    if (this._resizeObs) this._resizeObs.disconnect();
    window.removeEventListener("keydown", this.onKeyDown);
    this.renderer.domElement.removeEventListener("pointerdown", this.onPointerDown);
    this.controls.dispose();
    this.renderer.dispose();
  }

  onKeyDown = (e) => {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;

    switch (e.key.toLowerCase()) {
      case "f":
        this.homeCamera();
        return;
      case "g":
        this.state.showGrid = !this.state.showGrid;
        this.grid.visible = this.state.showGrid;
        return;
      case "m":
        this.measurementMode = !this.measurementMode;
        return;
      case "i":
        e.preventDefault();
        this.statsPopover.classList.toggle("hidden");
        return;
      case "?":
        e.preventDefault();
        this.helpPopover.classList.toggle("hidden");
        return;
      case "escape":
        closeAllPopovers();
        this.measurePoints = [];
        return;
    }
  };

  onPointerDown = (e) => {
    if (e.button !== 0) return;

    if (!this.measurementMode) {
      this.performRaycast(e);
      return;
    }

    this.pointer.x = (e.offsetX / this.renderer.domElement.clientWidth) * 2 - 1;
    this.pointer.y = -(e.offsetY / this.renderer.domElement.clientHeight) * 2 + 1;
    this.raycaster.setFromCamera(this.pointer, this.camera);
    const hits = this.raycaster.intersectObject(this.modelRoot, true);
    if (!hits.length) return;

    this.measurePoints.push(hits[0].point.clone());

    if (this.measurePoints.length === 2) {
      const a = this.measurePoints[0];
      const b = this.measurePoints[1];
      const d = distance(a, b);
      this.toastStack.push(
        "Measurement",
        `<div class="v3d-measure-line">Distance: ${d.toFixed(5)}</div>`,
      );
      this.measurePoints.length = 0;
    }
  };

  performRaycast(e) {
    if (!this.modelRoot) return;

    this.pointer.x = (e.offsetX / this.renderer.domElement.clientWidth) * 2 - 1;
    this.pointer.y = -(e.offsetY / this.renderer.domElement.clientHeight) * 2 + 1;
    this.raycaster.setFromCamera(this.pointer, this.camera);
    const hits = this.raycaster.intersectObject(this.modelRoot, true);
    if (!hits.length) return;

    const hit = hits[0];
    this.toastStack.push(
      "Selection",
      `<div><b>Object</b><br>${hit.object.name || "Unnamed"}</div>
             <br>
             <div><b>Position</b><br>
             ${hit.point.x.toFixed(4)}, ${hit.point.y.toFixed(4)}, ${hit.point.z.toFixed(4)}
             </div>`,
    );
  }

  // ── Model ────────────────────────────────────────────────────────────────

  setModel(object) {
    if (this.modelRoot) {
      this.scene.remove(this.modelRoot);
    }
    this.modelRoot = object;
    this.scene.add(object);
    this.detectCapabilities();
    this.computeStats();
    this.rebuildRenderSection();
    this.homeCamera(true);
  }

  detectCapabilities() {
    const caps = { mesh: false, points: false, texture: false, normals: false, vertexColors: false };
    this.modelRoot.traverse((obj) => {
      if (obj.isMesh) {
        caps.mesh = true;
        const mats = Array.isArray(obj.material) ? obj.material : [obj.material];
        mats.forEach((m) => {
          if (!m) return;
          if (m.map) caps.texture = true;
          if (obj.geometry && obj.geometry.attributes.normal) caps.normals = true;
          if (obj.geometry && obj.geometry.attributes.color) caps.vertexColors = true;
        });
      }
      if (obj.isPoints) caps.points = true;
    });
    this.capabilities = caps;
  }

  computeStats() {
    const stats = { triangles: 0, vertices: 0, drawCalls: 0, materials: 0, textures: 0 };
    const materialSet = new Set();
    const textureSet = new Set();

    this.modelRoot.traverse((obj) => {
      if (obj.geometry && obj.geometry.attributes.position) {
        stats.vertices += obj.geometry.attributes.position.count;
      }
      if (obj.isMesh) {
        stats.drawCalls++;
        const geom = obj.geometry;
        if (geom.index) {
          stats.triangles += geom.index.count / 3;
        } else if (geom.attributes.position) {
          stats.triangles += geom.attributes.position.count / 3;
        }
        const mats = Array.isArray(obj.material) ? obj.material : [obj.material];
        mats.forEach((m) => {
          if (!m) return;
          materialSet.add(m);
          if (m.map) textureSet.add(m.map);
        });
      }
    });

    stats.materials = materialSet.size;
    stats.textures = textureSet.size;
    this.stats = stats;

    const update = (id, value) => {
      const el = document.getElementById(id);
      if (el) el.textContent = value.toLocaleString();
    };
    update("v3d-stat-tris", stats.triangles);
    update("v3d-stat-verts", stats.vertices);
    update("v3d-stat-mats", stats.materials);
    update("v3d-stat-tex", stats.textures);
  }

  // ── Render section (built dynamically after setModel) ────────────────────

  rebuildRenderSection() {
    this.sections.render.innerHTML = "";

    if (this.capabilities.mesh) {
      const wireBtn = createButton("◫", "Wireframe", () => {
        this.state.wireframe = !this.state.wireframe;
        this.applyRenderMode();
        savePrefs(this.state);
        wireBtn.classList.toggle("active", this.state.wireframe);
      });
      wireBtn.classList.toggle("active", this.state.wireframe);
      this.sections.render.appendChild(wireBtn);

      const lightBtn = createButton("☀", "Lighting", () => {
        this.state.lighting = !this.state.lighting;
        this.applyRenderMode();
        savePrefs(this.state);
        lightBtn.classList.toggle("active", this.state.lighting);
      });
      lightBtn.classList.toggle("active", this.state.lighting);
      this.sections.render.appendChild(lightBtn);

      const backBtn = createButton("◐", "Backfaces", () => {
        this.state.backfaces = !this.state.backfaces;
        this.applyRenderMode();
        savePrefs(this.state);
        backBtn.classList.toggle("active", this.state.backfaces);
      });
      backBtn.classList.toggle("active", this.state.backfaces);
      this.sections.render.appendChild(backBtn);

      const rayBtn = createButton("◎", "Raytrace", () => {
        this.state.raytracing = !this.state.raytracing;
        this.applyRenderMode();
        savePrefs(this.state);
        rayBtn.classList.toggle("active", this.state.raytracing);
      });
      rayBtn.classList.toggle("active", this.state.raytracing);
      this.sections.render.appendChild(rayBtn);
    }

    if (this.capabilities.texture) {
      const texBtn = createButton("🖼", "Textures", () => {
        this.toggleTextures();
      });
      texBtn.classList.toggle("active", true);
      this.sections.render.appendChild(texBtn);
    }

    if (this.capabilities.points) {
      const pointsBtn = createButton("•", "Point Size", () => {});
      this.sections.render.appendChild(pointsBtn);

      const content = document.createElement("div");
      content.className = "v3d-popover-content";
      const slider = document.createElement("input");
      slider.type = "range";
      slider.min = 0.1;
      slider.max = 10;
      slider.step = 0.1;
      slider.value = this.state.pointsSize;
      slider.addEventListener("input", () => {
        this.state.pointsSize = parseFloat(slider.value);
        this.applyRenderMode();
        savePrefs(this.state);
      });
      content.appendChild(slider);
      createPopover(pointsBtn, "Point Size", content);
    }

    // Lighting offset slider (always visible when there's a model)
    const lightOffBtn = createButton("↻", "Light Dir", () => {});
    this.sections.render.appendChild(lightOffBtn);

    const loContent = document.createElement("div");
    loContent.className = "v3d-popover-content";
    const loLabel = document.createElement("label");
    loLabel.style.cssText = "font-size:13px;display:flex;justify-content:space-between";
    const loSpan = document.createElement("span");
    loSpan.textContent = `${this.state.lightingOffset}°`;
    loLabel.innerHTML = "<span>Azimuth offset</span>";
    loLabel.appendChild(loSpan);

    const loSlider = document.createElement("input");
    loSlider.type = "range";
    loSlider.min = -180;
    loSlider.max = 180;
    loSlider.step = 1;
    loSlider.value = this.state.lightingOffset;
    loSlider.addEventListener("input", () => {
      this.state.lightingOffset = parseFloat(loSlider.value);
      loSpan.textContent = `${this.state.lightingOffset}°`;
      savePrefs(this.state);
    });

    loContent.appendChild(loLabel);
    loContent.appendChild(loSlider);
    createPopover(lightOffBtn, "Light Direction", loContent);
  }

  // ── Texture toggle ───────────────────────────────────────────────────────

  toggleTextures() {
    this.modelRoot.traverse((obj) => {
      if (!obj.material) return;
      const mats = Array.isArray(obj.material) ? obj.material : [obj.material];
      mats.forEach((m) => {
        if (!m) return;
        if (!m.userData.originalMap) {
          m.userData.originalMap = m.map || null;
        }
        if (m.map) {
          m.map = null;
        } else {
          m.map = m.userData.originalMap;
        }
        m.needsUpdate = true;
      });
    });
  }

  // ── Render mode ──────────────────────────────────────────────────────────

  applyRenderMode() {
    this.modelRoot.traverse((obj) => {
      if (obj.isMesh && obj.material) {
        const mats = Array.isArray(obj.material) ? obj.material : [obj.material];
        const updated = [];

        for (const m of mats) {
          if (!m) { updated.push(m); continue; }

          m.wireframe = this.state.wireframe;
          m.side = this.state.backfaces ? THREE.DoubleSide : THREE.FrontSide;
          m.needsUpdate = true;

          const wantLighting = this.state.lighting && !this.state.raytracing;
          const wantBasic = !wantLighting;

          if (this.state.raytracing && m.type !== "MeshPhysicalMaterial") {
            const r = new THREE.MeshPhysicalMaterial();
            copyMaterialProps(m, r);
            r.roughness = 0.25;
            r.metalness = 0.6;
            r.clearcoat = 0.4;
            r.needsUpdate = true;
            updated.push(r);
          } else if (wantLighting && m.type === "MeshBasicMaterial") {
            const r = new THREE.MeshStandardMaterial();
            copyMaterialProps(m, r);
            r.needsUpdate = true;
            updated.push(r);
          } else if (wantBasic && m.type !== "MeshBasicMaterial") {
            const r = new THREE.MeshBasicMaterial();
            copyMaterialProps(m, r);
            r.needsUpdate = true;
            updated.push(r);
          } else {
            updated.push(m);
          }
        }

        if (Array.isArray(obj.material)) {
          obj.material = updated;
        } else {
          obj.material = updated[0];
        }
      }

      if (obj.isPoints && obj.material) {
        obj.material.size = this.state.pointsSize;
        obj.material.needsUpdate = true;
      }
    });
  }

  // ── Home / fit ───────────────────────────────────────────────────────────

  homeCamera(forceDir) {
    if (!this.modelRoot) return;

    const box = new THREE.Box3().setFromObject(this.modelRoot);
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());

    const radius = Math.max(size.x, size.y, size.z) * 0.5;
    const fov = this.camera.fov * (Math.PI / 180);
    let dist = radius / Math.tan(fov / 2);
    dist *= 1.8;

    const dir = new THREE.Vector3();
    if (forceDir) {
      dir.set(1, 1, 1).normalize();
    } else {
      dir.copy(this.camera.position).sub(this.controls.target);
      if (dir.lengthSq() < 1e-10) dir.set(0, 0, 1);
      dir.normalize();
    }

    this.controls.target.copy(center);
    this.camera.position.copy(center).add(dir.multiplyScalar(dist));
    this.camera.near = Math.max(0.001, dist / 1000);
    this.camera.far = Math.max(1000, dist * 100);
    this.camera.updateProjectionMatrix();
    this.camera.lookAt(center);
    this.controls.saveSpherical();
  }

  // ── Update lighting (called each frame) ──────────────────────────────────

  updateLighting() {
    const offset = new THREE.Vector3().copy(this.camera.position).sub(this.controls.target);
    const radius = offset.length();
    if (radius < 1e-10) return;

    const theta = Math.atan2(offset.x, offset.z) + this.state.lightingOffset * (Math.PI / 180);
    offset.x = radius * Math.sin(theta);
    offset.z = radius * Math.cos(theta);

    this.dirLight.position.copy(this.controls.target).add(offset);
    this.dirLight.target.position.copy(this.controls.target);
  }

  // ── Animation loop ───────────────────────────────────────────────────────

  animate = () => {
    requestAnimationFrame(this.animate);
    this.timer.update();
    const dt = this.timer.getDelta();

    this.controls.update(dt);
    this.updateLighting();

    for (const mixer of this.mixers) {
      mixer.update(dt);
    }

    this.renderer.render(this.scene, this.camera);
  };
}

// ── Launch helper — called by the Rust backend ────────────────────────────

export async function launchGlbViewer(b64, filename) {
  const container = document.createElement("div");
  container.id = "viewer3d-container";
  Object.assign(container.style, {
    position: "fixed",
    inset: "0",
    zIndex: "9999",
    background: "#111318",
  });

  const closeBtn = document.createElement("button");
  closeBtn.textContent = "\u00d7";
  Object.assign(closeBtn.style, {
    position: "fixed",
    top: "16px",
    right: "16px",
    zIndex: "10000",
    width: "36px",
    height: "36px",
    borderRadius: "50%",
    border: "none",
    background: "rgba(255,255,255,0.12)",
    color: "#fff",
    fontSize: "20px",
    cursor: "pointer",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
  });

  document.body.append(container, closeBtn);

  const binary = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  const blob = new Blob([binary], { type: "model/gltf-binary" });
  const url = URL.createObjectURL(blob);

  try {
    const gltf = await new Promise((resolve, reject) => {
      const loader = new GLTFLoader();
      loader.load(url, resolve, undefined, reject);
    });

    const viewer = new Viewer3D(container);
    viewer.setModel(gltf.scene);

    closeBtn.onclick = () => {
      viewer.dispose();
      container.remove();
      closeBtn.remove();
    };

    return viewer;
  } catch (err) {
    container.remove();
    closeBtn.remove();
    URL.revokeObjectURL(url);
    throw err;
  } finally {
    URL.revokeObjectURL(url);
  }
}

// ── Optional utility helpers ──────────────────────────────────────────────

export function centerAndScaleModel(object, targetSize = 1) {
  const box = new THREE.Box3().setFromObject(object);
  const size = box.getSize(new THREE.Vector3());
  const center = box.getCenter(new THREE.Vector3());
  const maxAxis = Math.max(size.x, size.y, size.z);
  if (maxAxis <= 0) return;
  const scale = targetSize / maxAxis;
  object.position.sub(center);
  object.scale.setScalar(scale);
}

export function disposeHierarchy(root) {
  root.traverse((obj) => {
    if (obj.geometry) obj.geometry.dispose();
    if (obj.material) {
      const mats = Array.isArray(obj.material) ? obj.material : [obj.material];
      mats.forEach((m) => {
        if (!m) return;
        for (const k in m) {
          const value = m[k];
          if (value && value.isTexture) value.dispose();
        }
        m.dispose();
      });
    }
  });
}
