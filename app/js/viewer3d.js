import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { ArcballControls } from "three/examples/jsm/controls/ArcballControls.js";

// ── State persistence ──────────────────────────────────────────────────────

const STORAGE_KEY = "viewer3d.prefs";

// ── URL state helpers ──────────────────────────────────────────────────────
//
// URL format: /viewer/:name/:file_encoded/:cfg
//   :file_encoded  — URL-encoded output-file path
//   :cfg           — base64 JSON blob: {cam:{position,target,up}, config:{…}}
//
// Path-based:  /viewer/MyProject/colpak%2Ffile/eyJjYW0iO…9fQ==
// Hash-based:  /#/viewer/MyProject/colpak%2Ffile/eyJjYW0iO…9fQ==

/**
 * Update the URL with viewer state using replaceState (no history push).
 *
 * File paths use pipe separators (|) instead of / to avoid the router
 * treating path components as separate segments.
 */
function updateViewerUrl(projectName, filePath, cfgBlobB64) {
  var pipePath = filePath.replace(/\//g, "|");
  var encFile = encodeURIComponent(pipePath);
  // cfgBlobB64 comes from btoa() which produces standard base64 (+ / =).
  // Make it URL-safe for the route segment: + → -, / → _ while keeping
  // padding so the Rust URL_SAFE decoder can parse it.
  var safeCfg = cfgBlobB64 ? cfgBlobB64.replace(/\+/g, "-").replace(/\//g, "_") : "";
  var url;
  if (window.location.hash.startsWith("#/")) {
    url = "#/viewer/" + encodeURIComponent(projectName) + "/" + encFile + "/" + safeCfg;
  } else {
    url = "/viewer/" + encodeURIComponent(projectName) + "/" + encFile + "/" + safeCfg;
  }
  history.replaceState(null, "", url);
}

/** debounce helper */
function debounce(fn, ms) {
  let timer;
  return (...args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  };
}
/**
 * Derive the default background colour matching the app theme.
 *
 * When the server forces a theme it sets the data-theme attribute on
 * <html>.  When data-theme is absent the server left the decision to
 * the browser's prefers-color-scheme media query (the common case on
 * non-Android platforms) — we mirror that via matchMedia to stay in
 * sync with what the CSS actually renders.
 */
function getDefaultBackground() {
  var theme = document.documentElement.getAttribute("data-theme");
  if (!theme) {
    theme = window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }
  return theme === "dark" ? "#111318" : "#e8ecf0";
}

const DEFAULT_STATE = {
  // background is set dynamically in the constructor so it reflects the
  // current data-theme at render time rather than at module-load time.
  textures: true,
  wireframe: false,
  backfaces: false,
  lighting: true,
  lightAzimuth: 0,
  lightElevation: 30,
  pointsSize: 1.5,
  toneMapping: true,
  exposure: 1.0,
};

function loadPrefs() {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY));
    return { ...DEFAULT_STATE, ...(saved || {}) };
  } catch {
    return { ...DEFAULT_STATE };
  }
}

function savePrefs(state) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  } catch {}
}

// ── Toast stack ─────────────────────────────────────────────────────────────

class ToastStack {
  constructor(root) {
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
      <div class="v3d-toast-content">${html}</div>`;
    card.querySelector("button").onclick = () => card.remove();

    let timer = setTimeout(() => card.remove(), 10000);
    card.addEventListener("pointerenter", () => clearTimeout(timer));
    card.addEventListener("pointerleave", () => {
      timer = setTimeout(() => card.remove(), 10000);
    });
    this.el.appendChild(card);
    return card;
  }
}

// ── CSS (injected once) ─────────────────────────────────────────────────────

const VIEWER_STYLE_ID = "v3d-style";

function injectViewerStyles() {
  if (document.getElementById(VIEWER_STYLE_ID)) return;

  const css = document.createElement("style");
  css.id = VIEWER_STYLE_ID;
  css.textContent = `
.v3d-toolbar {
  position:absolute;top:14px;left:14px;display:flex;gap:10px;z-index:50;
  flex-wrap:wrap;max-width:calc(100% - 28px);pointer-events:none;
}
.v3d-toolbar-section {
  display:flex;gap:6px;padding:6px;border-radius:14px;
  background:rgba(18,20,26,0.5);backdrop-filter:blur(2px);
  border:1px solid rgba(255,255,255,0.08);pointer-events:auto;
  box-shadow:0 10px 30px rgba(0,0,0,0.24);
}
.v3d-btn {
  border:none;background:rgba(255,255,255,0.07);color:#f3f4f7;
  display:flex;align-items:center;gap:8px;border-radius:10px;
  padding:9px 12px;cursor:pointer;font-size:13px;font-weight:500;
  transition:background 120ms,transform 120ms,opacity 120ms;
}
.v3d-btn:hover { background:rgba(255,255,255,0.16); }
.v3d-btn:active { transform:scale(0.98); }
.v3d-btn.active { background:rgba(78,132,255,0.28);color:#a8c4ff; }
.v3d-btn-icon { font-size:15px; }
.v3d-panel {
  position:fixed;min-width:260px;max-width:340px;border-radius:16px;
  background:rgba(16,18,24,0.5);color:#eef2ff;
  border:1px solid rgba(255,255,255,0.08);box-shadow:0 16px 50px rgba(0,0,0,0.4);
  z-index:10000;overflow:hidden;backdrop-filter:blur(2px);
}
.v3d-panel.hidden { display:none; }
.v3d-panel-header {
  display:flex;align-items:center;justify-content:space-between;
  padding:14px 16px;border-bottom:1px solid rgba(255,255,255,0.06);
}
.v3d-panel-title { font-weight:700;font-size:14px; }
.v3d-close { border:none;background:transparent;color:inherit;cursor:pointer;font-size:18px; }
.v3d-panel-content { padding:14px 16px;display:flex;flex-direction:column;gap:12px; }
.v3d-stat-row,.v3d-help-item { display:flex;justify-content:space-between;gap:20px;font-size:13px; }
.v3d-stat-row span:last-child { font-weight:600; }
.v3d-toast-stack {
  position:absolute;right:16px;bottom:16px;display:flex;flex-direction:column;
  gap:10px;z-index:40;pointer-events:none;
}
.v3d-toast {
  min-width:240px;max-width:360px;border-radius:14px;
  background:rgba(18,20,26,0.92);border:1px solid rgba(255,255,255,0.08);
  color:#f1f4ff;overflow:hidden;pointer-events:auto;
  box-shadow:0 14px 34px rgba(0,0,0,0.35);backdrop-filter:blur(16px);
}
.v3d-toast-header {
  display:flex;justify-content:space-between;align-items:center;
  padding:10px 12px;border-bottom:1px solid rgba(255,255,255,0.06);
  font-size:13px;font-weight:600;
}
.v3d-toast-header button {
  border:none;background:transparent;color:inherit;cursor:pointer;font-size:16px;
}
.v3d-toast-content { padding:12px;font-size:13px;line-height:1.45; }
.v3d-measure-line { color:#9cc2ff; }
canvas { touch-action:none; }
/* Label visibility is now handled by JS (_updateToolbarLabels) so it
   accounts for CSS zoom (e.g. from Playwright screenshots). */
.v3d-compact .v3d-toolbar { gap:8px; }
.v3d-compact .v3d-btn-label { display:none; }
.v3d-compact .v3d-toolbar-section { padding:4px; }
.v3d-compact .v3d-btn { padding:10px; }
.v3d-compact .v3d-panel { width:calc(100vw - 24px);left:12px !important;right:12px;max-width:none; }`;
  document.head.appendChild(css);
}

// ── Main viewer ─────────────────────────────────────────────────────────────

export class Viewer3D {
  constructor(container, opts = {}) {
    this.container = container;
    this._projectName = opts.projectName || "";
    this._filePath = opts.filePath || "";
    this._onStateChange = opts.onStateChange || null;

    // Merge state from: theme-aware defaults → saved prefs → URL/options.
    // DEFAULT_STATE (without background) provides the non-colour defaults.
    // getDefaultBackground() is called here (not at module load time) so it
    // picks up whatever data-theme is active when the viewer renders.
    // URL config may still override the background, but it should normally
    // be left out so the theme choice stays in one place.
    const saved = loadPrefs();
    this.state = { background: getDefaultBackground(), ...DEFAULT_STATE, ...saved, ...(opts.initialConfig || {}) };

    this.scene = new THREE.Scene();
    this.scene.background = new THREE.Color(this.state.background);

    // preserveDrawingBuffer is required so that Playwright / headless
    // Chromium can capture the WebGL canvas in a page screenshot.
    // Without it the GPU drawing buffer is cleared after compositing and
    // the screenshot comes back empty (just the background colour).
    this.renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false, preserveDrawingBuffer: true });
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

    // controls
    this.controls = new ArcballControls(this.camera, canvas, this.scene);
    this.controls.enableGrid = false;
    this.controls.enableFocus = true;
    this.controls.enableAnimations = true;
    this.controls.enablePan = true;
    this.controls.enableRotate = true;
    this.controls.enableZoom = true;
    this.controls.rotateSpeed = 1;
    this.controls.dampingFactor = 25;
    this.controls.minDistance = 0.001;
    this.controls.maxDistance = Infinity;
    this.controls.target.set(0, 0, 0);

    this._queuedCameraState = opts.initialCamera || null;
    this._debouncedPersistUrl = debounce(() => {
      this._persistUrlState();
      if (this._onStateChange) {
        const cam = this._getCameraState();
        this._onStateChange({ camera: cam, config: { ...this.state } });
      }
    }, 300);

    this.mixers = [];

    this.raycaster = new THREE.Raycaster();
    this.pointer = new THREE.Vector2();

    this.modelRoot = null;
    this.capabilities = { mesh: false, points: false, texture: false, normals: false, vertexColors: false };
    this.stats = { triangles: 0, vertices: 0, drawCalls: 0, materials: 0, textures: 0 };

    this.measurementMode = false;
    this.measurePoints = [];
    this.raycastEnabled = false;

    this.toastStack = new ToastStack(container);
    this._panel = null; // reusable info panel

    this.createEnvironment();
    this.createToolbar();
    injectViewerStyles();
    this.bind();
    this.animate();

    // Zoom-aware toolbar label hiding (matches sidebar breakpoint logic)
    this._updateToolbarLabels();
    this._resizeLabels = () => this._updateToolbarLabels();
    window.addEventListener("resize", this._resizeLabels);
  }

  // ── Environment ──────────────────────────────────────────────────────────

  createEnvironment() {
    this.hemiLight = new THREE.HemisphereLight(0xffffff, 0x222233, 0.6);
    this.scene.add(this.hemiLight);

    this.dirLight = new THREE.DirectionalLight(0xffffff, 1.8);
    this.dirLight.position.set(4, 8, 6);
    this.scene.add(this.dirLight);
    this.scene.add(this.dirLight.target);
  }

  // ── Toolbar ───────────────────────────────────────────────────────────────

  createToolbar() {
    // Try to embed into the Rust-managed top bar first; fall back to a
    // floating toolbar inside the container.
    const host = document.getElementById("viewer-toolbar");
    if (host) {
      host.innerHTML = "";
      this.toolbar = host;
    } else {
      this.toolbar = document.createElement("div");
      this.toolbar.className = "v3d-toolbar";
      this.container.appendChild(this.toolbar);
    }

    const sec = (label) => {
      const s = document.createElement("div");
      s.className = "v3d-toolbar-section";
      s.dataset.label = label;
      return s;
    };

    const viewSec = sec("View");
    const renderSec = sec("Render");
    const toolsSec = sec("Tools");
    const infoSec = sec("Info");
    this.toolbar.append(viewSec, renderSec, toolsSec, infoSec);
    this.sections = { view: viewSec, render: renderSec, tools: toolsSec, info: infoSec };

    // ---- View ----
    this._btnHome = this._addBtn(viewSec, "🏠", "Home", () => this.homeCamera(true));

    // ---- Tools ----
    this._addBtn(toolsSec, "📏", "Measure", () => {
      this.measurementMode = !this.measurementMode;
      this._btnMeasure.classList.toggle("active", this.measurementMode);
    });
    this._btnMeasure = toolsSec.lastChild;

    this._addBtn(toolsSec, "🎯", "Pick", () => {
      this.raycastEnabled = !this.raycastEnabled;
      this._btnPick.classList.toggle("active", this.raycastEnabled);
    });
    this._btnPick = toolsSec.lastChild;

    // ---- Info ----
    this._addBtn(infoSec, "📊", "Stats", (e) => this._togglePanel("stats", e.currentTarget));
    this._addBtn(infoSec, "❓", "Help", (e) => this._togglePanel("help", e.currentTarget));
  }

  _addBtn(section, icon, label, onClick) {
    const btn = document.createElement("button");
    btn.className = "v3d-btn";
    btn.type = "button";
    btn.title = label;
    btn.setAttribute("aria-label", label);
    btn.innerHTML = `<span class="v3d-btn-icon">${icon}</span><span class="v3d-btn-label">${label}</span>`;
    btn.addEventListener("click", onClick);
    section.appendChild(btn);
    return btn;
  }

  _togglePanel(kind, triggerBtn) {
    if (this._panel && !this._panel.classList.contains("hidden") && this._panel._kind === kind) {
      this._panel.classList.add("hidden");
      return;
    }

    if (this._panel) this._panel.remove();
    if (this._panelOutside) document.removeEventListener("pointerdown", this._panelOutside);

    this._panel = document.createElement("div");
    this._panel.className = "v3d-panel";
    this._panel._kind = kind;
    document.body.appendChild(this._panel);

    const close = () => {
      this._panel.classList.add("hidden");
    };

    if (kind === "stats") {
      const s = this.stats;
      this._panel.innerHTML = `
        <div class="v3d-panel-header">
          <div class="v3d-panel-title">Scene Statistics</div>
          <button class="v3d-close">×</button>
        </div>
        <div class="v3d-panel-content">
          <div class="v3d-stat-row"><span>Triangles</span><span>${s.triangles.toLocaleString()}</span></div>
          <div class="v3d-stat-row"><span>Vertices</span><span>${s.vertices.toLocaleString()}</span></div>
          <div class="v3d-stat-row"><span>Materials</span><span>${s.materials.toLocaleString()}</span></div>
          <div class="v3d-stat-row"><span>Textures</span><span>${s.textures.toLocaleString()}</span></div>
        </div>`;
    } else {
      this._panel.innerHTML = `
        <div class="v3d-panel-header">
          <div class="v3d-panel-title">Controls</div>
          <button class="v3d-close">×</button>
        </div>
        <div class="v3d-panel-content">
          <div class="v3d-help-item"><b>Rotate</b><span>Left drag</span></div>
          <div class="v3d-help-item"><b>Pan</b><span>Middle / right drag</span></div>
          <div class="v3d-help-item"><b>Zoom</b><span>Mouse wheel</span></div>
          <div class="v3d-help-item"><b>Pick</b><span>Press <kbd>R</kbd></span></div>
          <div class="v3d-help-item"><b>Measure</b><span>Press <kbd>M</kbd></span></div>
          <div class="v3d-help-item"><b>Home</b><span>Press <kbd>F</kbd></span></div>
          <div class="v3d-help-item"><b>Stats</b><span>Press <kbd>I</kbd></span></div>
          <div class="v3d-help-item"><b>Help</b><span>Press <kbd>?</kbd></span></div>
        </div>`;
    }

    this._panel.querySelector(".v3d-close").onclick = close;

    // Position relative to the trigger button
    const trigger = triggerBtn || this.toolbar.querySelector(".v3d-btn");
    if (trigger) {
      const r = trigger.getBoundingClientRect();
      this._panel.style.left = Math.min(r.left, window.innerWidth - 360) + "px";
      this._panel.style.top = r.bottom + 10 + "px";
    }

    // Close on outside click (track reference for cleanup)
    this._panelOutside = (e) => {
      if (!this._panel.contains(e.target) && !e.target.closest(".v3d-btn")) {
        close();
        document.removeEventListener("pointerdown", this._panelOutside);
      }
    };
    setTimeout(() => document.addEventListener("pointerdown", this._panelOutside), 0);
  }

  // ── Light direction panel (azimuth, elevation, distance) ──────────────

  _createLightPanel() {
    const container = this.container;

    const show = () => {
      if (this._loPanel && !this._loPanel.classList.contains("hidden")) {
        this._loPanel.classList.add("hidden");
        return;
      }
      if (!this._loPanel) {
        this._loPanel = document.createElement("div");
        this._loPanel.className = "v3d-panel";
        document.body.appendChild(this._loPanel);

        const sA = (this.state.lightAzimuth || 0).toFixed(0);
        const sE = (this.state.lightElevation || 0).toFixed(0);
        this._loPanel.innerHTML = `
          <div class="v3d-panel-header">
            <div class="v3d-panel-title">Light Direction</div>
            <button class="v3d-close">×</button>
          </div>
          <div class="v3d-panel-content">
            <label style="font-size:13px;display:flex;justify-content:space-between">
              <span>Azimuth °</span><span id="v3d-lo-az">${sA}</span>
            </label>
            <input type="range" min="-180" max="180" step="1" value="${sA}" data-lo-axis="azimuth">
            <label style="font-size:13px;display:flex;justify-content:space-between">
              <span>Elevation °</span><span id="v3d-lo-el">${sE}</span>
            </label>
            <input type="range" min="-90" max="90" step="1" value="${sE}" data-lo-axis="elevation">
          </div>`;

        this._loPanel.querySelector(".v3d-close").onclick = () => this._loPanel.classList.add("hidden");

        for (const input of this._loPanel.querySelectorAll("[data-lo-axis]")) {
          input.addEventListener("input", () => {
            const axis = input.dataset.loAxis;
            const v = parseFloat(input.value);
            this.state[axis === "azimuth" ? "lightAzimuth" : "lightElevation"] = v;
            const id = axis === "azimuth" ? "v3d-lo-az" : "v3d-lo-el";
            const span = document.getElementById(id);
            if (span) span.textContent = v.toFixed(0);
            this._onConfigChange();
          });
        }
      } else {
        if (this._loOutside) document.removeEventListener("pointerdown", this._loOutside);
      }

      this._loPanel.classList.remove("hidden");

      const btn = document.querySelector("[data-lo-btn]");
      if (btn) {
        const r = btn.getBoundingClientRect();
        this._loPanel.style.left = Math.min(r.left, window.innerWidth - 360) + "px";
        this._loPanel.style.top = r.bottom + 10 + "px";
      }

      this._loOutside = (e) => {
        if (!this._loPanel.contains(e.target) && !e.target.closest("[data-lo-btn]")) {
          this._loPanel.classList.add("hidden");
          document.removeEventListener("pointerdown", this._loOutside);
        }
      };
      setTimeout(() => document.addEventListener("pointerdown", this._loOutside), 0);
    };

    return show;
  }

  // ── Point size slider panel (called from rebuildRenderSection) ──────────

  _createPointsSlider() {
    const show = () => {
      if (this._psPanel && !this._psPanel.classList.contains("hidden")) {
        this._psPanel.classList.add("hidden");
        return;
      }
      if (!this._psPanel) {
        this._psPanel = document.createElement("div");
        this._psPanel.className = "v3d-panel";
        document.body.appendChild(this._psPanel);

        this._psPanel.innerHTML = `
          <div class="v3d-panel-header">
            <div class="v3d-panel-title">Point Size</div>
            <button class="v3d-close">×</button>
          </div>
          <div class="v3d-panel-content">
            <input type="range" min="0.1" max="10" step="0.1" value="${this.state.pointsSize}" id="v3d-ps-slider">
          </div>`;

        this._psPanel.querySelector(".v3d-close").onclick = () => this._psPanel.classList.add("hidden");

        const slider = this._psPanel.querySelector("#v3d-ps-slider");
        slider.addEventListener("input", () => {
          this.state.pointsSize = parseFloat(slider.value);
          this.applyRenderMode();
          this._onConfigChange();
        });
      } else {
        if (this._psOutside) document.removeEventListener("pointerdown", this._psOutside);
      }

      this._psPanel.classList.remove("hidden");

      const btn = this._psBtn;
      if (btn) {
        const r = btn.getBoundingClientRect();
        this._psPanel.style.left = Math.min(r.left, window.innerWidth - 360) + "px";
        this._psPanel.style.top = r.bottom + 10 + "px";
      }

      this._psOutside = (e) => {
        if (!this._psPanel.contains(e.target) && !e.target.closest(".v3d-close") && !e.target.closest(".v3d-btn")) {
          this._psPanel.classList.add("hidden");
          document.removeEventListener("pointerdown", this._psOutside);
        }
      };
      setTimeout(() => document.addEventListener("pointerdown", this._psOutside), 0);
    };

    return show;
  }

  // ── Render section (rebuilt after model loads) ────────────────────────────

  rebuildRenderSection() {
    this.sections.render.innerHTML = "";

    if (this.capabilities.mesh) {
      const wireBtn = this._addBtn(this.sections.render, "🔲", "Wireframe", () => {
        this.state.wireframe = !this.state.wireframe;
        this.applyRenderMode();
        this._onConfigChange();
        wireBtn.classList.toggle("active", this.state.wireframe);
      });
      wireBtn.classList.toggle("active", this.state.wireframe);

      const lightBtn = this._addBtn(this.sections.render, "💡", "Lighting", () => {
        this.state.lighting = !this.state.lighting;
        this.applyRenderMode();
        this._onConfigChange();
        lightBtn.classList.toggle("active", this.state.lighting);
      });
      lightBtn.classList.toggle("active", this.state.lighting);

      const backBtn = this._addBtn(this.sections.render, "🔁", "Backfaces", () => {
        this.state.backfaces = !this.state.backfaces;
        this.applyRenderMode();
        this._onConfigChange();
        backBtn.classList.toggle("active", this.state.backfaces);
      });
      backBtn.classList.toggle("active", this.state.backfaces);

      const loBtn = this._addBtn(this.sections.render, "🔆", "Light Dir", this._createLightPanel());
      loBtn.setAttribute("data-lo-btn", "");
    }

    if (this.capabilities.texture) {
      const texBtn = this._addBtn(this.sections.render, "🖼", "Textures", () => {
        this.state.textures = !this.state.textures;
        this.applyTextures();
        this._onConfigChange();
        texBtn.classList.toggle("active", this.state.textures);
      });
      texBtn.classList.toggle("active", this.state.textures);
    }

    if (this.capabilities.points) {
      this._psBtn = this._addBtn(this.sections.render, "🔵", "Point Size", this._createPointsSlider());
    }
  }

  // ── Event binding ─────────────────────────────────────────────────────────

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

    this.renderer.domElement.addEventListener("pointerdown", this.onPointerDown, { capture: true });
    window.addEventListener("keydown", this.onKeyDown);

    // Close panels on Escape
    this._onEscapePanel = (e) => {
      if (e.key === "Escape") {
        if (this._panel) this._panel.classList.add("hidden");
        if (this._loPanel) this._loPanel.classList.add("hidden");
        if (this._psPanel) this._psPanel.classList.add("hidden");
      }
    };
    window.addEventListener("keydown", this._onEscapePanel);

    // Track camera changes → persist to URL hash (debounced)
    this.controls.addEventListener("change", () => {
      this._debouncedPersistUrl();
    });
  }

  _updateToolbarLabels() {
    // Same zoom-aware check as the sidebar: divide viewport width by the
    // CSS zoom factor so labels don't spill offscreen on zoomed viewports.
    const zoom = parseFloat(document.documentElement.style.zoom) || 1;
    const compact = window.innerWidth / zoom < 720;
    if (this.container) {
      document.documentElement.classList.toggle("v3d-compact", compact);
    }
  }

  dispose() {
    document.documentElement.classList.remove("v3d-compact");
    window.removeEventListener("resize", this._resizeLabels);
    if (this._resizeObs) this._resizeObs.disconnect();
    window.removeEventListener("keydown", this.onKeyDown);
    window.removeEventListener("keydown", this._onEscapePanel);
    this.renderer.domElement.removeEventListener("pointerdown", this.onPointerDown, { capture: true });
    this.controls.dispose();
    this.renderer.dispose();
    for (const el of [this.toolbar, this._panel, this._loPanel, this._psPanel]) {
      if (el && el.parentNode) el.remove();
    }
  }

  onKeyDown = (e) => {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;

    switch (e.key.toLowerCase()) {
      case "f":
        this.homeCamera();
        return;
      case "m":
        this.measurementMode = !this.measurementMode;
        return;
      case "r":
        this.raycastEnabled = !this.raycastEnabled;
        return;
      case "i":
        e.preventDefault();
        this._togglePanel("stats");
        return;
      case "?":
        e.preventDefault();
        this._togglePanel("help");
        return;
      case "escape":
        this.measurePoints = [];
        return;
    }
  };

  onPointerDown = (e) => {
    if (e.button !== 0) return;

    if (this.measurementMode) {
      e.stopPropagation();
      this.pointer.x = (e.offsetX / this.renderer.domElement.clientWidth) * 2 - 1;
      this.pointer.y = -(e.offsetY / this.renderer.domElement.clientHeight) * 2 + 1;
      this.raycaster.setFromCamera(this.pointer, this.camera);
      const hits = this.raycaster.intersectObject(this.modelRoot, true);
      if (!hits.length) return;

      this.measurePoints.push(hits[0].point.clone());
      if (this.measurePoints.length === 2) {
        const d = this.measurePoints[0].distanceTo(this.measurePoints[1]);
        this.toastStack.push("Measurement", `<div class="v3d-measure-line">Distance: ${d.toFixed(5)}</div>`);
        this.measurePoints.length = 0;
      }
      return;
    }

    if (this.raycastEnabled) {
      e.stopPropagation();
      if (!this.modelRoot) return;
      this.pointer.x = (e.offsetX / this.renderer.domElement.clientWidth) * 2 - 1;
      this.pointer.y = -(e.offsetY / this.renderer.domElement.clientHeight) * 2 + 1;
      this.raycaster.setFromCamera(this.pointer, this.camera);
      const hits = this.raycaster.intersectObject(this.modelRoot, true);
      if (!hits.length) return;
      const hit = hits[0];
      this.toastStack.push(
        "Selection",
        `<div><b>Object</b><br>${hit.object.name || "Unnamed"}</div><br>
         <div><b>Position</b><br>${hit.point.x.toFixed(4)}, ${hit.point.y.toFixed(4)}, ${hit.point.z.toFixed(4)}</div>`,
      );
      return;
    }
  };

  // ── Model ─────────────────────────────────────────────────────────────────

  setModel(object, initialCamera) {
    if (this.modelRoot) this.scene.remove(this.modelRoot);
    this.modelRoot = object;
    this.scene.add(object);

    this.detectCapabilities();
    this.computeStats();
    this.rebuildRenderSection();
    this.applyRenderMode();
    this.applyTextures();

    // Queue the camera state from URL so homeCamera picks it up
    if (initialCamera) {
      this._queuedCameraState = initialCamera;
    }
    this.homeCamera(false);

    // Force an immediate render so there is at least one frame before the
    // animation loop (driven by requestAnimationFrame) kicks in.  This is
    // important on headless / CI environments where rAF timing may lag,
    // causing the first screenshot to be blank.
    this.renderer.render(this.scene, this.camera);
  }

  detectCapabilities() {
    const caps = { mesh: false, points: false, texture: false, normals: false, vertexColors: false };
    this.modelRoot.traverse((obj) => {
      if (obj.isMesh) {
        caps.mesh = true;
        const mats = Array.isArray(obj.material) ? obj.material : [obj.material];
        for (const m of mats) {
          if (!m) continue;
          if (m.map) caps.texture = true;
          if (obj.geometry?.attributes.normal) caps.normals = true;
          if (obj.geometry?.attributes.color) caps.vertexColors = true;
        }
      }
      if (obj.isPoints) caps.points = true;
    });
    this.capabilities = caps;
  }

  computeStats() {
    const stats = { triangles: 0, vertices: 0, drawCalls: 0, materials: 0, textures: 0 };
    const matSet = new Set();
    const texSet = new Set();

    this.modelRoot.traverse((obj) => {
      if (obj.geometry?.attributes.position) {
        stats.vertices += obj.geometry.attributes.position.count;
      }
      if (obj.isMesh) {
        stats.drawCalls++;
        const idx = obj.geometry.index;
        const pos = obj.geometry.attributes.position;
        if (idx) stats.triangles += idx.count / 3;
        else if (pos) stats.triangles += pos.count / 3;

        const mats = Array.isArray(obj.material) ? obj.material : [obj.material];
        for (const m of mats) {
          if (!m) continue;
          matSet.add(m);
          if (m.map) texSet.add(m.map);
        }
      }
    });

    stats.materials = matSet.size;
    stats.textures = texSet.size;
    this.stats = stats;
  }

  // ── Material mode switching ───────────────────────────────────────────────

  copyMaterialProps(src, dst) {
    const props = [
      "alphaMap",
      "alphaTest",
      "blendDst",
      "blendEquation",
      "blendSrc",
      "blending",
      "colorWrite",
      "depthFunc",
      "depthTest",
      "depthWrite",
      "name",
      "opacity",
      "polygonOffset",
      "polygonOffsetFactor",
      "polygonOffsetUnits",
      "premultipliedAlpha",
      "side",
      "toneMapped",
      "transparent",
      "visible",
      "wireframe",
    ];
    for (const p of props) {
      if (p in src) dst[p] = src[p];
    }
    dst.color.copy(src.color);
    dst.map = src.map;
    if (src.userData) dst.userData = { ...src.userData };
    dst.needsUpdate = true;
  }

  applyRenderMode() {
    this.modelRoot.traverse((obj) => {
      if (obj.isMesh && obj.material) {
        const mats = Array.isArray(obj.material) ? obj.material : [obj.material];
        const updated = [];

        for (const m of mats) {
          if (!m) {
            updated.push(m);
            continue;
          }

          m.wireframe = this.state.wireframe;
          m.side = this.state.backfaces ? THREE.DoubleSide : THREE.FrontSide;
          m.needsUpdate = true;

          if (this.state.lighting && m.type === "MeshBasicMaterial") {
            const r = new THREE.MeshStandardMaterial();
            this.copyMaterialProps(m, r);
            r.needsUpdate = true;
            updated.push(r);
          } else if (!this.state.lighting && m.type !== "MeshBasicMaterial") {
            const r = new THREE.MeshBasicMaterial();
            this.copyMaterialProps(m, r);
            r.needsUpdate = true;
            updated.push(r);
          } else {
            updated.push(m);
          }
        }

        obj.material = Array.isArray(obj.material) ? updated : updated[0];
      }

      if (obj.isPoints && obj.material) {
        obj.material.size = this.state.pointsSize;
        obj.material.needsUpdate = true;
      }
    });
  }

  applyTextures() {
    this.modelRoot.traverse((obj) => {
      if (!obj.material) return;
      const mats = Array.isArray(obj.material) ? obj.material : [obj.material];
      for (const m of mats) {
        if (!m) continue;
        if (!m.userData.originalMap) m.userData.originalMap = m.map;
        m.map = this.state.textures ? m.userData.originalMap : null;
        m.needsUpdate = true;
      }
    });
  }

  // ── Config change helper ─────────────────────────────────────────────────

  _onConfigChange() {
    savePrefs(this.state);
    this._persistUrlState();
  }

  // ── Camera state serialisation ────────────────────────────────────────────

  _getCameraState() {
    return {
      position: [
        Math.round(this.camera.position.x * 1e4) / 1e4,
        Math.round(this.camera.position.y * 1e4) / 1e4,
        Math.round(this.camera.position.z * 1e4) / 1e4,
      ],
      target: [
        Math.round(this.controls.target.x * 1e4) / 1e4,
        Math.round(this.controls.target.y * 1e4) / 1e4,
        Math.round(this.controls.target.z * 1e4) / 1e4,
      ],
      up: [Math.round(this.camera.up.x * 1e4) / 1e4, Math.round(this.camera.up.y * 1e4) / 1e4, Math.round(this.camera.up.z * 1e4) / 1e4],
      near: Math.round(this.camera.near * 1e4) / 1e4,
      far: Math.round(this.camera.far * 1e4) / 1e4,
    };
  }

  _setCameraState(state) {
    if (!state || !state.position) return false;
    const [px, py, pz] = state.position;
    const [tx, ty, tz] = state.target || [0, 0, 0];
    this.camera.position.set(px, py, pz);
    this.controls.target.set(tx, ty, tz);
    if (state.up) {
      this.camera.up.set(state.up[0], state.up[1], state.up[2]);
    }
    if (state.near !== undefined) this.camera.near = state.near;
    if (state.far !== undefined) this.camera.far = state.far;
    this.camera.lookAt(tx, ty, tz);
    this.camera.updateProjectionMatrix();

    // Sync controls via setCamera (the standard Three.js way for
    // ArcballControls), but then explicitly re-assert target + lookAt
    // so the pixel-precise saved target is preserved through saveState.
    this.controls.setCamera(this.camera);
    this.controls.target.set(tx, ty, tz);
    this.camera.lookAt(tx, ty, tz);
    this.controls.saveState();
    // FIXME: Always looks at the target's center, even when the URL saved different target...
    return true;
  }

  _persistUrlState() {
    if (!this._projectName || !this._filePath) return;
    const cam = this._getCameraState();
    const cfg = { ...this.state };
    // Combine into a single config blob: {cam: ..., config: ...}
    const blob = { cam, config: cfg };
    const b64 = btoa(JSON.stringify(blob));
    updateViewerUrl(this._projectName, this._filePath, b64);
  }

  // ── Home camera ───────────────────────────────────────────────────────────

  homeCamera(forceDir) {
    if (!this.modelRoot) return;

    // If an initial camera state from the URL was queued, use that instead.
    // Do NOT persist afterwards — the URL already contains the correct state;
    // re-saving after controls.setCamera would compound rounding / precision
    // errors on every reload.
    if (!forceDir && this._queuedCameraState) {
      const restored = this._setCameraState(this._queuedCameraState);
      this._queuedCameraState = null;
      if (restored) {
        return;
      }
    }

    const box = new THREE.Box3().setFromObject(this.modelRoot);
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());

    const radius = Math.max(size.x, size.y, size.z) * 0.5;
    const fov = this.camera.fov * (Math.PI / 180);
    let dist = (radius / Math.tan(fov / 2)) * 1.8;

    const dir = new THREE.Vector3();
    if (forceDir) {
      dir.set(1, 1, 1).normalize();
      this.camera.up.set(0, 1, 0);
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
    this.controls.setCamera(this.camera);
    this.controls.saveState();

    this._persistUrlState();
  }

  // ── Lighting update (each frame) ──────────────────────────────────────────

  updateLighting() {
    const toTarget = new THREE.Vector3().copy(this.camera.position).sub(this.controls.target);
    const radius = toTarget.length();
    if (radius < 1e-10) return;

    const forward = toTarget.clone().normalize();
    let right = new THREE.Vector3().crossVectors(forward, this.camera.up);
    if (right.lengthSq() < 1e-10) right.set(1, 0, 0);
    else right.normalize();
    const up = new THREE.Vector3().crossVectors(right, forward).normalize();

    const azimuth = (this.state.lightAzimuth || 0) * (Math.PI / 180);
    const elevation = (this.state.lightElevation || 0) * (Math.PI / 180);

    const qAz = new THREE.Quaternion().setFromAxisAngle(up, azimuth);
    const rightRot = right.clone().applyQuaternion(qAz);
    const qEl = new THREE.Quaternion().setFromAxisAngle(rightRot, elevation);
    const dir = forward.clone().applyQuaternion(new THREE.Quaternion().multiplyQuaternions(qEl, qAz));

    const lightPos = this.controls.target.clone().add(dir.clone().multiplyScalar(radius));
    this.dirLight.position.copy(lightPos);
    this.dirLight.target.position.copy(this.controls.target);
    this.dirLight.updateWorldMatrix(true, false);
    this.dirLight.target.updateWorldMatrix(true, false);
  }

  // ── Animation loop ────────────────────────────────────────────────────────

  animate = () => {
    requestAnimationFrame(this.animate);
    this.updateLighting();
    this.renderer.render(this.scene, this.camera);
  };
}

// ── Launch helper (called by Rust backend) ──────────────────────────────────

export async function launchGlbViewer(b64, filename) {
  const container = document.createElement("div");
  container.id = "viewer3d-container";
  Object.assign(container.style, { position: "fixed", inset: "0", zIndex: "9999", background: "#111318" });

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
      new GLTFLoader().load(url, resolve, undefined, reject);
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
    throw err;
  } finally {
    URL.revokeObjectURL(url);
  }
}

// ── Embedded viewer (mount in an existing container) ─────────────────────────

/**
 * Mount the 3D viewer inside a given container element.
 *
 * @param {HTMLElement} container  - The DOM element to render into.
 * @param {object}       options
 * @param {string}       options.projectName  - Project name (for URL state).
 * @param {string}       options.filePath     - Output file path (for URL state).
 * @param {string}       options.glbBase64    - Base64-encoded GLB data.
 * @param {object|null}  options.initialCamera - { position: [x,y,z], target: [x,y,z] }
 * @param {object|null}  options.initialConfig - Partial config to merge over defaults.
 * @param {function|null} options.onStateChange - Called with { camera, config } on changes.
 * @returns {Promise<Viewer3D>} The viewer instance.
 */
export async function mountViewer3d(container, options) {
  const { projectName = "", filePath = "", glbBase64, initialCamera = null, initialConfig = null, onStateChange = null } = options || {};

  if (!glbBase64) throw new Error("mountViewer3d: glbBase64 is required");

  const binary = Uint8Array.from(atob(glbBase64), (c) => c.charCodeAt(0));
  const blob = new Blob([binary], { type: "model/gltf-binary" });
  const url = URL.createObjectURL(blob);

  try {
    const gltf = await new Promise((resolve, reject) => {
      new GLTFLoader().load(url, resolve, undefined, reject);
    });

    const viewer = new Viewer3D(container, {
      projectName,
      filePath,
      initialCamera,
      initialConfig,
      onStateChange,
    });
    viewer.setModel(gltf.scene, initialCamera);

    return viewer;
  } catch (err) {
    throw err;
  } finally {
    URL.revokeObjectURL(url);
  }
}
