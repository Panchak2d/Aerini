import type { Canvas } from "./Canvas";
import type { CanvasNode } from "./Node";

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
    const mw = moving.width, mh = moving.height;
    for (const n of this.canvas.nodes.values()) {
      if (n === moving || n.data.id === moving.data.id) continue;
      if (!this.canvas.isShown(n)) continue;
      const nx = n.data.position.x, ny = n.data.position.y, nh = n.height, nw = n.width;
      const vy1 = Math.min(my, ny) - 20, vy2 = Math.max(my + mh, ny + nh) + 20;
      if (Math.abs(mx - nx) < SNAP) this.addGuide(nx, vy1, nx, vy2);
      const right = nx + nw - mw;
      if (Math.abs(mx - right) < SNAP) this.addGuide(right, vy1, right, vy2);
      const midX = nx + nw / 2;
      if (Math.abs(mx + mw / 2 - midX) < SNAP) this.addGuide(midX, vy1, midX, vy2);
      const hx1 = Math.min(mx, nx) - 20, hx2 = Math.max(mx + mw, nx + nw) + 20;
      if (Math.abs(my - ny) < SNAP) this.addGuide(hx1, ny, hx2, ny);
      const bottom = ny + nh - mh;
      if (Math.abs(my - bottom) < SNAP) this.addGuide(hx1, bottom, hx2, bottom);
      const midY = ny + nh / 2;
      if (Math.abs(my + mh / 2 - midY) < SNAP) this.addGuide(hx1, midY, hx2, midY);
    }
  }

  overlapsExisting(x: number, y: number, w: number, h: number, excludeId?: string): boolean {
    for (const n of this.canvas.nodes.values()) {
      if (n.data.id === excludeId || n.isNote) continue;
      const nx = n.data.position.x, ny = n.data.position.y;
      if (x < nx + n.width && x + w > nx && y < ny + n.height && y + h > ny) return true;
    }
    return false;
  }

  avoidOverlap(node: CanvasNode): void {
    if (node.isNote) return;
    const GAP = 30;
    const MAX_NUDGE_TRIES = 30;
    const h = node.height, w = node.width;
    const x = node.data.position.x;
    let y = node.data.position.y;
    let tries = 0;
    while (this.overlapsExisting(x, y, w, h, node.data.id) && tries < MAX_NUDGE_TRIES) {
      y += h + GAP;
      tries++;
    }
    if (tries > 0) {
      node.data.position = { x, y };
      node.updatePortPositions();
    }
  }
}
