/**
 * Entry-point functions for mounting the 3D viewer in various contexts.
 */
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { Viewer3D } from "./viewer3d-class";
import { CameraState, ConfigState } from "./utils";

// Type for the state change callback
type StateChangeCallback = (state: { camera: CameraState; config: ConfigState }) => void;

/** Options for mountViewer3d */
export interface MountOptions {
  projectName?: string;
  filePath?: string;
  glbBase64?: string;
  glbUrl?: string;
  initialCamera?: CameraState | null;
  initialConfig?: Partial<ConfigState> | null;
  onStateChange?: StateChangeCallback | null;
}

/**
 * Mount the 3D viewer inside a given container element.
 *
 * @param container - The DOM element to render into.
 * @param options - Configuration options.
 * @returns The viewer instance.
 */
export async function mountViewer3d(container: HTMLElement, options: MountOptions = {}): Promise<Viewer3D> {
  const { projectName = "", filePath = "", glbBase64, glbUrl, initialCamera = null, initialConfig = null, onStateChange = null } = options;

  if (!glbBase64 && !glbUrl) {
    throw new Error("mountViewer3d: glbBase64 or glbUrl is required");
  }

  let url: string;
  let ownsUrl = false;
  if (glbUrl) {
    url = glbUrl;
  } else {
    const binary = Uint8Array.from(atob(glbBase64!), (c) => c.charCodeAt(0));
    const blob = new Blob([binary], { type: "model/gltf-binary" });
    url = URL.createObjectURL(blob);
    ownsUrl = true;
  }

  try {
    const gltf = await new Promise<{ scene: THREE.Group }>((resolve, reject) => {
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
  } finally {
    if (ownsUrl) URL.revokeObjectURL(url);
  }
}

/**
 * Launch a full-screen GLB viewer as a modal overlay.
 *
 * @param b64 - Base64-encoded GLB data.
 * @param _filename - Original filename (for display).
 * @returns The viewer instance.
 */
export async function launchGlbViewer(b64: string, _filename: string): Promise<Viewer3D> {
  // Read the current theme background the same way _updateThemeBackground does.
  const isDark = getComputedStyle(document.documentElement).getPropertyValue("--dark").trim() === "initial";
  const bgColor = isDark ? "#111318" : "#e8ecf0";

  const container = document.createElement("div");
  container.id = "viewer3d-container";
  Object.assign(container.style, {
    position: "fixed",
    inset: "0",
    zIndex: "9999",
    background: bgColor,
  } as CSSStyleDeclaration);

  const closeBtn = document.createElement("button");
  closeBtn.textContent = "\u00d7";
  Object.assign(closeBtn.style, {
    position: "fixed" as string,
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
  } as CSSStyleDeclaration);

  document.body.append(container, closeBtn);

  const binary = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  const blob = new Blob([binary], { type: "model/gltf-binary" });
  const url = URL.createObjectURL(blob);

  try {
    const gltf = await new Promise<{ scene: THREE.Group }>((resolve, reject) => {
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
