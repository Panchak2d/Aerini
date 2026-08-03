import type { Canvas } from "./Canvas";
import { CanvasNode, NODE_WIDTH, wrapText } from "./Node";
import { NODE_IDS } from "../node-ids";

export class SnapEngine {
  private canvas: Canvas;
  snapGuides: Array<{ x1: number; y1: number; x2: number; y2: number }> = [];

  constructor(canvas: Canvas) {
    this.canvas = canvas;
  }

  computeSnap(moving: CanvasNode): void {
    this.snapGuides = [];
    const SNAP = 8;
    const mx = moving.data.position.x, my = moving.data.position.y;
    const mw = NODE_WIDTH, mh = moving.height;
    for (const n of this.canvas.nodes.values()) {
      if (n.data.id === moving.data.id) continue;
      const nx = n.data.position.x, ny = n.data.position.y, nh = n.height;
      for (const [a, b] of [[mx, nx], [mx, nx + NODE_WIDTH - mw], [mx + mw / 2, nx + NODE_WIDTH / 2]] as [number, number][]) {
        if (Math.abs(a - b) < SNAP) {
          this.snapGuides.push({ x1: b, y1: Math.min(my, ny) - 20, x2: b, y2: Math.max(my + mh, ny + nh) + 20 });
        }
      }
      // y-axis (row) alignment — top edge, bottom edge, vertical center.
      // Mirrors the x-axis loop above; needed here (unlike width) since node
      // heights genuinely vary (e.g. Note nodes via effectiveHeight()).
      for (const [a, b] of [[my, ny], [my, ny + nh - mh], [my + mh / 2, ny + nh / 2]] as [number, number][]) {
        if (Math.abs(a - b) < SNAP) {
          this.snapGuides.push({ x1: Math.min(mx, nx) - 20, y1: b, x2: Math.max(mx + mw, nx + NODE_WIDTH) + 20, y2: b });
        }
      }
    }
  }

  effectiveHeight(node: CanvasNode): number {
    if (node.data.node_type_id !== NODE_IDS.NOTE) return node.height;
    const text = String(node.data.config["text"] ?? "Double-click to edit");
    this.canvas.ctx.font = "12px -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif";
    const lines = wrapText(this.canvas.ctx, text, NODE_WIDTH - 24);
    return Math.max(60, 28 + lines.length * 18);
  }

  overlapsExisting(x: number, y: number, h: number, excludeId?: string): boolean {
    for (const n of this.canvas.nodes.values()) {
      if (n.data.id === excludeId) continue;
      const nx = n.data.position.x, ny = n.data.position.y;
      if (x < nx + NODE_WIDTH && x + NODE_WIDTH > nx && y < ny + this.effectiveHeight(n) && y + h > ny) return true;
    }
    return false;
  }

  avoidOverlap(node: CanvasNode): void {
    const GAP = 30;
    const MAX_NUDGE_TRIES = 30;
    const h = this.effectiveHeight(node);
    const x = node.data.position.x;
    let y = node.data.position.y;
    let tries = 0;
    while (this.overlapsExisting(x, y, h, node.data.id) && tries < MAX_NUDGE_TRIES) {
      y += h + GAP;
      tries++;
    }
    if (tries > 0) {
      node.data.position = { x, y };
      node.updatePortPositions();
    }
  }
}
