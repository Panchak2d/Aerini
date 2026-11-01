import type { Canvas } from "./Canvas";
import { CanvasNode, NODE_WIDTH, wrapText } from "./Node";
import { NODE_IDS } from "../node-ids";

export class SnapEngine {
  private canvas: Canvas;
  snapGuides: Array<{ x1: number; y1: number; x2: number; y2: number }> = [];

  constructor(canvas: Canvas) {
    this.canvas = canvas;
  }

  private addGuide(x1: number, y1: number, x2: number, y2: number): void {
    for (const g of this.snapGuides) if (g.x1 === x1 && g.y1 === y1 && g.x2 === x2 && g.y2 === y2) return;
    this.snapGuides.push({ x1, y1, x2, y2 });
  }

  computeSnap(moving: CanvasNode): void {
    this.snapGuides.length = 0;
    const SNAP = 8;
    const mx = moving.data.position.x, my = moving.data.position.y;
    const mw = NODE_WIDTH, mh = moving.height;
    for (const n of this.canvas.nodes.values()) {
      if (n === moving || n.data.id === moving.data.id) continue;
      const nx = n.data.position.x, ny = n.data.position.y, nh = n.height;
      const vy1 = Math.min(my, ny) - 20, vy2 = Math.max(my + mh, ny + nh) + 20;
      if (Math.abs(mx - nx) < SNAP) this.addGuide(nx, vy1, nx, vy2);
      const right = nx + NODE_WIDTH - mw;
      if (Math.abs(mx - right) < SNAP) this.addGuide(right, vy1, right, vy2);
      const midX = nx + NODE_WIDTH / 2;
      if (Math.abs(mx + mw / 2 - midX) < SNAP) this.addGuide(midX, vy1, midX, vy2);
      const hx1 = Math.min(mx, nx) - 20, hx2 = Math.max(mx + mw, nx + NODE_WIDTH) + 20;
      if (Math.abs(my - ny) < SNAP) this.addGuide(hx1, ny, hx2, ny);
      const bottom = ny + nh - mh;
      if (Math.abs(my - bottom) < SNAP) this.addGuide(hx1, bottom, hx2, bottom);
      const midY = ny + nh / 2;
      if (Math.abs(my + mh / 2 - midY) < SNAP) this.addGuide(hx1, midY, hx2, midY);
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
