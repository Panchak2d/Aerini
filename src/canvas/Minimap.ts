import type { Canvas } from "./Canvas";
import { NODE_WIDTH } from "./Node";

export class Minimap {
  private canvas: Canvas;
  readonly el:  HTMLCanvasElement | null;
  private ctx:  CanvasRenderingContext2D | null;
  readonly W = 160;
  readonly H = 100;
  private _bounds: { mnX: number; mnY: number; s: number } | null = null;
  private _dragging = false;

  readonly _mmUpH = (): void => { this._dragging = false; };

  constructor(canvas: Canvas, mmEl: HTMLCanvasElement | null) {
    this.canvas = canvas;
    if (mmEl) {
      this.el  = mmEl;
      this.ctx = mmEl.getContext("2d")!;
      mmEl.style.pointerEvents = "all";
      mmEl.addEventListener("mousedown", (e) => this._onDown(e));
      mmEl.addEventListener("mousemove", (e) => this._onMove(e));
      window.addEventListener("mouseup", this._mmUpH);
    } else {
      this.el  = null;
      this.ctx = null;
    }
  }

  draw(vW: number, vH: number): void {
    if (!this.el || !this.ctx) return;
    const mc = this.ctx;
    mc.clearRect(0, 0, this.W, this.H);
    const c = this.canvas;
    if (c.nodes.size === 0) return;

    let mnX = 1e9, mnY = 1e9, mxX = -1e9, mxY = -1e9;
    for (const n of c.nodes.values()) {
      mnX = Math.min(mnX, n.data.position.x - 30);
      mnY = Math.min(mnY, n.data.position.y - 30);
      mxX = Math.max(mxX, n.data.position.x + NODE_WIDTH + 30);
      mxY = Math.max(mxY, n.data.position.y + n.height + 30);
    }
    const ww = mxX - mnX || 1, wh = mxY - mnY || 1;
    const s  = Math.min(this.W / ww, this.H / wh);
    this._bounds = { mnX, mnY, s };
    const tx = (x: number) => (x - mnX) * s;
    const ty = (y: number) => (y - mnY) * s;

    mc.strokeStyle = "#ffffff10"; mc.lineWidth = 1;
    for (const conn of c.connectors.values()) {
      const fn = c.nodes.get(conn.data.from_node), tn = c.nodes.get(conn.data.to_node);
      if (!fn || !tn) continue;
      const fp = fn.ports.find(p => p.id === conn.data.from_port);
      const tp = tn.ports.find(p => p.id === conn.data.to_port);
      if (!fp || !tp) continue;
      mc.beginPath(); mc.moveTo(tx(fp.x), ty(fp.y)); mc.lineTo(tx(tp.x), ty(tp.y)); mc.stroke();
    }
    for (const n of c.nodes.values()) {
      mc.fillStyle = n.selected ? "#4d9eff44" : n.status === "error" ? "#f8717133" : "#21262d";
      mc.fillRect(tx(n.data.position.x), ty(n.data.position.y), Math.max(NODE_WIDTH * s, 3), Math.max(n.height * s, 2));
    }
    const vpX = (-c.panX) / c.zoom, vpY = (-c.panY) / c.zoom;
    mc.strokeStyle = "rgba(77,158,255,0.6)"; mc.lineWidth = 1.5;
    mc.strokeRect(tx(vpX), ty(vpY), (vW / c.zoom) * s, (vH / c.zoom) * s);
  }

  private _clickToPan(e: MouseEvent): void {
    if (!this.el || !this._bounds) return;
    const r = this.el.getBoundingClientRect();
    const mx = e.clientX - r.left;
    const my = e.clientY - r.top;
    const { mnX, mnY, s } = this._bounds;
    const wx = mx / s + mnX;
    const wy = my / s + mnY;
    this.canvas.centerOn(wx, wy);
  }

  private _onDown(e: MouseEvent): void {
    e.preventDefault();
    e.stopPropagation();
    this._dragging = true;
    this._clickToPan(e);
  }

  private _onMove(e: MouseEvent): void {
    if (!this._dragging) return;
    e.preventDefault();
    this._clickToPan(e);
  }
}
