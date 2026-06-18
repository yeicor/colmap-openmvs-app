/**
 * Shared utility functions for the 3D viewer.
 */

/** Lightweight debounce helper */
export function debounce<T extends (...args: unknown[]) => void>(fn: T, ms: number): (...args: Parameters<T>) => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return (...args: Parameters<T>) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  };
}

/** Default viewer configuration state.
 *
 * Background colour is NOT part of the config — it is always derived from the
 * app theme at the point of use (see `_updateThemeBackground` in the viewer).
 */
export interface ConfigState {
  textures: boolean;
  wireframe: boolean;
  backfaces: boolean;
  lighting: boolean;
  lightAzimuth: number;
  lightElevation: number;
  pointsSize: number;
  toneMapping: boolean;
  exposure: number;
}

export const DEFAULT_STATE: ConfigState = {
  textures: true,
  wireframe: false,
  backfaces: false,
  lighting: true,
  lightAzimuth: 0,
  lightElevation: 0,
  pointsSize: 1.5,
  toneMapping: true,
  exposure: 1.0,
};

/** Camera state serialization */
export interface CameraState {
  position: [number, number, number];
  target: [number, number, number];
  up?: [number, number, number];
  near?: number;
  far?: number;
}

/** Aggregate state blob for URL persistence */
export interface StateBlob {
  cam: CameraState;
  config: ConfigState;
}
