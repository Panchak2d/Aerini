import { visibleViewport, type VisibleViewport } from "./canvas/viewport";

export const MINIMAP_DEFAULT_W = 150;
export const MINIMAP_MIN_W = 120;
export const MINIMAP_ABS_MAX_W = 480;
export const MINIMAP_W_KEY = "aerini_minimap_w_v1";

const ASPECT_W = 5;
const ASPECT_H = 3;
const MAX_AREA_FRACTION_W = 0.4;
const MAX_AREA_FRACTION_H = 0.5;
const KEY_STEP = 10;
const KEY_PAGE_STEP = 40;
const DRAG_THRESHOLD_PX = 3;
const DRAG_BODY_CLASS = "resizing-minimap";

export function minimapHeightFor(width: number): number {
  return Math.round((width * ASPECT_H) / ASPECT_W);
}

/** The minimum wins when the area is too small to fit it. */
export function maxWidthForArea(area: VisibleViewport): number {
  const m = Math.min(
    MINIMAP_ABS_MAX_W,
    MAX_AREA_FRACTION_W * area.width,
    (MAX_AREA_FRACTION_H * area.height * ASPECT_W) / ASPECT_H,
  );
  return Number.isFinite(m) ? Math.max(MINIMAP_MIN_W, Math.floor(m)) : MINIMAP_MIN_W;
}

export function clampWidth(width: number, max: number): number {
  const n = Number.isFinite(width) ? Math.round(width) : MINIMAP_DEFAULT_W;
  return Math.max(MINIMAP_MIN_W, Math.min(max, n));
}

export function parseStoredWidth(raw: string | null): number {
  if (raw === null || raw.trim() === "") return MINIMAP_DEFAULT_W;
  const n = Number(raw);
  return Number.isFinite(n) ? clampWidth(n, MINIMAP_ABS_MAX_W) : MINIMAP_DEFAULT_W;
}

/** Next width for a handle keypress, or null when the key is not one the handle handles. */
export function keyStepWidth(key: string, current: number, max: number): number | null {
  switch (key) {
    case "ArrowRight":
    case "ArrowUp":     return clampWidth(current + KEY_STEP, max);
    case "ArrowLeft":
    case "ArrowDown":   return clampWidth(current - KEY_STEP, max);
    case "PageUp":      return clampWidth(current + KEY_PAGE_STEP, max);
    case "PageDown":    return clampWidth(current - KEY_PAGE_STEP, max);
    case "Home":        return MINIMAP_MIN_W;
    case "End":         return clampWidth(max, max);
    case "Enter":       return clampWidth(MINIMAP_DEFAULT_W, max);
    default:            return null;
  }
}

/** Width change for a pointer moved from `start` to `now`. The handle sits at the
 * top-left, so moving up or left grows the minimap; the axis that moved further
 * (vertical distance scaled by the aspect ratio) decides. */
export function dragWidthDelta(
  start: { x: number; y: number },
  now: { x: number; y: number },
): number {
  const dx = start.x - now.x;
  const dy = ((start.y - now.y) * ASPECT_W) / ASPECT_H;
  return Math.abs(dx) >= Math.abs(dy) ? dx : dy;
}

export function dragThresholdExceeded(
  start: { x: number; y: number },
  now: { x: number; y: number },
): boolean {
  return Math.hypot(now.x - start.x, now.y - start.y) >= DRAG_THRESHOLD_PX;
}

function readStoredWidth(): number {
  try { return parseStoredWidth(localStorage.getItem(MINIMAP_W_KEY)); }
  catch { return MINIMAP_DEFAULT_W; }
}

function writeStoredWidth(width: number): void {
  try { localStorage.setItem(MINIMAP_W_KEY, String(width)); } catch {}
}

interface DragState {
  pointerId: number;
  start: { x: number; y: number };
  startWidth: number;
  startRequested: number;
  moved: boolean;
}

let teardown: (() => void) | null = null;

export function bindMinimapResize(): void {
  teardown?.();
  teardown = null;

  // A reload mid-drag leaves the body cursor lock and the handle's dragging
  // class behind with no pointerup listener left to clear them.
  document.body.classList.remove(DRAG_BODY_CLASS);
  const handle = document.getElementById("minimap-resize-handle");
  handle?.classList.remove("dragging");

  const area = document.getElementById("canvas-area");
  if (!handle || !area) return;

  handle.tabIndex = 0;
  handle.setAttribute("role", "slider");
  handle.setAttribute("aria-label", "Resize minimap");

  const root = document.documentElement.style;
  const ac = new AbortController();
  const { signal } = ac;

  // What the user asked for; the applied width is this, re-clamped to the space available.
  let requested = readStoredWidth();
  let effective = MINIMAP_DEFAULT_W;
  let drag: DragState | null = null;

  const currentMax = (): number => maxWidthForArea(visibleViewport(area.getBoundingClientRect()));

  const apply = (): void => {
    const max = currentMax();
    effective = clampWidth(requested, max);
    const h = minimapHeightFor(effective);
    root.setProperty("--minimap-w", `${effective}px`);
    root.setProperty("--minimap-h", `${h}px`);
    handle.setAttribute("aria-valuemin", String(MINIMAP_MIN_W));
    handle.setAttribute("aria-valuemax", String(max));
    handle.setAttribute("aria-valuenow", String(effective));
    handle.setAttribute("aria-valuetext", `${effective} by ${h} pixels`);
  };

  const setRequested = (width: number, persist: boolean): void => {
    requested = clampWidth(width, currentMax());
    apply();
    if (persist) writeStoredWidth(requested);
  };

  const finish = (commit: boolean): void => {
    if (!drag) return;
    const d = drag;
    drag = null;
    handle.classList.remove("dragging");
    document.body.classList.remove(DRAG_BODY_CLASS);
    if (!d.moved) return;
    if (commit) {
      writeStoredWidth(requested);
    } else {
      requested = d.startRequested;
      apply();
    }
  };

  handle.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || !e.isPrimary || drag) return;
    e.stopPropagation();
    handle.setPointerCapture(e.pointerId);
    drag = {
      pointerId: e.pointerId,
      start: { x: e.clientX, y: e.clientY },
      startWidth: effective,
      startRequested: requested,
      moved: false,
    };
  }, { signal });

  handle.addEventListener("pointermove", (e) => {
    if (!drag || e.pointerId !== drag.pointerId) return;
    const now = { x: e.clientX, y: e.clientY };
    if (!drag.moved) {
      if (!dragThresholdExceeded(drag.start, now)) return;
      drag.moved = true;
      handle.classList.add("dragging");
      document.body.classList.add(DRAG_BODY_CLASS);
    }
    setRequested(drag.startWidth + dragWidthDelta(drag.start, now), false);
  }, { signal });

  handle.addEventListener("pointerup", (e) => {
    if (drag && e.pointerId === drag.pointerId) finish(true);
  }, { signal });
  // A cancelled gesture (or capture taken away, e.g. the minimap hidden mid-drag) restores the pre-drag width.
  const abortDrag = (e: PointerEvent): void => {
    if (drag && e.pointerId === drag.pointerId) finish(false);
  };
  handle.addEventListener("pointercancel", abortDrag, { signal });
  handle.addEventListener("lostpointercapture", abortDrag, { signal });

  handle.addEventListener("dblclick", () => setRequested(MINIMAP_DEFAULT_W, true), { signal });

  handle.addEventListener("keydown", (e) => {
    if (e.altKey || e.ctrlKey || e.metaKey) return;
    const next = keyStepWidth(e.key, effective, currentMax());
    if (next === null) return;
    e.preventDefault();
    setRequested(next, true);
  }, { signal });

  // The available space changes without the canvas area itself resizing when the
  // output drawer or chat panel changes, so those are watched too.
  window.addEventListener("resize", apply, { signal });
  const observers: Array<ResizeObserver | MutationObserver> = [];
  if (typeof ResizeObserver !== "undefined") {
    const ro = new ResizeObserver(apply);
    for (const el of [area, document.getElementById("output-drawer"), document.getElementById("chat-panel")]) {
      if (el) ro.observe(el);
    }
    observers.push(ro);
  }
  if (typeof MutationObserver !== "undefined") {
    const mo = new MutationObserver(apply);
    mo.observe(document.body, { attributes: true, attributeFilter: ["class"] });
    observers.push(mo);
  }

  apply();

  teardown = () => {
    ac.abort();
    for (const o of observers) o.disconnect();
    drag = null;
    handle.classList.remove("dragging");
    document.body.classList.remove(DRAG_BODY_CLASS);
  };
}
