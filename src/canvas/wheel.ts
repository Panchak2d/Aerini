export type ScrollMode = "auto" | "zoom" | "pan";

export const SCROLL_MODE_KEY = "aerini_scroll_wheel";

export function parseScrollMode(raw: string | null): ScrollMode {
  return raw === "zoom" || raw === "pan" ? raw : "auto";
}

export interface WheelLike {
  deltaX: number;
  deltaY: number;
  deltaMode: number;
  ctrlKey: boolean;
  shiftKey: boolean;
  wheelDelta?: number;
}

export type WheelAction =
  | { kind: "zoom"; factor: number }
  | { kind: "pan"; dx: number; dy: number };

const LINE_PX = 33;
/** One 100 px mouse notch zooms by 1/0.9, the step the wheel used before it was normalized. */
const K_WHEEL = Math.log(1 / 0.9) / 100;
const K_PINCH = 0.01;
const MAX_WHEEL_PX = 120;
const MAX_PINCH_PX = 25;
const TRACKPAD_STICKY_MS = 250;

/** Converts a wheel delta to pixels: lines are about 33 px, pages are the viewport height. */
export function normalizeDelta(delta: number, deltaMode: number, viewportHeight: number): number {
  if (deltaMode === 1) return delta * LINE_PX;
  if (deltaMode === 2) return delta * Math.max(1, viewportHeight);
  return delta;
}

function clamp(v: number, max: number): number {
  return Math.max(-max, Math.min(max, v));
}

function isNotch(e: WheelLike): boolean {
  if (e.deltaMode !== 0) return true;
  return typeof e.wheelDelta === "number" && e.wheelDelta !== 0 && e.wheelDelta % 120 === 0;
}

function looksLikeTrackpad(e: WheelLike): boolean {
  return e.deltaX !== 0 || !Number.isInteger(e.deltaY);
}

/**
 * Zoom factor for one wheel event, independent of how many events a gesture
 * is split into: the same total delta always gives the same total zoom.
 */
export function wheelZoomFactor(e: WheelLike, viewportHeight: number): number {
  const dy = normalizeDelta(e.deltaY, e.deltaMode, viewportHeight);
  if (e.ctrlKey && !isNotch(e)) return Math.exp(-clamp(dy, MAX_PINCH_PX) * K_PINCH);
  return Math.exp(-clamp(dy, MAX_WHEEL_PX) * K_WHEEL);
}

/** Decides whether a wheel event zooms or pans. Keeps the last trackpad sighting so one stray event does not flip the device guess. */
export class WheelInterpreter {
  mode: ScrollMode = "auto";
  private trackpadUntil = 0;

  interpret(e: WheelLike, viewportHeight: number, now: number): WheelAction {
    if (e.ctrlKey) return { kind: "zoom", factor: wheelZoomFactor(e, viewportHeight) };

    const dx = normalizeDelta(e.deltaX, e.deltaMode, viewportHeight);
    const dy = normalizeDelta(e.deltaY, e.deltaMode, viewportHeight);
    if (e.shiftKey) return { kind: "pan", dx: dx || dy, dy: 0 };

    if (this.mode === "pan") return { kind: "pan", dx, dy };
    if (this.mode === "zoom") return { kind: "zoom", factor: wheelZoomFactor(e, viewportHeight) };

    if (looksLikeTrackpad(e)) this.trackpadUntil = now + TRACKPAD_STICKY_MS;
    else if (isNotch(e)) this.trackpadUntil = 0;
    if (now < this.trackpadUntil) return { kind: "pan", dx, dy };
    return { kind: "zoom", factor: wheelZoomFactor(e, viewportHeight) };
  }
}
