import type { Canvas } from "./Canvas";
import { NODE_WIDTH, typeMeta, statusAccent, roundedRect, type CanvasNode, type CornerRadii } from "./Node";
import { getIconBitmap } from "../icon-cache";
import { NODE_IDS } from "../node-ids";
import { isTriggerNodeType } from "./node-registry";
import {
  getCanvasColors,
  resolveNoteColors,
  type CanvasThemeColors,
  type NoteColorSet,
} from "./theme-colors";

export interface Point { x: number; y: number }
export interface Size { w: number; h: number }
export interface Rect extends Point, Size {}

export interface MinimapBounds {
  minX: number;
  minY: number;
  scale: number;
  offX: number;
  offY: number;
  w: number;
  h: number;
}

export interface DragState {
  bounds: MinimapBounds;
  offset: Point;
  moved: boolean;
}

export interface MinimapColors {
  conn: string;
  nav: string;
  run: string;
  panel: string;
}

export type NodeStatus = CanvasNode["status"];

export interface MinimapNodeInput {
  isNote: boolean;
  noteColor: string | undefined;
  status: NodeStatus;
  selected: boolean;
  disabled: boolean;
  isTrigger: boolean;
  categoryAccent: string;
  dimmed: boolean;
}

export interface MinimapNodeStyle {
  rank: number;
  fill: string;
  fillAlpha: number;
  stroke: string | null;
  strokeWidth: number;
  selected: boolean;
  trigger: boolean;
  glyphAccent: string;
  minW: number;
  minH: number;
}

const BOUNDS_MARGIN = 30;
// The view rectangle may stretch the minimap's world box to at most this multiple of the node box on each axis, so nodes always keep a readable share of the panel when the canvas is zoomed far out.
const MAX_VIEWPORT_GROWTH = 2;
export const MIN_VIEW_ZOOM = 1;
export const MAX_VIEW_ZOOM = 8;
const WHEEL_ZOOM_STEP = 1.25;
const WHEEL_NOTCH_DELTA = 100;
const MAX_WHEEL_DELTA = 300;
const WHEEL_LINE_PX = 33;
export const EDGE_INSET = 3;
const EDGE_PAN_ZONE = 16;
const EDGE_PAN_SPEED = 360;
const EDGE_PAN_MAX_DT = 0.05;
const DISABLED_ALPHA = 0.35;
// Highest outside-the-viewport dim that keeps every node mark at 3:1 against the panel in Midnight and Paper (Paper is the limit, about 0.28).
const DIM_ALPHA = 0.25;
const OUTLINE_WIDTH = 1;
const ARROW_LENGTH = 7;
const ARROW_HALF_WIDTH = 4;
const MIN_VISIBLE_PX = 2;
const PULSE_PERIOD_MS = 1200;
const PULSE_MIN_ALPHA = 0.65;
const HOVER_SLOP = 2;
const LABEL_FONT = "10px system-ui, sans-serif";
const LABEL_PAD_X = 4;
const LABEL_H = 14;
const LABEL_GAP = 4;
const BADGE_H = 12;
const FOCUS_DIM_ALPHA = 0.5;
const DIM_WIRE_ALPHA = 0.4;
const WIRE_ARROW_LENGTH = 5;
const WIRE_ARROW_HALF_WIDTH = 3;
const WIRE_ARROW_MIN_CHORD = 28;
const CHIP_SIZE = 12;
const CHIP_MIN_W = 18;
const CHIP_MIN_H = 14;
const CHIP_INSET = 2;
const NAME_MIN_W = 52;
const NAME_GAP = 3;
const NAME_FONT = "600 9px system-ui, sans-serif";
const TRIGGER_MAX_RADIUS = 6;

const STATUS_RANK: Record<NodeStatus, number> = { idle: 1, success: 2, running: 3, error: 4 };
const NOTE_RANK = 0;
const MIN_SIZE: Record<NodeStatus, Size> = {
  idle:    { w: 4, h: 3 },
  success: { w: 5, h: 4 },
  running: { w: 6, h: 5 },
  error:   { w: 8, h: 6 },
};
const STATUS_RING_WIDTH: Record<NodeStatus, number> = { idle: 0, success: 1, running: 1, error: 1.5 };

export function resolveMinimapColors(colors: CanvasThemeColors): MinimapColors {
  return { conn: colors.wire, nav: colors.actionNav, run: colors.actionRun, panel: colors.surface1 };
}

export function normalizeDpr(dpr: number): number {
  return Number.isFinite(dpr) && dpr > 0 ? dpr : 1;
}

export function backingSize(css: Size, dpr: number): Size {
  const d = normalizeDpr(dpr);
  return { w: Math.max(1, Math.round(css.w * d)), h: Math.max(1, Math.round(css.h * d)) };
}

export function computeBounds(nodes: readonly Rect[], size: Size, viewport?: Rect): MinimapBounds | null {
  if (nodes.length === 0 || !(size.w > 2 * EDGE_INSET && size.h > 2 * EDGE_INSET)) return null;
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const n of nodes) {
    x0 = Math.min(x0, n.x - BOUNDS_MARGIN);
    y0 = Math.min(y0, n.y - BOUNDS_MARGIN);
    x1 = Math.max(x1, n.x + n.w + BOUNDS_MARGIN);
    y1 = Math.max(y1, n.y + n.h + BOUNDS_MARGIN);
  }
  if (viewport) {
    const padX = ((x1 - x0) * (MAX_VIEWPORT_GROWTH - 1)) / 2;
    const padY = ((y1 - y0) * (MAX_VIEWPORT_GROWTH - 1)) / 2;
    const vx0 = Math.max(viewport.x, x0 - padX), vx1 = Math.min(viewport.x + viewport.w, x1 + padX);
    const vy0 = Math.max(viewport.y, y0 - padY), vy1 = Math.min(viewport.y + viewport.h, y1 + padY);
    if (vx1 > vx0 && vy1 > vy0) {
      x0 = Math.min(x0, vx0); x1 = Math.max(x1, vx1);
      y0 = Math.min(y0, vy0); y1 = Math.max(y1, vy1);
    }
  }
  const ww = Math.max(x1 - x0, 1);
  const wh = Math.max(y1 - y0, 1);
  const scale = Math.min((size.w - 2 * EDGE_INSET) / ww, (size.h - 2 * EDGE_INSET) / wh);
  return {
    minX: x0,
    minY: y0,
    scale,
    offX: (size.w - ww * scale) / 2,
    offY: (size.h - wh * scale) / 2,
    w: size.w,
    h: size.h,
  };
}

export interface MinimapView { zoom: number; panX: number; panY: number }

export const DEFAULT_VIEW: MinimapView = { zoom: 1, panX: 0, panY: 0 };

function clampAxis(off: number, content: number, size: number): number {
  return content <= size ? (size - content) / 2 : Math.min(0, Math.max(size - content, off));
}

// The minimap's own magnification: scales the fitted bounds about the panel centre and shifts them by the pan, keeping the content on screen. Independent of the main canvas zoom.
export function viewBounds(base: MinimapBounds | null, view: MinimapView): MinimapBounds | null {
  if (!base) return null;
  const z = Math.min(MAX_VIEW_ZOOM, Math.max(MIN_VIEW_ZOOM, view.zoom));
  if (z === 1) return base;
  const cx = base.w / 2, cy = base.h / 2;
  const contentW = (base.w - 2 * base.offX) * z, contentH = (base.h - 2 * base.offY) * z;
  const offX = clampAxis((base.offX - cx) * z + cx + view.panX, contentW, base.w);
  const offY = clampAxis((base.offY - cy) * z + cy + view.panY, contentH, base.h);
  return { ...base, scale: base.scale * z, offX, offY };
}

// Next view after a wheel event at `pointer` (deltaY < 0 zooms in): the world point under the pointer stays under it. Exponential in deltaY, so a mouse notch (about 100) is one WHEEL_ZOOM_STEP and a trackpad pinch's many small deltas stay smooth.
export function zoomView(base: MinimapBounds | null, view: MinimapView, deltaY: number, pointer: Point): MinimapView {
  if (!base) return DEFAULT_VIEW;
  const d = Math.min(MAX_WHEEL_DELTA, Math.max(-MAX_WHEEL_DELTA, deltaY));
  const z = Math.min(MAX_VIEW_ZOOM, Math.max(MIN_VIEW_ZOOM, view.zoom * Math.exp((-d * Math.log(WHEEL_ZOOM_STEP)) / WHEEL_NOTCH_DELTA)));
  if (z === 1) return DEFAULT_VIEW;
  const cur = viewBounds(base, view)!;
  const wx = (pointer.x - cur.offX) / cur.scale, wy = (pointer.y - cur.offY) / cur.scale;
  const cx = base.w / 2, cy = base.h / 2;
  return {
    zoom: z,
    panX: pointer.x - wx * base.scale * z - (base.offX - cx) * z - cx,
    panY: pointer.y - wy * base.scale * z - (base.offY - cy) * z - cy,
  };
}

function viewFromOffsets(base: MinimapBounds, zoom: number, offX: number, offY: number): MinimapView {
  return { zoom, panX: offX - ((base.offX - base.w / 2) * zoom + base.w / 2), panY: offY - ((base.offY - base.h / 2) * zoom + base.h / 2) };
}

// Moves the magnified content by (dx, dy) panel pixels, like dragging it. The stored pan is re-clamped to the content edges, so dragging past an edge leaves no slack the next drag would have to burn through first.
export function panView(base: MinimapBounds | null, view: MinimapView, dx: number, dy: number): MinimapView {
  if (!base) return DEFAULT_VIEW;
  const z = Math.min(MAX_VIEW_ZOOM, Math.max(MIN_VIEW_ZOOM, view.zoom));
  if (z === 1) return DEFAULT_VIEW;
  const cur = viewBounds(base, view)!;
  const moved = viewBounds(base, viewFromOffsets(base, z, cur.offX + dx, cur.offY + dy))!;
  return viewFromOffsets(base, z, moved.offX, moved.offY);
}

// Panel pixels per second to shift a zoomed-in minimap while the view box is dragged against its edge; the closer to the edge, the faster. Positive moves the content right/down, revealing what lies left/up.
export function edgePanVelocity(p: Point, size: Size): Point {
  const axis = (v: number, len: number): number => {
    if (v < EDGE_PAN_ZONE) return EDGE_PAN_SPEED * Math.min(1, (EDGE_PAN_ZONE - v) / EDGE_PAN_ZONE);
    if (v > len - EDGE_PAN_ZONE) return -EDGE_PAN_SPEED * Math.min(1, (v - (len - EDGE_PAN_ZONE)) / EDGE_PAN_ZONE);
    return 0;
  };
  return { x: axis(p.x, size.w), y: axis(p.y, size.h) };
}

// A zoomed-in minimap shows only part of the workflow, so when the canvas view leaves that part (fit to screen, jump to a node, canvas zoom) the minimap scrolls to bring the view box back to its centre.
export function followViewport(base: MinimapBounds | null, view: MinimapView, viewport: Rect): MinimapView {
  if (!base || view.zoom <= 1) return view;
  const b = viewBounds(base, view)!;
  const v = viewportToMinimap(b, viewport);
  if (overlapsEnough(v.x, v.x + v.w, 0, b.w) && overlapsEnough(v.y, v.y + v.h, 0, b.h)) return view;
  return panView(base, view, b.w / 2 - (v.x + v.w / 2), b.h / 2 - (v.y + v.h / 2));
}

// Ctrl/Cmd + primary drag, or a middle-button drag, pans a magnified minimap; any other drag moves the canvas view as before.
export function isPanGesture(button: number, modifier: boolean, zoom: number): boolean {
  return zoom > 1 && (button === 1 || (button === 0 && modifier));
}

export function activeBounds(drag: DragState | null, live: MinimapBounds | null): MinimapBounds | null {
  return drag ? drag.bounds : live;
}

export function worldToMinimap(b: MinimapBounds, x: number, y: number): Point {
  return { x: (x - b.minX) * b.scale + b.offX, y: (y - b.minY) * b.scale + b.offY };
}

export function minimapToWorld(b: MinimapBounds, px: number, py: number): Point {
  const cx = Math.min(Math.max(px, b.offX), b.w - b.offX);
  const cy = Math.min(Math.max(py, b.offY), b.h - b.offY);
  return { x: (cx - b.offX) / b.scale + b.minX, y: (cy - b.offY) / b.scale + b.minY };
}

export function viewportToMinimap(b: MinimapBounds, viewport: Rect): Rect {
  const o = worldToMinimap(b, viewport.x, viewport.y);
  return { x: o.x, y: o.y, w: viewport.w * b.scale, h: viewport.h * b.scale };
}

function overlapsEnough(a0: number, a1: number, b0: number, b1: number): boolean {
  return Math.min(a1, b1) - Math.max(a0, b0) >= Math.min(MIN_VISIBLE_PX, a1 - a0, b1 - b0);
}

export function viewportOnContent(b: MinimapBounds, viewport: Rect): boolean {
  const v = viewportToMinimap(b, viewport);
  return overlapsEnough(v.x, v.x + v.w, Math.max(b.offX, 0), Math.min(b.w - b.offX, b.w)) && overlapsEnough(v.y, v.y + v.h, Math.max(b.offY, 0), Math.min(b.h - b.offY, b.h));
}

export function edgeArrow(b: MinimapBounds, viewport: Rect): [Point, Point, Point] | null {
  const v = viewportToMinimap(b, viewport);
  const cx = b.w / 2, cy = b.h / 2;
  const dx = v.x + v.w / 2 - cx, dy = v.y + v.h / 2 - cy;
  const t = Math.min((cx - EDGE_INSET) / Math.abs(dx), (cy - EDGE_INSET) / Math.abs(dy));
  if (!Number.isFinite(t)) return null;
  const len = Math.hypot(dx, dy);
  const ux = dx / len, uy = dy / len;
  const tip = { x: cx + dx * t, y: cy + dy * t };
  const bx = tip.x - ux * ARROW_LENGTH, by = tip.y - uy * ARROW_LENGTH;
  return [
    tip,
    { x: bx - uy * ARROW_HALF_WIDTH, y: by + ux * ARROW_HALF_WIDTH },
    { x: bx + uy * ARROW_HALF_WIDTH, y: by - ux * ARROW_HALF_WIDTH },
  ];
}

export function snapStroke(v: number, lineWidth: number, dpr: number): number {
  const device = Math.max(1, Math.round(lineWidth * dpr));
  const half = device % 2 === 1 ? 0.5 : 0;
  return (Math.round(v * dpr - half) + half) / dpr;
}

export type WireKind = "base" | "selected" | "active";

export function wireKind(active: boolean, connectorSelected: boolean, endSelected: boolean): WireKind {
  return active ? "active" : connectorSelected || endSelected ? "selected" : "base";
}

// Same control-point reach as Connector.draw, so a minimap wire bends like the canvas wire it stands for.
export function wireControlOffset(x1: number, x2: number): number {
  return Math.max(Math.abs(x2 - x1) * 0.55, 80);
}

export function wireBaseAlpha(anyActive: boolean, hasSelection: boolean): number {
  return anyActive || hasSelection ? DIM_WIRE_ALPHA : 1;
}

// Arrowhead at the wire's midpoint pointing along its direction of flow, or null when the wire is too short to carry one.
export function wireArrow(p0: Point, p1: Point, p2: Point, p3: Point): [Point, Point, Point] | null {
  if (Math.hypot(p3.x - p0.x, p3.y - p0.y) < WIRE_ARROW_MIN_CHORD) return null;
  const mid = { x: (p0.x + 3 * p1.x + 3 * p2.x + p3.x) / 8, y: (p0.y + 3 * p1.y + 3 * p2.y + p3.y) / 8 };
  const tx = p3.x + p2.x - p1.x - p0.x, ty = p3.y + p2.y - p1.y - p0.y;
  const len = Math.hypot(tx, ty);
  if (!(len > 0)) return null;
  const ux = tx / len, uy = ty / len;
  const tip = { x: mid.x + (ux * WIRE_ARROW_LENGTH) / 2, y: mid.y + (uy * WIRE_ARROW_LENGTH) / 2 };
  const bx = tip.x - ux * WIRE_ARROW_LENGTH, by = tip.y - uy * WIRE_ARROW_LENGTH;
  return [
    tip,
    { x: bx - uy * WIRE_ARROW_HALF_WIDTH, y: by + ux * WIRE_ARROW_HALF_WIDTH },
    { x: bx + uy * WIRE_ARROW_HALF_WIDTH, y: by - ux * WIRE_ARROW_HALF_WIDTH },
  ];
}

export interface NodeDecor {
  chip: Rect | null;
  name: { x: number; y: number; maxW: number } | null;
}

// What a node block has room for: an icon chip once it is big enough, plus the node's name beside it once it is wider still.
export function nodeDecor(rect: Rect): NodeDecor {
  if (rect.w < CHIP_MIN_W || rect.h < CHIP_MIN_H) return { chip: null, name: null };
  const chip: Rect = { x: rect.x + CHIP_INSET, y: rect.y + (rect.h - CHIP_SIZE) / 2, w: CHIP_SIZE, h: CHIP_SIZE };
  if (rect.w < NAME_MIN_W) return { chip, name: null };
  const x = chip.x + chip.w + NAME_GAP;
  return { chip, name: { x, y: rect.y + rect.h / 2, maxW: rect.x + rect.w - CHIP_INSET - x } };
}

// The part of a node mark inside the panel, so its chip and name stay on screen while the mark is magnified past the edge.
export function visiblePart(rect: Rect, size: Size): Rect | null {
  const x0 = Math.max(rect.x, 0), y0 = Math.max(rect.y, 0);
  const x1 = Math.min(rect.x + rect.w, size.w), y1 = Math.min(rect.y + rect.h, size.h);
  return x1 > x0 && y1 > y0 ? { x: x0, y: y0, w: x1 - x0, h: y1 - y0 } : null;
}

export function triggerRadii(rect: Rect): CornerRadii {
  const r = Math.min(TRIGGER_MAX_RADIUS, rect.w / 2, rect.h / 2);
  return { tl: r, tr: 0, br: 0, bl: r };
}

export function pulseAlpha(timeMs: number, reducedMotion: boolean): number {
  if (reducedMotion) return 1;
  const phase = (timeMs % PULSE_PERIOD_MS) / PULSE_PERIOD_MS;
  return PULSE_MIN_ALPHA + (1 - PULSE_MIN_ALPHA) * (0.5 + 0.5 * Math.sin(2 * Math.PI * phase));
}

export function insideRect(p: Point, r: Rect, slop = 0): boolean {
  return p.x >= r.x - slop && p.x <= r.x + r.w + slop && p.y >= r.y - slop && p.y <= r.y + r.h + slop;
}

export function hitTopmost(rects: readonly Rect[], p: Point): number {
  for (let i = rects.length - 1; i >= 0; i--) {
    if (insideRect(p, rects[i], HOVER_SLOP)) return i;
  }
  return -1;
}

export function hoverText(name: string, status: NodeStatus, disabled: boolean): string {
  const parts = [name || "Untitled"];
  if (status !== "idle") parts.push(status);
  if (disabled) parts.push("disabled");
  return parts.join(" \u00b7 ");
}

export function placeLabel(anchor: Rect, box: Size, size: Size): Point {
  const maxX = Math.max(size.w - EDGE_INSET - box.w, EDGE_INSET);
  const maxY = Math.max(size.h - EDGE_INSET - box.h, EDGE_INSET);
  const x = Math.min(Math.max(anchor.x + anchor.w / 2 - box.w / 2, EDGE_INSET), maxX);
  const above = anchor.y - LABEL_GAP - box.h;
  const y = above >= EDGE_INSET ? above : Math.min(anchor.y + anchor.h + LABEL_GAP, maxY);
  return { x, y };
}

export function nextErrorIndex(current: number, count: number, backwards: boolean): number {
  if (count <= 0) return -1;
  if (current < 0 || current >= count) return backwards ? count - 1 : 0;
  return (current + (backwards ? -1 : 1) + count) % count;
}

function traceBlock(mc: CanvasRenderingContext2D, rect: Rect, trigger: boolean): void {
  if (trigger) {
    roundedRect(mc, rect.x, rect.y, rect.w, rect.h, triggerRadii(rect));
  } else {
    mc.beginPath();
    mc.rect(rect.x, rect.y, rect.w, rect.h);
  }
}

function fitText(mc: CanvasRenderingContext2D, text: string, maxWidth: number): string {
  if (mc.measureText(text).width <= maxWidth) return text;
  for (let end = text.length - 1; end > 0; end--) {
    const t = text.slice(0, end) + "\u2026";
    if (mc.measureText(t).width <= maxWidth) return t;
  }
  return "\u2026";
}

export function contentPoint(
  clientX: number,
  clientY: number,
  box: { left: number; top: number },
  border: { left: number; top: number },
): Point {
  return { x: clientX - box.left - border.left, y: clientY - box.top - border.top };
}

export function grabOffset(pointer: Point, viewport: Rect): Point {
  const inside =
    pointer.x >= viewport.x && pointer.x <= viewport.x + viewport.w &&
    pointer.y >= viewport.y && pointer.y <= viewport.y + viewport.h;
  if (!inside) return { x: 0, y: 0 };
  return { x: pointer.x - (viewport.x + viewport.w / 2), y: pointer.y - (viewport.y + viewport.h / 2) };
}

export function dragCenter(pointer: Point, offset: Point): Point {
  return { x: pointer.x - offset.x, y: pointer.y - offset.y };
}

export function floorRect(r: Rect, minW: number, minH: number): Rect {
  const w = Math.max(r.w, minW);
  const h = Math.max(r.h, minH);
  return { x: r.x + r.w / 2 - w / 2, y: r.y + r.h / 2 - h / 2, w, h };
}

export function resolveNodeStyle(
  n: MinimapNodeInput,
  colors: CanvasThemeColors,
  noteColors: Record<string, NoteColorSet>,
): MinimapNodeStyle {
  if (n.isNote) {
    const c = noteColors[n.noteColor ?? "default"] ?? noteColors["default"];
    return {
      rank: NOTE_RANK, fill: c.bg, fillAlpha: 1, stroke: c.text, strokeWidth: 1,
      selected: n.selected, trigger: false, glyphAccent: c.text, minW: MIN_SIZE.idle.w, minH: MIN_SIZE.idle.h,
    };
  }
  const fill = statusAccent(n.status, colors) ?? (n.isTrigger ? colors.catTrigger : n.categoryAccent);
  const min = MIN_SIZE[n.status];
  const base = {
    rank: STATUS_RANK[n.status], fill, selected: n.selected,
    trigger: n.isTrigger, glyphAccent: n.categoryAccent, minW: min.w, minH: min.h,
  };
  if (n.disabled) {
    return { ...base, fillAlpha: DISABLED_ALPHA, stroke: fill, strokeWidth: 1 };
  }
  const ring = STATUS_RING_WIDTH[n.status];
  return {
    ...base, fillAlpha: n.dimmed ? FOCUS_DIM_ALPHA : 1,
    stroke: ring > 0 ? colors.textPrimary : null,
    strokeWidth: ring,
  };
}

export function minimapLabel(total: number, errors: number): string {
  if (total === 0) return "Workflow minimap: empty";
  const nodes = `${total} ${total === 1 ? "node" : "nodes"}`;
  return errors > 0
    ? `Workflow minimap: ${nodes}, ${errors} with ${errors === 1 ? "an error" : "errors"}`
    : `Workflow minimap: ${nodes}`;
}

interface Placed { rect: Rect; style: MinimapNodeStyle; node: CanvasNode }
interface Wire { p0: Point; p1: Point; p2: Point; p3: Point; kind: WireKind }

export class Minimap {
  private canvas: Canvas;
  readonly el: HTMLCanvasElement | null;
  private ctx: CanvasRenderingContext2D | null;
  private _size: Size = { w: 0, h: 0 };
  private _bounds: MinimapBounds | null = null;
  private _hits: { rect: Rect; node: CanvasNode }[] = [];
  private _base: MinimapBounds | null = null;
  private _view: MinimapView = DEFAULT_VIEW;
  private _viewport: Rect | null = null;
  private _drag: DragState | null = null;
  private _pointerId: number | null = null;
  private _pan: { id: number; last: Point } | null = null;
  private _dragPointer: Point | null = null;
  private _lastViewport: Rect | null = null;
  private _edgeTime: number | null = null;
  private _label = "";
  private _noteSrc: CanvasThemeColors | null = null;
  private _noteColors: Record<string, NoteColorSet> = {};
  private _hover: Point | null = null;
  private _badge: Rect | null = null;
  private _errorTargets: Point[] = [];
  private _errorIndex = -1;
  private _reducedMotion = false;

  constructor(canvas: Canvas, mmEl: HTMLCanvasElement | null) {
    this.canvas = canvas;
    if (!mmEl) {
      this.el = null;
      this.ctx = null;
      return;
    }
    this.el = mmEl;
    this.ctx = mmEl.getContext("2d");
    mmEl.style.pointerEvents = "all";
    this._size = { w: mmEl.clientWidth, h: mmEl.clientHeight };

    mmEl.addEventListener("mousedown", (e) => { e.preventDefault(); e.stopPropagation(); });
    mmEl.addEventListener("pointerdown", (e) => this._onDown(e));
    mmEl.addEventListener("pointermove", (e) => this._onMove(e));
    mmEl.addEventListener("pointerup", (e) => this._onEnd(e));
    mmEl.addEventListener("pointercancel", (e) => this._onEnd(e));
    mmEl.addEventListener("lostpointercapture", (e) => this._onEnd(e));
    mmEl.addEventListener("wheel", (e) => this._onWheel(e), { passive: false });
    mmEl.addEventListener("pointerleave", () => { this._hover = null; });
    mmEl.addEventListener("dblclick", (e) => this._onDoubleClick(e));
    mmEl.addEventListener("contextmenu", (e) => e.preventDefault());

    if (typeof window !== "undefined" && typeof window.matchMedia === "function") {
      const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
      this._reducedMotion = mq.matches;
      mq.addEventListener?.("change", (e) => { this._reducedMotion = e.matches; });
    }

    if (typeof ResizeObserver !== "undefined") {
      new ResizeObserver((entries) => {
        const r = entries[entries.length - 1]?.contentRect;
        if (r) this._size = { w: r.width, h: r.height };
      }).observe(mmEl);
    }
  }

  draw(vW: number, vH: number): void {
    const el = this.el, mc = this.ctx;
    if (!el || !mc) return;
    const { w, h } = this._size;
    if (!(w > 0 && h > 0)) return;

    // ResizeObserver does not fire when only devicePixelRatio changes (browser zoom, moving between monitors).
    const dpr = normalizeDpr(window.devicePixelRatio);
    const px = backingSize(this._size, dpr);
    if (el.width !== px.w) el.width = px.w;
    if (el.height !== px.h) el.height = px.h;
    mc.setTransform(dpr, 0, 0, dpr, 0, 0);
    mc.clearRect(0, 0, w, h);

    const c = this.canvas;
    if (c.nodes.size === 0) {
      this._bounds = null;
      this._base = null;
      this._hits = [];
      this._view = DEFAULT_VIEW;
      this._viewport = null;
      this._setLabel(minimapLabel(0, 0));
      return;
    }

    const colors = getCanvasColors();
    const mm = resolveMinimapColors(colors);
    if (colors !== this._noteSrc) {
      this._noteSrc = colors;
      this._noteColors = resolveNoteColors(colors);
    }

    const viewport: Rect = { x: -c.panX / c.zoom, y: -c.panY / c.zoom, w: vW / c.zoom, h: vH / c.zoom };
    const worldRects: Rect[] = [];
    for (const n of c.nodes.values()) {
      worldRects.push({ x: n.data.position.x, y: n.data.position.y, w: NODE_WIDTH, h: n.height });
    }
    // While magnified the fitted base is held still, so the view the user zoomed into doesn't shift as the canvas pans underneath it.
    const stale = !this._base || this._base.w !== this._size.w || this._base.h !== this._size.h;
    if (!this._drag && (this._view.zoom === 1 || stale)) {
      if (stale) this._view = DEFAULT_VIEW;
      this._base = computeBounds(worldRects, this._size, viewport);
    }
    this._edgePan();
    const last = this._lastViewport;
    const moved = !last || last.x !== viewport.x || last.y !== viewport.y || last.w !== viewport.w || last.h !== viewport.h;
    this._lastViewport = viewport;
    if (moved && !this._drag && !this._pan) this._view = followViewport(this._base, this._view, viewport);
    const bounds = activeBounds(this._drag, viewBounds(this._base, this._view));
    this._bounds = bounds;
    this._viewport = viewport;
    if (!bounds) return;

    let hasSelection = false;
    for (const n of c.nodes.values()) {
      if (n.selected) { hasSelection = true; break; }
    }

    const wires: Wire[] = [];
    for (const conn of c.connectors.values()) {
      const fn = c.nodes.get(conn.data.from_node), tn = c.nodes.get(conn.data.to_node);
      if (!fn || !tn) continue;
      const fp = fn.ports.find(p => p.id === conn.data.from_port);
      const tp = tn.ports.find(p => p.id === conn.data.to_port);
      if (!fp || !tp) continue;
      const cp = wireControlOffset(fp.x, tp.x);
      wires.push({
        p0: worldToMinimap(bounds, fp.x, fp.y),
        p1: worldToMinimap(bounds, fp.x + cp, fp.y),
        p2: worldToMinimap(bounds, tp.x - cp, tp.y),
        p3: worldToMinimap(bounds, tp.x, tp.y),
        kind: wireKind(conn.active, conn.selected, fn.selected || tn.selected),
      });
    }
    const wireStyles: [WireKind, string, number][] = [["base", mm.conn, 1], ["selected", mm.nav, 1.5], ["active", mm.run, 1.5]];
    const baseWireAlpha = wireBaseAlpha(wires.some(wire => wire.kind === "active"), hasSelection);
    for (const [kind, color, width] of wireStyles) {
      mc.globalAlpha = kind === "base" ? baseWireAlpha : 1;
      mc.strokeStyle = color;
      mc.fillStyle = color;
      mc.lineWidth = width;
      mc.beginPath();
      for (const wire of wires) {
        if (wire.kind !== kind) continue;
        mc.moveTo(wire.p0.x, wire.p0.y);
        mc.bezierCurveTo(wire.p1.x, wire.p1.y, wire.p2.x, wire.p2.y, wire.p3.x, wire.p3.y);
      }
      mc.stroke();
      mc.beginPath();
      for (const wire of wires) {
        if (wire.kind !== kind) continue;
        const arrow = wireArrow(wire.p0, wire.p1, wire.p2, wire.p3);
        if (!arrow) continue;
        mc.moveTo(arrow[0].x, arrow[0].y);
        mc.lineTo(arrow[1].x, arrow[1].y);
        mc.lineTo(arrow[2].x, arrow[2].y);
        mc.closePath();
      }
      mc.fill();
    }
    mc.globalAlpha = 1;

    const placed: Placed[] = [];
    const errorTargets: Point[] = [];
    let errors = 0;
    let i = 0;
    for (const n of c.nodes.values()) {
      const world = worldRects[i++];
      const noteColor = n.data.config["color"];
      const style = resolveNodeStyle(
        {
          isNote: n.data.node_type_id === NODE_IDS.NOTE,
          noteColor: typeof noteColor === "string" ? noteColor : undefined,
          status: n.status,
          selected: n.selected,
          disabled: n.disabled,
          isTrigger: isTriggerNodeType(n.data.node_type_id),
          categoryAccent: typeMeta(n.data.node_type).accent,
          dimmed: hasSelection && !n.selected && n.status !== "error",
        },
        colors,
        this._noteColors,
      );
      if (n.status === "error") {
        errors++;
        errorTargets.push({ x: world.x + world.w / 2, y: world.y + world.h / 2 });
      }
      const o = worldToMinimap(bounds, world.x, world.y);
      placed.push({
        rect: floorRect({ x: o.x, y: o.y, w: world.w * bounds.scale, h: world.h * bounds.scale }, style.minW, style.minH),
        style,
        node: n,
      });
    }
    placed.sort((a, b) => a.style.rank - b.style.rank);
    errorTargets.sort((a, b) => a.x - b.x || a.y - b.y);
    this._hits = placed.map(({ rect, node }) => ({ rect, node }));
    this._errorTargets = errorTargets;
    if (errors === 0) this._errorIndex = -1;
    const pulse = pulseAlpha(performance.now(), this._reducedMotion);

    mc.textAlign = "left";
    mc.textBaseline = "middle";
    for (const { rect, style, node } of placed) {
      mc.globalAlpha = style.fillAlpha;
      mc.fillStyle = style.fill;
      traceBlock(mc, rect, style.trigger);
      mc.fill();
      mc.globalAlpha = 1;
      if (style.stroke) {
        mc.globalAlpha = node.status === "running" ? pulse : 1;
        mc.strokeStyle = style.stroke;
        mc.lineWidth = style.strokeWidth;
        traceBlock(mc, rect, style.trigger);
        mc.stroke();
        mc.globalAlpha = 1;
      }
      const visible = node.data.node_type_id === NODE_IDS.NOTE ? null : visiblePart(rect, this._size);
      if (visible) this._drawDecor(mc, visible, style, node, mm.panel, style.fillAlpha < 1 ? colors.textPrimary : mm.panel);
    }

    mc.strokeStyle = mm.nav;
    mc.lineWidth = 1;
    for (const { rect, style } of placed) {
      if (style.selected) mc.strokeRect(rect.x - 2, rect.y - 2, rect.w + 4, rect.h + 4);
    }

    if (viewportOnContent(bounds, viewport)) {
      const v = viewportToMinimap(bounds, viewport);
      // Clamped to the panel so a view larger than the map still shows its outline along the edge instead of vanishing.
      const edge = OUTLINE_WIDTH / 2;
      const x0 = Math.max(snapStroke(v.x, OUTLINE_WIDTH, dpr), edge), y0 = Math.max(snapStroke(v.y, OUTLINE_WIDTH, dpr), edge);
      const x1 = Math.min(snapStroke(v.x + v.w, OUTLINE_WIDTH, dpr), w - edge), y1 = Math.min(snapStroke(v.y + v.h, OUTLINE_WIDTH, dpr), h - edge);
      mc.globalAlpha = DIM_ALPHA;
      mc.fillStyle = mm.panel;
      mc.beginPath();
      mc.rect(0, 0, w, h);
      mc.rect(x0, y0, x1 - x0, y1 - y0);
      mc.fill("evenodd");
      mc.globalAlpha = 1;
      mc.strokeStyle = mm.nav;
      mc.lineWidth = OUTLINE_WIDTH;
      mc.strokeRect(x0, y0, x1 - x0, y1 - y0);
    } else {
      const arrow = edgeArrow(bounds, viewport);
      if (arrow) {
        mc.beginPath();
        mc.moveTo(arrow[0].x, arrow[0].y);
        mc.lineTo(arrow[1].x, arrow[1].y);
        mc.lineTo(arrow[2].x, arrow[2].y);
        mc.closePath();
        mc.lineWidth = 2;
        mc.strokeStyle = mm.panel;
        mc.stroke();
        mc.fillStyle = mm.nav;
        mc.fill();
      }
    }

    this._badge = null;
    mc.font = LABEL_FONT;
    mc.textAlign = "left";
    mc.textBaseline = "middle";
    if (errors > 0) {
      const shown = this._errorIndex >= 0 && this._errorIndex < errors ? `${this._errorIndex + 1}/${errors}` : `${errors}`;
      const text = `! ${shown}`;
      const badge: Rect = { x: EDGE_INSET, y: h - EDGE_INSET - BADGE_H, w: Math.ceil(mc.measureText(text).width) + 2 * LABEL_PAD_X, h: BADGE_H };
      mc.fillStyle = colors.surface2;
      mc.fillRect(badge.x, badge.y, badge.w, badge.h);
      mc.strokeStyle = colors.error;
      mc.lineWidth = 1;
      mc.strokeRect(badge.x + 0.5, badge.y + 0.5, badge.w - 1, badge.h - 1);
      mc.fillStyle = colors.textPrimary;
      mc.fillText(text, badge.x + LABEL_PAD_X, badge.y + badge.h / 2);
      this._badge = badge;
    }

    const hover = this._hover;
    if (hover && !this._drag && !(this._badge && insideRect(hover, this._badge))) {
      const hit = hitTopmost(placed.map(pl => pl.rect), hover);
      if (hit >= 0) {
        const { rect, node } = placed[hit];
        const text = fitText(mc, hoverText(node.data.name, node.status, node.disabled), w - 2 * EDGE_INSET - 2 * LABEL_PAD_X);
        const box: Size = { w: Math.ceil(mc.measureText(text).width) + 2 * LABEL_PAD_X, h: LABEL_H };
        const pos = placeLabel(rect, box, this._size);
        mc.fillStyle = colors.surface2;
        mc.fillRect(pos.x, pos.y, box.w, box.h);
        mc.strokeStyle = colors.border;
        mc.lineWidth = 1;
        mc.strokeRect(pos.x + 0.5, pos.y + 0.5, box.w - 1, box.h - 1);
        mc.fillStyle = colors.textPrimary;
        mc.fillText(text, pos.x + LABEL_PAD_X, pos.y + box.h / 2);
      }
    }

    this._setLabel(minimapLabel(c.nodes.size, errors));
  }

  private _drawDecor(mc: CanvasRenderingContext2D, rect: Rect, style: MinimapNodeStyle, node: CanvasNode, panel: string, ink: string): void {
    const decor = nodeDecor(rect);
    const chip = decor.chip;
    if (!chip) return;
    mc.globalAlpha = style.fillAlpha;
    mc.fillStyle = panel;
    roundedRect(mc, chip.x, chip.y, chip.w, chip.h, 3);
    mc.fill();
    const bmp = getIconBitmap(node.data.node_type_id, style.glyphAccent);
    if (bmp) {
      mc.drawImage(bmp, chip.x + 1, chip.y + 1, chip.w - 2, chip.h - 2);
    } else {
      mc.font = NAME_FONT;
      mc.fillStyle = style.glyphAccent;
      mc.textAlign = "center";
      mc.fillText(typeMeta(node.data.node_type).icon, chip.x + chip.w / 2, chip.y + chip.h / 2 + 0.5);
      mc.textAlign = "left";
    }
    if (decor.name) {
      mc.globalAlpha = 1;
      mc.font = NAME_FONT;
      mc.fillStyle = ink;
      mc.fillText(fitText(mc, node.data.name || "Untitled", decor.name.maxW), decor.name.x, decor.name.y + 0.5);
    }
    mc.globalAlpha = 1;
  }

  private _setLabel(text: string): void {
    if (text === this._label || !this.el) return;
    this._label = text;
    this.el.setAttribute("aria-label", text);
  }

  private _pointerContent(e: MouseEvent): Point | null {
    const el = this.el;
    if (!el) return null;
    return contentPoint(e.clientX, e.clientY, el.getBoundingClientRect(), { left: el.clientLeft, top: el.clientTop });
  }

  private _pointerWorld(e: PointerEvent, bounds: MinimapBounds): Point | null {
    const p = this._pointerContent(e);
    return p ? minimapToWorld(bounds, p.x, p.y) : null;
  }

  private _overBadge(e: MouseEvent): boolean {
    const badge = this._badge;
    if (!badge) return false;
    const p = this._pointerContent(e);
    return !!p && insideRect(p, badge);
  }

  private _cycleError(backwards: boolean): void {
    const next = nextErrorIndex(this._errorIndex, this._errorTargets.length, backwards);
    const target = this._errorTargets[next];
    if (!target) return;
    this._errorIndex = next;
    this.canvas.centerOn(target.x, target.y);
    this.canvas.onViewportChange?.();
  }

  // Runs every frame: while the view box is held against the edge of a zoomed-in minimap, scrolls the minimap and keeps the box under the pointer.
  private _edgePan(): void {
    const drag = this._drag, p = this._dragPointer, base = this._base;
    if (!drag || !p || !base || this._view.zoom <= 1) { this._edgeTime = null; return; }
    const now = performance.now();
    const dt = this._edgeTime === null ? 0 : Math.min((now - this._edgeTime) / 1000, EDGE_PAN_MAX_DT);
    this._edgeTime = now;
    const v = edgePanVelocity(p, this._size);
    if (dt === 0 || (v.x === 0 && v.y === 0)) return;
    this._view = panView(base, this._view, v.x * dt, v.y * dt);
    const next = viewBounds(base, this._view);
    if (!next) return;
    drag.bounds = next;
    this._panTo(minimapToWorld(next, p.x, p.y), drag);
  }

  private _panTo(world: Point, drag: DragState): void {
    const target = dragCenter(world, drag.offset);
    this.canvas.centerOn(target.x, target.y);
    drag.moved = true;
  }

  private _onDown(e: PointerEvent): void {
    const el = this.el, bounds = this._bounds, viewport = this._viewport;
    if (!el || !bounds || !viewport || this._drag || this._pan) return;
    if (isPanGesture(e.button, e.ctrlKey || e.metaKey, this._view.zoom)) {
      this._beginPan(e);
      return;
    }
    if (e.button !== 0) return;
    if (this._overBadge(e)) {
      this._cycleError(e.shiftKey);
      return;
    }
    this._hover = null;
    const world = this._pointerWorld(e, bounds);
    if (!world) return;
    const drag: DragState = { bounds, offset: grabOffset(world, viewport), moved: false };
    this._drag = drag;
    this._dragPointer = this._pointerContent(e);
    this._edgeTime = null;
    this._pointerId = e.pointerId;
    try { el.setPointerCapture?.(e.pointerId); } catch { /* pointer already gone; the drag still ends on pointerup/cancel */ }
    this._panTo(world, drag);
  }

  private _beginPan(e: PointerEvent): void {
    const el = this.el, p = this._pointerContent(e);
    if (!el || !p) return;
    this._hover = null;
    this._pan = { id: e.pointerId, last: p };
    el.style.cursor = "grabbing";
    try { el.setPointerCapture?.(e.pointerId); } catch { /* pointer already gone; the pan still ends on pointerup/cancel */ }
  }

  // A release lost to a context menu, another window or a dropped capture would otherwise leave the minimap stuck mid-gesture and deaf to new ones.
  private _dropStaleGesture(e: PointerEvent): void {
    if (e.buttons !== 0 || !(this._pan || this._drag)) return;
    this._pan = null;
    this._drag = null;
    this._dragPointer = null;
    this._pointerId = null;
    if (this.el) this.el.style.cursor = "";
  }

  private _onMove(e: PointerEvent): void {
    this._dropStaleGesture(e);
    const pan = this._pan;
    if (pan) {
      if (e.pointerId !== pan.id) return;
      const p = this._pointerContent(e);
      if (!p) return;
      this._view = panView(this._base, this._view, p.x - pan.last.x, p.y - pan.last.y);
      pan.last = p;
      return;
    }
    const drag = this._drag;
    if (!drag) {
      this._hover = this._pointerContent(e);
      if (this.el) this.el.style.cursor = this._view.zoom > 1 && (e.ctrlKey || e.metaKey) ? "grab" : "";
      return;
    }
    if (e.pointerId !== this._pointerId) return;
    this._dragPointer = this._pointerContent(e);
    const world = this._pointerWorld(e, drag.bounds);
    if (world) this._panTo(world, drag);
  }

  // Double-click a node: select it on the canvas and bring it to the centre. Anywhere else: reset the minimap zoom and fit the workflow.
  private _onDoubleClick(e: MouseEvent): void {
    if (this._overBadge(e)) return;
    const p = this._pointerContent(e);
    const hit = p ? hitTopmost(this._hits.map(h => h.rect), p) : -1;
    if (hit >= 0) {
      this.canvas.revealNode(this._hits[hit].node);
      return;
    }
    this._view = DEFAULT_VIEW;
    this.canvas.fitToScreen();
  }

  private _onWheel(e: WheelEvent): void {
    e.preventDefault();
    if (this._drag || this._pan || e.deltaY === 0) return;
    if (!(e.ctrlKey || e.metaKey)) {
      if (e.deltaY < 0) this.canvas.zoomIn();
      else this.canvas.zoomOut();
      return;
    }
    // Ctrl/Cmd (also what a trackpad pinch reports) magnifies the minimap itself.
    const base = this._base, p = this._pointerContent(e);
    if (!base || !p) return;
    this._view = zoomView(base, this._view, e.deltaY * (e.deltaMode === 1 ? WHEEL_LINE_PX : 1), p);
  }

  private _onEnd(e: PointerEvent): void {
    const pan = this._pan;
    if (pan && e.pointerId === pan.id) {
      this._pan = null;
      if (this.el) this.el.style.cursor = "";
      try { this.el?.releasePointerCapture?.(e.pointerId); } catch { /* capture already released */ }
      return;
    }
    const drag = this._drag;
    if (!drag || e.pointerId !== this._pointerId) return;
    this._drag = null;
    this._dragPointer = null;
    this._pointerId = null;
    try { this.el?.releasePointerCapture?.(e.pointerId); } catch { /* capture already released */ }
    if (drag.moved) this.canvas.onViewportChange?.();
  }
}
