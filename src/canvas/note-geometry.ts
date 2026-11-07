export type NoteStyle = "note" | "frame";
export type Handle = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

export interface Rect { x: number; y: number; w: number; h: number }

export const NOTE_DEFAULT_W = 220;
export const NOTE_MIN_W = 100;
export const NOTE_MIN_H = 60;
export const FRAME_DEFAULT_W = 420;
export const FRAME_DEFAULT_H = 260;
export const FRAME_MIN_W = 120;
export const FRAME_MIN_H = 80;
export const FRAME_HEADER = 32;
export const FRAME_PAD = 32;
export const NOTE_MAX_SIZE = 6000;
export const NOTE_SIZES = ["s", "m", "l", "xl"] as const;
export type NoteSize = (typeof NOTE_SIZES)[number];
export const NOTE_SIZE_LABEL: Record<NoteSize, string> = { s: "S", m: "M", l: "L", xl: "XL" };
export const NOTE_SIZE_NAME: Record<NoteSize, string> = { s: "Small", m: "Medium", l: "Large", xl: "Extra large" };

export interface NoteMetrics {
  body: number;
  heading: number;
  line: number;
  box: number;
  indent: number;
  title: number;
}

const BODY_PX: Record<NoteSize, number> = { s: 10, m: 12, l: 18, xl: 28 };
// A frame's title sits in a fixed FRAME_HEADER band, so its largest size must fit inside it.
const TITLE_PX: Record<NoteSize, number> = { s: 11, m: 13, l: 16, xl: 20 };

/** The size stored in a note's config, with anything unknown read as medium. */
export function noteSizeOf(config: Record<string, unknown>): NoteSize {
  const v = config["size"];
  return NOTE_SIZES.find(s => s === v) ?? "m";
}

/** Text metrics in canvas units for a size; medium matches the original fixed sizes. */
export function noteMetrics(size: NoteSize): NoteMetrics {
  const body = BODY_PX[size];
  return { body, heading: body + 2, line: Math.round(body * 1.5), box: body, indent: body + 8, title: TITLE_PX[size] };
}

const FRAME_EDGE_HIT = 8;
const HANDLE_HIT_PX = 7;

export function noteStyleOf(config: Record<string, unknown>): NoteStyle {
  return config["style"] === "frame" ? "frame" : "note";
}

export function minSize(style: NoteStyle): { w: number; h: number } {
  return style === "frame" ? { w: FRAME_MIN_W, h: FRAME_MIN_H } : { w: NOTE_MIN_W, h: NOTE_MIN_H };
}

export function clampDim(v: unknown, min: number, max: number, fallback: number): number {
  if (typeof v !== "number" || !Number.isFinite(v)) return fallback;
  return Math.min(max, Math.max(min, v));
}

export function resizeRect(r: Rect, handle: Handle, dx: number, dy: number, min: { w: number; h: number }): Rect {
  let { x, y, w, h } = r;
  if (handle.includes("e")) w = Math.max(min.w, Math.min(NOTE_MAX_SIZE, r.w + dx));
  if (handle.includes("s")) h = Math.max(min.h, Math.min(NOTE_MAX_SIZE, r.h + dy));
  if (handle.includes("w")) {
    w = Math.max(min.w, Math.min(NOTE_MAX_SIZE, r.w - dx));
    x = r.x + r.w - w;
  }
  if (handle.includes("n")) {
    h = Math.max(min.h, Math.min(NOTE_MAX_SIZE, r.h - dy));
    y = r.y + r.h - h;
  }
  return { x, y, w, h };
}

export function handleAt(r: Rect, wx: number, wy: number, zoom: number): Handle | null {
  const t = HANDLE_HIT_PX / zoom;
  const nearL = Math.abs(wx - r.x) <= t, nearR = Math.abs(wx - (r.x + r.w)) <= t;
  const nearT = Math.abs(wy - r.y) <= t, nearB = Math.abs(wy - (r.y + r.h)) <= t;
  if (nearT && nearL) return "nw";
  if (nearT && nearR) return "ne";
  if (nearB && nearL) return "sw";
  if (nearB && nearR) return "se";
  const withinX = wx >= r.x && wx <= r.x + r.w, withinY = wy >= r.y && wy <= r.y + r.h;
  if (nearT && withinX) return "n";
  if (nearB && withinX) return "s";
  if (nearL && withinY) return "w";
  if (nearR && withinY) return "e";
  return null;
}

export const HANDLE_CURSOR: Record<Handle, string> = {
  n: "ns-resize", s: "ns-resize", e: "ew-resize", w: "ew-resize",
  ne: "nesw-resize", sw: "nesw-resize", nw: "nwse-resize", se: "nwse-resize",
};

/** Frames can only be grabbed by their title bar or border, so the inside stays usable for panning and box-select. */
export function frameHit(r: Rect, wx: number, wy: number): boolean {
  if (wx < r.x - FRAME_EDGE_HIT || wx > r.x + r.w + FRAME_EDGE_HIT || wy < r.y - FRAME_EDGE_HIT || wy > r.y + r.h + FRAME_EDGE_HIT) return false;
  if (wy <= r.y + FRAME_HEADER) return true;
  return wx <= r.x + FRAME_EDGE_HIT || wx >= r.x + r.w - FRAME_EDGE_HIT || wy >= r.y + r.h - FRAME_EDGE_HIT;
}

export function rectContains(outer: Rect, inner: Rect): boolean {
  return inner.x >= outer.x && inner.y >= outer.y
    && inner.x + inner.w <= outer.x + outer.w && inner.y + inner.h <= outer.y + outer.h;
}

/** The frame rectangle that wraps `inner` with the standard padding and a title band above, or null when there is nothing to wrap. */
export function fitFrameRect(inner: Rect[]): Rect | null {
  if (!inner.length) return null;
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const r of inner) {
    x0 = Math.min(x0, r.x); y0 = Math.min(y0, r.y);
    x1 = Math.max(x1, r.x + r.w); y1 = Math.max(y1, r.y + r.h);
  }
  const w = x1 - x0 + FRAME_PAD * 2;
  const h = y1 - y0 + FRAME_PAD * 2 + FRAME_HEADER;
  return {
    x: x0 - FRAME_PAD,
    y: y0 - FRAME_PAD - FRAME_HEADER,
    w: clampDim(w, FRAME_MIN_W, NOTE_MAX_SIZE, w),
    h: clampDim(h, FRAME_MIN_H, NOTE_MAX_SIZE, h),
  };
}

/** The smallest frame whose rectangle contains every dragged rectangle, or null when none does. The dragged set's own frames must not be passed in. */
export function innermostContainingFrame(frames: { id: string; rect: Rect }[], dragged: Rect[]): string | null {
  if (!dragged.length) return null;
  let best: string | null = null, bestArea = Infinity;
  for (const f of frames) {
    if (!dragged.every(d => rectContains(f.rect, d))) continue;
    const area = f.rect.w * f.rect.h;
    if (area < bestArea) { best = f.id; bestArea = area; }
  }
  return best;
}
