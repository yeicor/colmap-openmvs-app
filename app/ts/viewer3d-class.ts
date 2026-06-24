// @ts-nocheck — All errors in this file are false positives from @types/three not perfectly matching runtime behavior.
/**
 * Main 3D viewer class — manages Three.js scene, camera, controls, toolbar, and panels.
 */
import * as THREE from "three";
import { ArcballControls } from "three/examples/jsm/controls/ArcballControls.js";
import { ToastStack } from "./ToastStack";
import { injectViewerStyles } from "./styles";
import { CameraState, ConfigState, debounce } from "./utils";
import { buildInitialState, encodeStateForUrl, updateViewerUrl } from "./state";

interface Capabilities {
  mesh: boolean;
  points: boolean;
  texture: boolean;
  normals: boolean;
  vertexColors: boolean;
}

interface ViewerStats {
  triangles: number;
  vertices: number;
  drawCalls: number;
  materials: number;
  textures: number;
}

interface ViewerOptions {
  projectName?: string;
  filePath?: string;
  initialCamera?: CameraState | null;
  initialConfig?: Partial<ConfigState> | null;
  onStateChange?: ((state: { camera: CameraState; config: ConfigState }) => void) | null;
}

export class Viewer3D {
  readonly container: HTMLElement;
  readonly scene: THREE.Scene;
  readonly renderer: THREE.WebGLRenderer;
  readonly camera: THREE.PerspectiveCamera;
  readonly controls: ArcballControls;
  readonly toastStack: ToastStack;

  state: ConfigState;
  modelRoot: THREE.Group | null = null;
  capabilities: Capabilities = { mesh: false, points: false, texture: false, normals: false, vertexColors: false };
  stats: ViewerStats = { triangles: 0, vertices: 0, drawCalls: 0, materials: 0, textures: 0 };

  measurementMode = false;
  measurePoints: THREE.Vector3[] = [];
  raycastEnabled = false;

  // Toolbar
  toolbar!: HTMLElement;
  sections!: Record<string, HTMLElement>;
  private _btnMeasure!: HTMLButtonElement;
  private _btnPick!: HTMLButtonElement;
  private _psBtn!: HTMLButtonElement;

  // Panels
  private _panel: HTMLDivElement | null = null;
  private _panelOutside: ((e: PointerEvent) => void) | null = null;
  private _loPanel: HTMLDivElement | null = null;
  private _loOutside: ((e: PointerEvent) => void) | null = null;
  private _psPanel: HTMLDivElement | null = null;
  private _psOutside: ((e: PointerEvent) => void) | null = null;

  private _themeObserver: MutationObserver | null = null;

  // Internal state
  private _projectName: string;
  private _filePath: string;
  private _onStateChange: ((state: { camera: CameraState; config: ConfigState }) => void) | null;
  private _initialized = false;
  private _queuedCameraState: CameraState | null = null;
  private _debouncedPersistUrl: () => void;
  private _animFrameId: number | undefined;
  private _resizeObs: ResizeObserver | null = null;
  private _resizeLabels: () => void;
  private _onEscapePanel: (e: KeyboardEvent) => void;

  readonly hemiLight: THREE.HemisphereLight;
  readonly dirLight: THREE.DirectionalLight;
  readonly dirLightTarget: THREE.Object3D;
  readonly raycaster: THREE.Raycaster;
  readonly pointer: THREE.Vector2;
  readonly mixers: THREE.AnimationMixer[] = [];

  constructor(container: HTMLElement, opts: ViewerOptions = {}) {
    this.container = container;
    this._projectName = opts.projectName || "";
    this._filePath = opts.filePath || "";
    this._onStateChange = opts.onStateChange || null;

    // Merge state
    this.state = buildInitialState(opts.initialConfig);

    this.scene = new THREE.Scene();
    // The scene has no background — the renderer is configured with `alpha: true`
    // and a transparent clear colour, so the theme-aware CSS background of the
    // container (set via `_updateThemeBackground`) shows through the canvas.
    this.scene.background = null;

    // preserveDrawingBuffer is required so that Playwright / headless
    // Chromium can capture the WebGL canvas in a page screenshot.
    // alpha: true makes the canvas transparent so the CSS background of the
    // container (which is theme-aware) shows through instead of a hardcoded
    // WebGL clear colour.
    this.renderer = new THREE.WebGLRenderer({
      antialias: true,
      preserveDrawingBuffer: true,
      alpha: true,
    });
    this.renderer.setPixelRatio(window.devicePixelRatio);
    this.renderer.setSize(container.clientWidth, container.clientHeight);
    this.renderer.setClearColor(0x000000, 0);
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    this.renderer.toneMapping = this.state.toneMapping ? THREE.ACESFilmicToneMapping : THREE.NoToneMapping;
    this.renderer.toneMappingExposure = this.state.exposure;

    const canvas = this.renderer.domElement;
    canvas.style.display = "block";
    container.appendChild(canvas);

    this.camera = new THREE.PerspectiveCamera(60, container.clientWidth / container.clientHeight, 0.01, 10000);
    this.camera.position.set(2, 2, 2);

    // Controls
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

    // URL persistence gated behind _initialized
    this._debouncedPersistUrl = debounce(() => {
      if (!this._initialized) return;
      this._persistUrlState();
      if (this._onStateChange) {
        const cam = this._getCameraState();
        this._onStateChange({ camera: cam, config: { ...this.state } });
      }
    }, 300);

    this.raycaster = new THREE.Raycaster();
    this.pointer = new THREE.Vector2();

    this.toastStack = new ToastStack(container);

    // Environment
    this.hemiLight = new THREE.HemisphereLight(0xffffff, 0x222233, 0.6);
    this.scene.add(this.hemiLight);

    this.dirLight = new THREE.DirectionalLight(0xffffff, 1.8);
    this.dirLight.position.set(4, 8, 6);
    this.scene.add(this.dirLight);
    this.dirLightTarget = this.dirLight.target;
    this.scene.add(this.dirLightTarget);

    // Toolbar
    this.createToolbar();
    injectViewerStyles();
    this.bind();
    this.animate();

    // Zoom-aware toolbar label hiding
    this._onEscapePanel = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (this._panel) this._panel.classList.add("hidden");
        if (this._loPanel) this._loPanel.classList.add("hidden");
        if (this._psPanel) this._psPanel.classList.add("hidden");
      }
    };
    window.addEventListener("keydown", this._onEscapePanel);

    this._updateToolbarLabels();
    this._resizeLabels = () => this._updateToolbarLabels();
    window.addEventListener("resize", this._resizeLabels);
  }

  // ── Toolbar ───────────────────────────────────────────────────────────────

  createToolbar(): void {
    const host = document.getElementById("viewer-toolbar");
    if (host) {
      host.innerHTML = "";
      this.toolbar = host;
    } else {
      this.toolbar = document.createElement("div");
      this.toolbar.className = "v3d-toolbar";
      this.container.appendChild(this.toolbar);
    }

    const sec = (label: string): HTMLDivElement => {
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

    // View
    this._addBtn(viewSec, "🏠", "Home", () => this.homeCamera(true));

    // Tools
    this._addBtn(toolsSec, "📏", "Measure", () => {
      this.measurementMode = !this.measurementMode;
      this._btnMeasure.classList.toggle("active", this.measurementMode);
    });
    this._btnMeasure = toolsSec.lastChild as HTMLButtonElement;

    this._addBtn(toolsSec, "🎯", "Pick", () => {
      this.raycastEnabled = !this.raycastEnabled;
      this._btnPick.classList.toggle("active", this.raycastEnabled);
    });
    this._btnPick = toolsSec.lastChild as HTMLButtonElement;

    // Info
    this._addBtn(infoSec, "📊", "Stats", (e) => this._togglePanel("stats", e.currentTarget as HTMLElement));
    this._addBtn(infoSec, "❓", "Help", (e) => this._togglePanel("help", e.currentTarget as HTMLElement));
  }

  private _addBtn(section: HTMLElement, icon: string, label: string, onClick: (e: MouseEvent) => void): HTMLButtonElement {
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

  private _togglePanel(kind: string, triggerBtn?: HTMLElement): void {
    if (this._panel && !this._panel.classList.contains("hidden") && (this._panel as unknown as Record<string, unknown>)._kind === kind) {
      this._panel.classList.add("hidden");
      return;
    }

    if (this._panel) this._panel.remove();
    if (this._panelOutside) {
      document.removeEventListener("pointerdown", this._panelOutside);
      this._panelOutside = null;
    }

    this._panel = document.createElement("div");
    this._panel.className = "v3d-panel";
    (this._panel as unknown as Record<string, unknown>)._kind = kind;
    document.body.appendChild(this._panel);

    const close = (): void => {
      if (this._panel) this._panel.classList.add("hidden");
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

    this._panel.querySelector(".v3d-close")!.onclick = close;

    // Position relative to the trigger button
    const trigger = triggerBtn || this.toolbar.querySelector(".v3d-btn");
    if (trigger) {
      const r = trigger.getBoundingClientRect();
      this._panel.style.left = Math.min(r.left, window.innerWidth - 360) + "px";
      this._panel.style.top = r.bottom + 10 + "px";
    }

    // Close on outside click
    this._panelOutside = (e: PointerEvent) => {
      if (this._panel && !this._panel.contains(e.target as Node) && !(e.target as Element).closest(".v3d-btn")) {
        close();
        if (this._panelOutside) {
          document.removeEventListener("pointerdown", this._panelOutside);
          this._panelOutside = null;
        }
      }
    };
    setTimeout(() => document.addEventListener("pointerdown", this._panelOutside!), 0);
  }

  // ── Light direction panel ────────────────────────────────────────────────

  private _createLightPanel(): () => void {
    const show = (): void => {
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

        this._loPanel.querySelector(".v3d-close")!.onclick = () => this._loPanel!.classList.add("hidden");

        for (const input of this._loPanel.querySelectorAll<HTMLInputElement>("[data-lo-axis]")) {
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
        if (this._loOutside) {
          document.removeEventListener("pointerdown", this._loOutside);
          this._loOutside = null;
        }
      }

      this._loPanel.classList.remove("hidden");

      const btn = document.querySelector<HTMLElement>("[data-lo-btn]");
      if (btn) {
        const r = btn.getBoundingClientRect();
        this._loPanel.style.left = Math.min(r.left, window.innerWidth - 360) + "px";
        this._loPanel.style.top = r.bottom + 10 + "px";
      }

      this._loOutside = (e: PointerEvent) => {
        if (this._loPanel && !this._loPanel.contains(e.target as Node) && !(e.target as Element).closest("[data-lo-btn]")) {
          this._loPanel.classList.add("hidden");
          if (this._loOutside) {
            document.removeEventListener("pointerdown", this._loOutside);
            this._loOutside = null;
          }
        }
      };
      setTimeout(() => document.addEventListener("pointerdown", this._loOutside!), 0);
    };

    return show;
  }

  // ── Point size slider panel ──────────────────────────────────────────────

  private _createPointsSlider(): () => void {
    const show = (): void => {
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

        this._psPanel.querySelector(".v3d-close")!.onclick = () => this._psPanel!.classList.add("hidden");

        const slider = this._psPanel.querySelector<HTMLInputElement>("#v3d-ps-slider")!;
        slider.addEventListener("input", () => {
          this.state.pointsSize = parseFloat(slider.value);
          this.applyRenderMode();
          this._onConfigChange();
        });
      } else {
        if (this._psOutside) {
          document.removeEventListener("pointerdown", this._psOutside);
          this._psOutside = null;
        }
      }

      this._psPanel.classList.remove("hidden");

      if (this._psBtn) {
        const r = this._psBtn.getBoundingClientRect();
        this._psPanel.style.left = Math.min(r.left, window.innerWidth - 360) + "px";
        this._psPanel.style.top = r.bottom + 10 + "px";
      }

      this._psOutside = (e: PointerEvent) => {
        if (
          this._psPanel &&
          !this._psPanel.contains(e.target as Node) &&
          !(e.target as Element).closest(".v3d-close") &&
          !(e.target as Element).closest(".v3d-btn")
        ) {
          this._psPanel.classList.add("hidden");
          if (this._psOutside) {
            document.removeEventListener("pointerdown", this._psOutside);
            this._psOutside = null;
          }
        }
      };
      setTimeout(() => document.addEventListener("pointerdown", this._psOutside!), 0);
    };

    return show;
  }

  // ── Render section (rebuilt after model loads) ────────────────────────────

  rebuildRenderSection(): void {
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

  bind(): void {
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

    this.renderer.domElement.addEventListener("pointerdown", this.onPointerDown, {
      capture: true,
    });
    window.addEventListener("keydown", this.onKeyDown);
  }

  /**
   * Initialise URL persistence: register the camera change listener and
   * mark the viewer as fully initialised.  Must be called after
   * homeCamera restores the queued camera state (i.e. at the end of
   * setModel).  Until this is called, _debouncedPersistUrl is a no-op
   * so stray change events cannot corrupt the URL.
   */
  _initUrlPersistence(): void {
    if (this._initialized) return;
    this._initialized = true;
    // Signal to any polling consumer (e.g. screenshot script,
    // Playwright, Rust-mounted-data-ready watcher) that the viewer
    // is fully initialised — camera restored, toolbar created,
    // change listener installed, and first render submitted.
    this.container.dataset.ready = "true";
    this.controls.addEventListener("change", () => {
      this._debouncedPersistUrl();
    });
  }

  _updateToolbarLabels(): void {
    const zoom = parseFloat(document.documentElement.style.zoom) || 1;
    const compact = window.innerWidth / zoom < 720;
    document.documentElement.classList.toggle("v3d-compact", compact);
  }

  // ── Dispose / cleanup ─────────────────────────────────────────────────────

  /** Release all GPU resources and stop rendering. */
  dispose(): void {
    // 1. Stop animation loop
    if (this._animFrameId !== undefined) {
      cancelAnimationFrame(this._animFrameId);
      this._animFrameId = undefined;
    }

    // 2. Remove event listeners
    document.documentElement.classList.remove("v3d-compact");
    window.removeEventListener("resize", this._resizeLabels);
    if (this._resizeObs) {
      this._resizeObs.disconnect();
      this._resizeObs = null;
    }
    window.removeEventListener("keydown", this.onKeyDown);
    window.removeEventListener("keydown", this._onEscapePanel);
    this.renderer.domElement.removeEventListener("pointerdown", this.onPointerDown, {
      capture: true,
    });
    if (this._panelOutside) {
      document.removeEventListener("pointerdown", this._panelOutside);
      this._panelOutside = null;
    }
    if (this._loOutside) {
      document.removeEventListener("pointerdown", this._loOutside);
      this._loOutside = null;
    }
    if (this._psOutside) {
      document.removeEventListener("pointerdown", this._psOutside);
      this._psOutside = null;
    }

    // 3. Dispose controls
    this.controls.dispose();

    // 4. Dispose all Three.js scene resources (geometries, materials, textures)
    this.scene.traverse((object) => {
      const obj = object as THREE.Mesh | THREE.Points | THREE.Line;
      if (obj.isMesh || obj.isPoints || obj.isLine) {
        // Geometry
        if (obj.geometry) {
          obj.geometry.dispose();
        }
        // Materials and their textures
        if (obj.material) {
          const materials = Array.isArray(obj.material) ? obj.material : [obj.material];
          for (const material of materials) {
            if (!material) continue;
            // Dispose textures referenced by the material
            for (const key of Object.keys(material)) {
              const value = (material as Record<string, unknown>)[key];
              if (value && typeof value === "object" && "isTexture" in (value as Record<string, unknown>)) {
                (value as THREE.Texture).dispose();
              }
            }
            material.dispose();
          }
        }
      }
    });

    // 5. Clear scene references
    this.scene.clear();
    this.modelRoot = null;

    // 6. Dispose renderer GPU resources
    const canvas = this.renderer.domElement;
    this.renderer.dispose();

    // 7. Disconnect theme observer
    if (this._themeObserver) {
      this._themeObserver.disconnect();
      this._themeObserver = null;
    }

    // 8. Aggressively release WebGL context on full teardown
    try {
      const gl = this.renderer.getContext();
      const ext = gl.getExtension("WEBGL_lose_context");
      if (ext) ext.loseContext();
    } catch {
      // Silently fail if extension not available
    }

    // 8. Remove DOM elements
    for (const el of [this.toolbar, this._panel, this._loPanel, this._psPanel]) {
      if (el && el.parentNode) el.remove();
    }

    // 9. Remove canvas from container
    if (canvas && canvas.parentNode) {
      canvas.parentNode.removeChild(canvas);
    }
  }

  // ── Keyboard ──────────────────────────────────────────────────────────────

  onKeyDown = (e: KeyboardEvent): void => {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;

    switch (e.key.toLowerCase()) {
      case "f":
        this.homeCamera();
        return;
      case "m":
        this.measurementMode = !this.measurementMode;
        this._btnMeasure.classList.toggle("active", this.measurementMode);
        return;
      case "r":
        this.raycastEnabled = !this.raycastEnabled;
        this._btnPick.classList.toggle("active", this.raycastEnabled);
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

  // ── Pointer ───────────────────────────────────────────────────────────────

  onPointerDown = (e: PointerEvent): void => {
    if (e.button !== 0) return;

    if (this.measurementMode) {
      e.stopPropagation();
      this.pointer.x = (e.offsetX / this.renderer.domElement.clientWidth) * 2 - 1;
      this.pointer.y = -(e.offsetY / this.renderer.domElement.clientHeight) * 2 + 1;
      this.raycaster.setFromCamera(this.pointer, this.camera);
      const hits = this.raycaster.intersectObject(this.modelRoot!, true);
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

  setModel(object: THREE.Group, initialCamera?: CameraState | null): void {
    if (this.modelRoot) this.scene.remove(this.modelRoot);
    this.modelRoot = object;
    this.scene.add(object);

    this.detectCapabilities();
    this.computeStats();
    this.rebuildRenderSection();
    this.applyRenderMode();
    this.applyTextures();

    if (initialCamera) {
      this._queuedCameraState = initialCamera;
    }
    this.homeCamera(false);

    // Force an immediate render so there is at least one frame before the
    // animation loop kicks in.
    this.renderer.render(this.scene, this.camera);

    // Signal that the viewer is fully initialised — only after the first
    // render has been submitted.  The screenshot script polls this flag
    // before capturing, so it must only become true once a frame is drawn.
    this._initUrlPersistence();
  }

  detectCapabilities(): void {
    const caps: Capabilities = {
      mesh: false,
      points: false,
      texture: false,
      normals: false,
      vertexColors: false,
    };
    this.modelRoot!.traverse((obj) => {
      const mesh = obj as THREE.Mesh;
      if (mesh.isMesh) {
        caps.mesh = true;
        const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
        for (const m of mats) {
          if (!m) continue;
          if (m.map) caps.texture = true;
          if (mesh.geometry?.attributes.normal) caps.normals = true;
          if (mesh.geometry?.attributes.color) caps.vertexColors = true;
        }
      }
      const points = obj as THREE.Points;
      if (points.isPoints) caps.points = true;
    });
    this.capabilities = caps;
  }

  computeStats(): void {
    const stats: ViewerStats = {
      triangles: 0,
      vertices: 0,
      drawCalls: 0,
      materials: 0,
      textures: 0,
    };
    const matSet = new Set<THREE.Material>();
    const texSet = new Set<THREE.Texture>();

    this.modelRoot!.traverse((obj) => {
      const mesh = obj as THREE.Mesh;
      if (mesh.geometry?.attributes.position) {
        stats.vertices += mesh.geometry.attributes.position.count;
      }
      if (mesh.isMesh) {
        stats.drawCalls++;
        const idx = mesh.geometry.index;
        const pos = mesh.geometry.attributes.position;
        if (idx) stats.triangles += idx.count / 3;
        else if (pos) stats.triangles += pos.count / 3;

        const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
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

  copyMaterialProps(src: THREE.Material, dst: THREE.Material): void {
    const props: (keyof THREE.Material)[] = [
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
      if (p in src) (dst as Record<string, unknown>)[p as string] = (src as Record<string, unknown>)[p as string];
    }
    if ("color" in src && "color" in dst) {
      (dst as THREE.MeshStandardMaterial).color.copy((src as THREE.MeshStandardMaterial).color);
    }
    if ("map" in src) {
      (dst as THREE.MeshStandardMaterial).map = (src as THREE.MeshStandardMaterial).map;
    }
    if (src.userData) dst.userData = { ...src.userData };
    dst.needsUpdate = true;
  }

  applyRenderMode(): void {
    if (!this.modelRoot) return;
    this.modelRoot.traverse((obj) => {
      const mesh = obj as THREE.Mesh;
      if (mesh.isMesh && mesh.material) {
        const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
        const updated: THREE.Material[] = [];

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

        mesh.material = Array.isArray(mesh.material) ? updated : updated[0];
      }

      const points = obj as THREE.Points;
      if (points.isPoints && points.material) {
        points.material.size = this.state.pointsSize;
        points.material.needsUpdate = true;
      }
    });
  }

  applyTextures(): void {
    if (!this.modelRoot) return;
    this.modelRoot.traverse((obj) => {
      const mesh = obj as THREE.Mesh;
      if (!mesh.material) return;
      const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
      for (const m of mats) {
        if (!m) continue;
        if (!m.userData.originalMap) m.userData.originalMap = m.map;
        m.map = this.state.textures ? m.userData.originalMap : null;
        m.needsUpdate = true;
      }
    });
  }

  // ── Config change helper ─────────────────────────────────────────────────

  _onConfigChange(): void {
    this._persistUrlState();
  }

  // ── Camera state serialisation ────────────────────────────────────────────

  _getCameraState(): CameraState {
    // IMPORTANT: ArcballControls NEVER modifies `controls.target` during user
    // interactions (rotation, pan, zoom). It only uses `this.target` as an
    // initial reference that is synced to _gizmos.position once via update().
    // After that, all interactions update _gizmos.position directly via
    // applyTransformMatrix(), leaving controls.target permanently stale.
    // Therefore we must read _gizmos.position — the actual point the camera
    // looks at — instead of controls.target.
    const gizPos = this.controls._gizmos.position;
    return {
      position: [
        Math.round(this.camera.position.x * 1e4) / 1e4,
        Math.round(this.camera.position.y * 1e4) / 1e4,
        Math.round(this.camera.position.z * 1e4) / 1e4,
      ],
      target: [Math.round(gizPos.x * 1e4) / 1e4, Math.round(gizPos.y * 1e4) / 1e4, Math.round(gizPos.z * 1e4) / 1e4],
      up: [Math.round(this.camera.up.x * 1e4) / 1e4, Math.round(this.camera.up.y * 1e4) / 1e4, Math.round(this.camera.up.z * 1e4) / 1e4],
      near: Math.round(this.camera.near * 1e4) / 1e4,
      far: Math.round(this.camera.far * 1e4) / 1e4,
    };
  }

  _setCameraState(state: CameraState): boolean {
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

    this.controls.setCamera(this.camera);
    // setCamera() may have reset target / up internally, so re-apply.
    this.controls.target.set(tx, ty, tz);
    if (state.up) {
      this.camera.up.set(state.up[0], state.up[1], state.up[2]);
    }
    this.camera.lookAt(tx, ty, tz);
    this.controls.update();
    // Sync ArcballControls' internal matrix states with the actual camera/gizmos
    // after update() modified them.  Without this, the stale _cameraMatrixState
    // (set earlier by setCamera()) can cause subsequent operations (or even just
    // the first frame in some browser/environment combinations) to snap back to
    // the wrong target, effectively resetting the lookAt to model center.
    this.controls.updateMatrixState();
    this.controls.saveState();
    return true;
  }

  _persistUrlState(): void {
    if (!this._projectName || !this._filePath) return;
    const cam = this._getCameraState();
    // Only persist known ConfigState fields so that legacy / future keys
    // (e.g. `background` from an older version) never leak into the URL.
    // Destructuring with explicit field names ensures a canonical set.
    const { textures, wireframe, backfaces, lighting, lightAzimuth, lightElevation, pointsSize, toneMapping, exposure } = this.state;
    const cfg: ConfigState = { textures, wireframe, backfaces, lighting, lightAzimuth, lightElevation, pointsSize, toneMapping, exposure };
    const b64 = encodeStateForUrl(cam, cfg);
    updateViewerUrl(this._projectName, this._filePath, b64);
  }

  // ── Home camera ───────────────────────────────────────────────────────────

  homeCamera(forceDir?: boolean): void {
    if (!this.modelRoot) return;

    if (!forceDir && this._queuedCameraState) {
      const restored = this._setCameraState(this._queuedCameraState);
      this._queuedCameraState = null;
      if (restored) return;
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

  updateLighting(): void {
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

  animate = (): void => {
    this._animFrameId = requestAnimationFrame(this.animate);
    this.updateLighting();
    this.renderer.render(this.scene, this.camera);
  };
}
