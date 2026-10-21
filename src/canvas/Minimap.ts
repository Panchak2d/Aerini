import type { Canvas } from "./Canvas";
import { NODE_WIDTH, typeMeta, statusAccent, type CanvasNode } from "./Node";
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
}

export interface MinimapNodeStyle {
  rank: number;
  fill: string;
  fillAlpha: number;
  stroke: string | null;
  strokeWidth: number;
  selected: boolean;
  minW: number;
  minH: number;
}

const BOUNDS_MARGIN = 30;
export const EDGE_INSET = 3;
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

export function computeBounds(nodes: readonly Rect[], size: Size): MinimapBounds | null {
  if (nodes.length === 0 || !(size.w > 2 * EDGE_INSET && size.h > 2 * EDGE_INSET)) return null;
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const n of nodes) {
    x0 = Math.min(x0, n.x - BOUNDS_MARGIN);
    y0 = Math.min(y0, n.y - BOUNDS_MARGIN);
    x1 = Math.max(x1, n.x + n.w + BOUNDS_MARGIN);
    y1 = Math.max(y1, n.y + n.h + BOUNDS_MARGIN);
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
  return overlapsEnough(v.x, v.x + v.w, b.offX, b.w - b.offX) && overlapsEnough(v.y, v.y + v.h, b.offY, b.h - b.offY);
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
      selected: n.selected, minW: MIN_SIZE.idle.w, minH: MIN_SIZE.idle.h,
    };
  }
  const fill = statusAccent(n.status, colors) ?? (n.isTrigger ? colors.catTrigger : n.categoryAccent);
  const min = MIN_SIZE[n.status];
  const base = { rank: STATUS_RANK[n.status], fill, selected: n.selected, minW: min.w, minH: min.h };
  if (n.disabled) {
    return { ...base, fillAlpha: DISABLED_ALPHA, stroke: fill, strokeWidth: 1 };
  }
  const ring = STATUS_RING_WIDTH[n.status];
  return {
    ...base, fillAlpha: 1,
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
  private _viewport: Rect | null = null;
  private _drag: DragState | null = null;
  private _pointerId: number | null = null;
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
    mmEl.addEventListener("dblclick", (e) => { if (!this._overBadge(e)) this.canvas.fitToScreen(); });

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
    const bounds = activeBounds(this._drag, computeBounds(worldRects, this._size));
    this._bounds = bounds;
    this._viewport = viewport;
    if (!bounds) return;

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
    for (const [kind, color, width] of wireStyles) {
      mc.strokeStyle = color;
      mc.lineWidth = width;
      mc.beginPath();
      for (const wire of wires) {
        if (wire.kind !== kind) continue;
        mc.moveTo(wire.p0.x, wire.p0.y);
        mc.bezierCurveTo(wire.p1.x, wire.p1.y, wire.p2.x, wire.p2.y, wire.p3.x, wire.p3.y);
      }
      mc.stroke();
    }

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
    this._errorTargets = errorTargets;
    if (errors === 0) this._errorIndex = -1;
    const pulse = pulseAlpha(performance.now(), this._reducedMotion);

    for (const { rect, style, node } of placed) {
      mc.globalAlpha = style.fillAlpha;
      mc.fillStyle = style.fill;
      mc.fillRect(rect.x, rect.y, rect.w, rect.h);
      mc.globalAlpha = 1;
      if (style.stroke) {
        mc.globalAlpha = node.status === "running" ? pulse : 1;
        mc.strokeStyle = style.stroke;
        mc.lineWidth = style.strokeWidth;
        mc.strokeRect(rect.x, rect.y, rect.w, rect.h);
        mc.globalAlpha = 1;
      }
    }

    mc.strokeStyle = mm.nav;
    mc.lineWidth = 1;
    for (const { rect, style } of placed) {
      if (style.selected) mc.strokeRect(rect.x - 2, rect.y - 2, rect.w + 4, rect.h + 4);
    }

    if (viewportOnContent(bounds, viewport)) {
      const v = viewportToMinimap(bounds, viewport);
      const x0 = snapStroke(v.x, OUTLINE_WIDTH, dpr), y0 = snapStroke(v.y, OUTLINE_WIDTH, dpr);
      const x1 = snapStroke(v.x + v.w, OUTLINE_WIDTH, dpr), y1 = snapStroke(v.y + v.h, OUTLINE_WIDTH, dpr);
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

  private _panTo(world: Point, drag: DragState): void {
    const target = dragCenter(world, drag.offset);
    this.canvas.centerOn(target.x, target.y);
    drag.moved = true;
  }

  private _onDown(e: PointerEvent): void {
    const el = this.el, bounds = this._bounds, viewport = this._viewport;
    if (!el || !bounds || !viewport || e.button !== 0 || this._drag) return;
    if (this._overBadge(e)) {
      this._cycleError(e.shiftKey);
      return;
    }
    this._hover = null;
    const world = this._pointerWorld(e, bounds);
    if (!world) return;
    const drag: DragState = { bounds, offset: grabOffset(world, viewport), moved: false };
    this._drag = drag;
    this._pointerId = e.pointerId;
    try { el.setPointerCapture?.(e.pointerId); } catch { /* pointer already gone; the drag still ends on pointerup/cancel */ }
    this._panTo(world, drag);
  }

  private _onMove(e: PointerEvent): void {
    const drag = this._drag;
    if (!drag) {
      this._hover = this._pointerContent(e);
      return;
    }
    if (e.pointerId !== this._pointerId) return;
    const world = this._pointerWorld(e, drag.bounds);
    if (world) this._panTo(world, drag);
  }

  private _onWheel(e: WheelEvent): void {
    e.preventDefault();
    if (this._drag || e.deltaY === 0) return;
    if (e.deltaY < 0) this.canvas.zoomIn();
    else this.canvas.zoomOut();
  }

  private _onEnd(e: PointerEvent): void {
    const drag = this._drag;
    if (!drag || e.pointerId !== this._pointerId) return;
    this._drag = null;
    this._pointerId = null;
    try { this.el?.releasePointerCapture?.(e.pointerId); } catch { /* capture already released */ }
    if (drag.moved) this.canvas.onViewportChange?.();
  }
}
