/**
 * Viewer state helpers — URL persistence only (no localStorage).
 *
 * Background colour is always derived from the app theme at the point of
 * use via `getDefaultBackground()` and is intentionally excluded from
 * the persisted URL config.
 */
import { CameraState, ConfigState, DEFAULT_STATE, StateBlob } from "./utils";

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
export function updateViewerUrl(projectName: string, filePath: string, cfgBlobB64: string | null): void {
  const pipePath = filePath.replace(/\//g, "|");
  const encFile = encodeURIComponent(pipePath);
  // cfgBlobB64 comes from btoa() which produces standard base64 (+ / =).
  // Make it URL-safe for the route segment: + → -, / → _ while keeping
  // padding so the Rust URL_SAFE decoder can parse it.
  const safeCfg = cfgBlobB64 ? cfgBlobB64.replace(/\+/g, "-").replace(/\//g, "_") : "";
  let url: string;
  if (window.location.hash.startsWith("#/")) {
    url = "#/viewer/" + encodeURIComponent(projectName) + "/" + encFile + "/" + safeCfg;
  } else {
    url = "/viewer/" + encodeURIComponent(projectName) + "/" + encFile + "/" + safeCfg;
  }
  history.replaceState(null, "", url);
}

/** Build the initial state from defaults merged with the URL config blob.
 *
 * Background colour is always derived from the app theme at the point of
 * use and is intentionally excluded from the persisted URL config.
 */
export function buildInitialState(initialConfig?: Partial<ConfigState> | null): ConfigState {
  // Strip legacy `background` from a URL-deserialised config blob too
  const { background: _, ...initialClean } = (initialConfig || {}) as Partial<ConfigState & { background?: unknown }>;
  return {
    ...DEFAULT_STATE,
    ...initialClean,
  };
}

/** Serialise camera + config to a base64 URL-safe blob string. */
export function encodeStateForUrl(cam: CameraState, config: ConfigState): string {
  const blob: StateBlob = { cam, config };
  return btoa(JSON.stringify(blob));
}
