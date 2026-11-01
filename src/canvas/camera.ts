import { markDirty } from "./render-state";

export const CAMERA_EASE_MS = 180;
/** Easing between two workflows' coordinate spaces would sweep across unrelated space, so a freshly loaded workflow is framed instantly. */
export const INSTANT_AFTER_REPLACE_MS = 500;

export interface CameraTarget { panX: number; panY: number; zoom: number }

interface CameraAnim { from: CameraTarget; to: CameraTarget; start: number | null }

export interface CameraHost extends CameraTarget {
  camAnim?: CameraAnim | null;
  mapsReplacedAt?: number;
  onZoomChange?: ((zoom: number) => void) | null;
  onViewportChange?: (() => void) | null;
}

/** True when motion should be skipped. Without matchMedia the preference is unknown, so motion is skipped. */
export function prefersReducedMotion(): boolean {
  try {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return true;
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return true;
  }
}

export function cancelCamera(host: CameraHost): void {
  if (host.camAnim) host.camAnim = null;
}

/** Moves the camera to `to`: eased, or at once when `instant`, under reduced motion, or just after a workflow was loaded. */
export function moveCamera(host: CameraHost, to: CameraTarget, instant = false): void {
  cancelCamera(host);
  const sinceReplace = typeof host.mapsReplacedAt === "number" ? performance.now() - host.mapsReplacedAt : Infinity;
  const same = to.panX === host.panX && to.panY === host.panY && to.zoom === host.zoom;
  if (instant || same || sinceReplace < INSTANT_AFTER_REPLACE_MS || prefersReducedMotion()) {
    host.panX = to.panX; host.panY = to.panY; host.zoom = to.zoom;
    return;
  }
  host.camAnim = { from: { panX: host.panX, panY: host.panY, zoom: host.zoom }, to: { ...to }, start: null };
  markDirty();
}

/** Advances the animation to frame time `ts`. Returns true while it is still running. */
export function stepCamera(host: CameraHost, ts: number): boolean {
  const a = host.camAnim;
  if (!a) return false;
  if (a.start === null) a.start = ts;
  const t = Math.min(1, Math.max(0, (ts - a.start) / CAMERA_EASE_MS));
  const k = 1 - Math.pow(1 - t, 3);
  const zoomBefore = host.zoom;
  host.panX = a.from.panX + (a.to.panX - a.from.panX) * k;
  host.panY = a.from.panY + (a.to.panY - a.from.panY) * k;
  host.zoom = a.from.zoom * Math.pow(a.to.zoom / a.from.zoom, k);
  if (host.zoom !== zoomBefore) host.onZoomChange?.(host.zoom);
  if (t >= 1) {
    host.camAnim = null;
    host.onViewportChange?.();
    return false;
  }
  return true;
}
