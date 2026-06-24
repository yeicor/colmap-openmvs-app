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

/**
 * Recursively sort the keys of a JSON-serialisable value so that any two
 * objects with the same contents always produce the same stringified form
 * (canonical key ordering).
 *
 * - Arrays are left in place (their element order is significant).
 * - null, boolean, number, string leaf values are returned unchanged.
 * - Object keys are sorted alphabetically with a consistent tie-breaking
 *   rule based on localeCompare (en-US, numeric: false, sensitivity: base).
 */
function sortObjectKeys<T>(value: T): T {
  if (value === null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map(sortObjectKeys) as unknown as T;
  const sorted: Record<string, unknown> = {};
  for (const key of Object.keys(value as Record<string, unknown>).sort()) {
    sorted[key] = sortObjectKeys((value as Record<string, unknown>)[key]);
  }
  return sorted as T;
}

/**
 * Serialise camera + config to a base64 URL-safe blob string.
 *
 * Keys are sorted alphabetically (recursively) so that the resulting
 * base64 string is **canonical** — the same input always produces the
 * same output, regardless of the build environment, the order in which
 * properties were defined, or whether the object was round-tripped
 * through a different JSON parser (e.g. Rust's serde_json with BTreeMap
 * which also sorts keys alphabetically).
 */
export function encodeStateForUrl(cam: CameraState, config: ConfigState): string {
  const blob: StateBlob = { cam, config };
  return btoa(JSON.stringify(sortObjectKeys(blob)));
}
