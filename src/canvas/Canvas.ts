import { CanvasNode, createNodeFromDescriptor, NODE_WIDTH, NODE_HEADER, PORT_RADIUS } from "./Node";
import { Connector, PendingConnector, newConnectorId } from "./Connector";
import { serialize } from "./CanvasSerializer";
import type { NodeDescriptor } from "../ipc/workflow";

type MoveEntry = { nodeId: string; from: { x: number; y: number }; to: { x: number; y: number } };

type UndoAction =
  | { type: "add_node";    node: CanvasNode }
  | { type: "delete_node"; node: CanvasNode; connectors: Connector[] }
  | { type: "add_edge";    connector: Connector }
  | { type: "delete_edge"; connector: Connector }
  | { type: "split_edge";  removed: Connector; added1: Connector; added2: Connector; node: CanvasNode }
  | { type: "move_node";   nodeId: string; from: { x: number; y: number }; to: { x: number; y: number } }
  | { type: "move_nodes";  moves: MoveEntry[] }
  | { type: "cut_edges";   connectors: Connector[] };

export class Canvas {
  private el:  HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private dpr = 1;

  nodes:      Map<string, CanvasNode> = new Map();
  connectors: Map<string, Connector>  = new Map();

  panX = 0; panY = 0; zoom = 1;
  private readonly MIN_ZOOM = 0.05;
  private readonly MAX_ZOOM = 4;

  private isPanning   = false;
  private panStartX   = 0; private panStartY  = 0;
  private panOriginX  = 0; private panOriginY = 0;

  private draggingNode: CanvasNode | null = null;
  private dragOffX = 0; private dragOffY = 0;
  private dragFromX = 0; private dragFromY = 0;
  private didDrag = false;

  private pendingConn: PendingConnector | null = null;
  private reconnEdge: { conn: Connector; fromEnd?: boolean } | null = null;

  private isCutting = false;
  private cutPath: { x: number; y: number }[] = [];

  private isBoxSel = false; // Shift+drag only
  private bx0 = 0; private by0 = 0; private bx1 = 0; private by1 = 0;

  private snapGuides: Array<{ x1: number; y1: number; x2: number; y2: number }> = [];

  selectedNodes: Set<string> = new Set();
  selectedNode:  CanvasNode | null = null;
  selectedConn:  Connector  | null = null;

  private shiftHeld = false;

  private undoStack: UndoAction[] = [];
  private redoStack: UndoAction[] = [];

  private mm:    HTMLCanvasElement | null = null;
  private mmCtx: CanvasRenderingContext2D | null = null;
  private readonly MM_W = 160;
  private readonly MM_H = 100;
  // Cached minimap world bounds for click-to-pan
  private _mmBounds: { mnX: number; mnY: number; s: number } | null = null;
  private _mmDragging = false;

  pendingInsert: NodeDescriptor | null = null;
  insertGhost:   { x: number; y: number } | null = null;

  private focusMode = false;

  _pendingWireDrop: { fromNode: string; fromPort: string; wx: number; wy: number } | null = null;
  _pendingPresetConfig: Record<string, unknown> | undefined = undefined;
  _pendingPresetName:   string | undefined = undefined;

  private _multiDragFrom: Map<string, { x: number; y: number }> | null = null;

  private lastPinchDist = 0;
  private isPinching    = false;

  private emptyEl: HTMLElement | null = null;

  onPaletteRequest: (() => void) | null = null;
  onWireDropRequest: ((fromNode: string, fromPort: string, wx: number, wy: number) => void) | null = null;
  onNodeSelected:   ((n: CanvasNode | null) => void) | null = null;
  onNodeClicked:    ((n: CanvasNode) => void) | null = null;
  onCanvasChanged:  (() => void) | null = null;
  onPanelClose:     (() => void) | null = null;
  onRunNode:        ((nodeId: string) => void) | null = null;

  private lastTs = 0;

  // Stored so they can be removed in destroy()
  private readonly _winMoveH = (e: MouseEvent)    => this.onWinMove(e);
  private readonly _winUpH   = (e: MouseEvent)    => this.onWinUp(e);
  private readonly _keyH     = (e: KeyboardEvent) => this.onKey(e);
  private readonly _keyUpH   = (e: KeyboardEvent) => { if (e.key === "Shift") { this.shiftHeld = false; this.el.classList.remove("shift-held"); } };
  private readonly _mmUpH    = () => { this._mmDragging = false; };

  constructor(canvasEl: HTMLCanvasElement) {
    this.el  = canvasEl;
    this.ctx = canvasEl.getContext("2d")!;

    const mmEl = document.getElementById("minimap-canvas") as HTMLCanvasElement | null;
    if (mmEl) {
      this.mm = mmEl;
      this.mmCtx = mmEl.getContext("2d")!;
      // Click-to-pan: clicking the minimap centers the viewport on that world point
      mmEl.style.pointerEvents = "all";
      mmEl.addEventListener("mousedown", (e) => this.onMinimapDown(e));
      mmEl.addEventListener("mousemove", (e) => this.onMinimapMove(e));
      window.addEventListener("mouseup", this._mmUpH);
    }

    this.emptyEl = document.getElementById("canvas-empty-state");
    this.resize();
    this.bind();
    requestAnimationFrame((t) => this.loop(t));
  }

  // ── Resize ────────────────────────────────────────────────────────────────

  resize() {
    this.dpr = window.devicePixelRatio || 1;
    const r  = this.el.getBoundingClientRect();
    // Only update the internal pixel buffer — do NOT set style.width/style.height.
    // Those CSS properties override width:100%;height:100% and freeze the canvas
    // at the size it had when resize() was first called (e.g. at a small window size).
    // CSS handles the display dimensions; we only control the pixel density here.
    this.el.width  = Math.round(r.width  * this.dpr);
    this.el.height = Math.round(r.height * this.dpr);
    // Center pan on first load
    if (this.panX === 0 && this.panY === 0) {
      this.panX = r.width  / 2;
      this.panY = r.height / 2;
    }
  }

  private s2w(sx: number, sy: number) {
    return { x: (sx - this.panX) / this.zoom, y: (sy - this.panY) / this.zoom };
  }

  private evSX(e: MouseEvent) {
    const r = this.el.getBoundingClientRect();
    return { sx: e.clientX - r.left, sy: e.clientY - r.top };
  }

  // ── Loop ──────────────────────────────────────────────────────────────────

  private loop(ts: number) {
    const dt = Math.min((ts - this.lastTs) / 1000, 0.1);
    this.lastTs = ts;
    this.draw(dt);
    requestAnimationFrame((t) => this.loop(t));
  }

  private draw(dt: number) {
    const dpr = this.dpr;
    const W   = this.el.width  / dpr;
    const H   = this.el.height / dpr;
    const ctx = this.ctx;

    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    ctx.setTransform(dpr * this.zoom, 0, 0, dpr * this.zoom, this.panX * dpr, this.panY * dpr);

    // Draw connectors
    for (const c of this.connectors.values()) {
      c.highlighted = this.isCutting && this.edgeCrossesPath(c);
      c.draw(ctx, this.nodes, dt);
    }
    if (this.pendingConn) this.pendingConn.draw(ctx);

    // Port snap ring
    if (this.pendingConn) {
      const snap = this.nearestIn(this.pendingConn.toX, this.pendingConn.toY, this.pendingConn.fromNode);
      if (snap) {
        ctx.save();
        ctx.beginPath();
        ctx.arc(snap.x, snap.y, PORT_RADIUS + 7, 0, Math.PI * 2);
        ctx.strokeStyle = "#4d9effcc";
        ctx.lineWidth   = 2 / this.zoom;
        ctx.stroke();
        ctx.beginPath();
        ctx.arc(snap.x, snap.y, PORT_RADIUS + 3, 0, Math.PI * 2);
        ctx.fillStyle = "#4d9eff22";
        ctx.fill();
        ctx.restore();
      }
    }

    // Draw nodes
    for (const n of this.nodes.values()) n.draw(ctx, dt);

    // ── G12: output port connection-count badges ───────────────────────────
    // Count outgoing connections per (nodeId, portId).
    const outCount = new Map<string, number>();
    for (const c of this.connectors.values()) {
      const key = `${c.data.from_node}:${c.data.from_port}`;
      outCount.set(key, (outCount.get(key) ?? 0) + 1);
    }
    for (const [key, cnt] of outCount) {
      if (cnt < 2) continue;
      const colonIdx = key.indexOf(":");
      const nodeId   = key.slice(0, colonIdx);
      const portId   = key.slice(colonIdx + 1);
      const node     = this.nodes.get(nodeId);
      if (!node) continue;
      const port     = node.ports.find(p => p.id === portId && !p.isInput);
      if (!port) continue;

      const BADGE_R = 7;
      const bx      = port.x + PORT_RADIUS + 1;
      const by      = port.y - PORT_RADIUS - 1;

      ctx.save();
      ctx.beginPath();
      ctx.arc(bx, by, BADGE_R, 0, Math.PI * 2);
      ctx.fillStyle   = "#4d9eff";
      ctx.shadowColor = "rgba(0,0,0,0.6)";
      ctx.shadowBlur  = 3 / this.zoom;
      ctx.fill();
      ctx.shadowBlur  = 0;

      ctx.font         = "bold 8px -apple-system, BlinkMacSystemFont, sans-serif";
      ctx.textAlign    = "center";
      ctx.textBaseline = "middle";
      ctx.fillStyle    = "#ffffff";
      ctx.fillText(String(cnt > 9 ? "9+" : cnt), bx, by);
      ctx.restore();
    }

    // Draw ghost
    if (this.pendingInsert && this.insertGhost) {
      ctx.globalAlpha = 0.35;
      const d = createNodeFromDescriptor(this.pendingInsert, this.insertGhost.x - NODE_WIDTH / 2, this.insertGhost.y - 20);
      d.draw(ctx, 0);
      ctx.globalAlpha = 1;
    }

    // Box select rect
    if (this.isBoxSel) {
      ctx.save();
      const bx = Math.min(this.bx0, this.bx1), by = Math.min(this.by0, this.by1);
      const bw = Math.abs(this.bx1 - this.bx0), bh = Math.abs(this.by1 - this.by0);
      ctx.fillStyle   = "rgba(77,158,255,0.06)";
      ctx.strokeStyle = "#4d9eff";
      ctx.lineWidth   = 1.5 / this.zoom;
      ctx.fillRect(bx, by, bw, bh);
      ctx.strokeRect(bx, by, bw, bh);
      ctx.restore();
    }

    // Cut path
    if (this.isCutting && this.cutPath.length > 1) {
      ctx.save();
      ctx.beginPath();
      ctx.moveTo(this.cutPath[0].x, this.cutPath[0].y);
      for (let i = 1; i < this.cutPath.length; i++) ctx.lineTo(this.cutPath[i].x, this.cutPath[i].y);
      ctx.strokeStyle = "#f87171";
      ctx.lineWidth   = 2 / this.zoom;
      ctx.setLineDash([5 / this.zoom, 3 / this.zoom]);
      ctx.stroke();
      ctx.restore();
    }

    // Snap guides
    if (this.snapGuides.length && this.draggingNode) {
      ctx.save();
      ctx.strokeStyle = "#4d9eff66";
      ctx.lineWidth   = 1 / this.zoom;
      for (const g of this.snapGuides) {
        ctx.beginPath();
        ctx.moveTo(g.x1, g.y1);
        ctx.lineTo(g.x2, g.y2);
        ctx.stroke();
      }
      ctx.restore();
    }

    // Empty state
    if (this.emptyEl) {
      this.emptyEl.style.display = this.nodes.size === 0 ? "flex" : "none";
    }

    this.drawMinimap(W, H);
  }

  // ── Minimap ───────────────────────────────────────────────────────────────

  private drawMinimap(vW: number, vH: number) {
    if (!this.mm || !this.mmCtx) return;
    const mc = this.mmCtx;
    mc.clearRect(0, 0, this.MM_W, this.MM_H);
    if (this.nodes.size === 0) return;

    let mnX = 1e9, mnY = 1e9, mxX = -1e9, mxY = -1e9;
    for (const n of this.nodes.values()) {
      mnX = Math.min(mnX, n.data.position.x - 30);
      mnY = Math.min(mnY, n.data.position.y - 30);
      mxX = Math.max(mxX, n.data.position.x + NODE_WIDTH + 30);
      mxY = Math.max(mxY, n.data.position.y + n.height + 30);
    }
    const ww = mxX - mnX || 1, wh = mxY - mnY || 1;
    const s  = Math.min(this.MM_W / ww, this.MM_H / wh);
    // Cache bounds so minimap click handlers can convert mm coords → world coords
    this._mmBounds = { mnX, mnY, s };
    const tx = (x: number) => (x - mnX) * s;
    const ty = (y: number) => (y - mnY) * s;

    mc.strokeStyle = "#ffffff10"; mc.lineWidth = 1;
    for (const c of this.connectors.values()) {
      const fn = this.nodes.get(c.data.from_node), tn = this.nodes.get(c.data.to_node);
      if (!fn || !tn) continue;
      const fp = fn.ports.find(p => p.id === c.data.from_port), tp = tn.ports.find(p => p.id === c.data.to_port);
      if (!fp || !tp) continue;
      mc.beginPath(); mc.moveTo(tx(fp.x), ty(fp.y)); mc.lineTo(tx(tp.x), ty(tp.y)); mc.stroke();
    }
    for (const n of this.nodes.values()) {
      mc.fillStyle = n.selected ? "#4d9eff44" : n.status === "error" ? "#f8717133" : "#21262d";
      mc.fillRect(tx(n.data.position.x), ty(n.data.position.y), Math.max(NODE_WIDTH * s, 3), Math.max(n.height * s, 2));
    }
    const vpX = (-this.panX) / this.zoom, vpY = (-this.panY) / this.zoom;
    mc.strokeStyle = "rgba(77,158,255,0.6)"; mc.lineWidth = 1.5;
    mc.strokeRect(tx(vpX), ty(vpY), (vW / this.zoom) * s, (vH / this.zoom) * s);
  }

  // ── Minimap interaction ───────────────────────────────────────────────────

  private mmClickToPan(e: MouseEvent): void {
    if (!this.mm || !this._mmBounds) return;
    const r = this.mm.getBoundingClientRect();
    const mx = e.clientX - r.left;
    const my = e.clientY - r.top;
    const { mnX, mnY, s } = this._mmBounds;
    // Convert minimap pixel → world coord
    const wx = mx / s + mnX;
    const wy = my / s + mnY;
    this.centerOn(wx, wy);
  }

  private onMinimapDown(e: MouseEvent): void {
    e.preventDefault();
    e.stopPropagation();
    this._mmDragging = true;
    this.mmClickToPan(e);
  }

  private onMinimapMove(e: MouseEvent): void {
    if (!this._mmDragging) return;
    e.preventDefault();
    this.mmClickToPan(e);
  }

  // ── Snap ──────────────────────────────────────────────────────────────────

  private computeSnap(moving: CanvasNode) {
    this.snapGuides = [];
    const SNAP = 8, mx = moving.data.position.x, my = moving.data.position.y, mw = NODE_WIDTH, mh = moving.height;
    for (const n of this.nodes.values()) {
      if (n.data.id === moving.data.id) continue;
      const nx = n.data.position.x, ny = n.data.position.y, nh = n.height;
      for (const [a, b] of [[mx, nx], [mx, nx + NODE_WIDTH - mw], [mx + mw / 2, nx + NODE_WIDTH / 2]] as [number, number][]) {
        if (Math.abs(a - b) < SNAP) this.snapGuides.push({ x1: b, y1: Math.min(my, ny) - 20, x2: b, y2: Math.max(my + mh, ny + nh) + 20 });
      }
    }
  }

  // ── Port snap ─────────────────────────────────────────────────────────────

  private nearestIn(wx: number, wy: number, excludeId?: string) {
    const SNAP = 56; let best: { x: number; y: number; nodeId: string; portId: string } | null = null; let bestD = SNAP;
    for (const n of this.nodes.values()) {
      if (n.data.id === excludeId) continue;
      for (const p of n.ports) {
        if (!p.isInput) continue;
        const d = Math.hypot(wx - p.x, wy - p.y);
        if (d < bestD) { bestD = d; best = { x: p.x, y: p.y, nodeId: n.data.id, portId: p.id }; }
      }
    }
    return best;
  }

  // ── Wire-drop insert (called for NEW nodes AND after DRAGGING existing ones) ──

  tryWireInsert(node: CanvasNode): boolean {
    node.rebuildPorts();
    const nx = node.data.position.x + NODE_WIDTH / 2;
    const ny = node.data.position.y + node.height / 2;

    for (const [id, conn] of this.connectors) {
      if (conn.data.from_node === node.data.id || conn.data.to_node === node.data.id) continue;
      const fn = this.nodes.get(conn.data.from_node), tn = this.nodes.get(conn.data.to_node);
      if (!fn || !tn) continue;
      const fp = fn.ports.find(p => p.id === conn.data.from_port), tp = tn.ports.find(p => p.id === conn.data.to_port);
      if (!fp || !tp) continue;
      const cp = Math.max(Math.abs(tp.x - fp.x) * 0.55, 80);
      let near = false;
      for (let t = 0; t <= 1; t += 0.04) {
        if (Math.hypot(nx - bz(fp.x, fp.x + cp, tp.x - cp, tp.x, t), ny - bz(fp.y, fp.y, tp.y, tp.y, t)) < 52) { near = true; break; }
      }
      if (!near) continue;
      const inP  = node.ports.find(p => p.isInput);
      const outP = node.ports.find(p => !p.isInput);
      if (!inP || !outP) continue;
      this.connectors.delete(id);
      const e1 = new Connector({ id: newConnectorId(), from_node: conn.data.from_node, from_port: conn.data.from_port, to_node: node.data.id, to_port: inP.id, condition: null, on_success: null, on_failure: null });
      const e2 = new Connector({ id: newConnectorId(), from_node: node.data.id, from_port: outP.id, to_node: conn.data.to_node, to_port: conn.data.to_port, condition: null, on_success: null, on_failure: null });
      this.connectors.set(e1.data.id, e1); this.connectors.set(e2.data.id, e2);
      this.pushUndo({ type: "split_edge", removed: conn, added1: e1, added2: e2, node });
      this.onCanvasChanged?.(); return true;
    }
    return false;
  }

  // ── Events ────────────────────────────────────────────────────────────────

  private bind() {
    // ResizeObserver fires on ANY size change of the canvas element:
    // window maximize/restore, panel open/close, drawer show/hide.
    // This replaces the old window 'resize' listener which missed grid reflows.
    const ro = new ResizeObserver(() => this.resize());
    ro.observe(this.el);
    this.el.addEventListener("mousedown",  (e) => this.onDown(e));
    this.el.addEventListener("mousemove",  (e) => this.onMove(e));
    this.el.addEventListener("mouseup",    (e) => this.onUp(e));
    this.el.addEventListener("mouseleave", ()  => this.onLeave());
    window.addEventListener("mousemove",   this._winMoveH);
    window.addEventListener("mouseup",     this._winUpH);
    this.el.addEventListener("wheel",      (e) => this.onWheel(e), { passive: false });
    this.el.addEventListener("dblclick",   (e) => this.onDbl(e));
    this.el.addEventListener("contextmenu",(e) => { e.preventDefault(); this.onRightClick(e); });
    this.el.addEventListener("touchstart", (e) => this.onTouchStart(e), { passive: false });
    this.el.addEventListener("touchmove",  (e) => this.onTouchMove(e),  { passive: false });
    this.el.addEventListener("touchend",   ()  => this.onTouchEnd());
    window.addEventListener("keydown", this._keyH);
    window.addEventListener("keyup",   this._keyUpH);
  }

  destroy() {
    window.removeEventListener("mousemove", this._winMoveH);
    window.removeEventListener("mouseup",   this._winUpH);
    window.removeEventListener("mouseup",   this._mmUpH);
    window.removeEventListener("keydown",   this._keyH);
    window.removeEventListener("keyup",     this._keyUpH);
  }

  private onKey(e: KeyboardEvent) {
    const inInput = document.activeElement instanceof HTMLInputElement || document.activeElement instanceof HTMLTextAreaElement;
    if (e.key === "Shift") { this.shiftHeld = true; this.el.classList.add("shift-held"); }
    if (inInput) return;

    if ((e.metaKey || e.ctrlKey) && e.key === "z" && !e.shiftKey) { e.preventDefault(); this.undo(); }
    if ((e.metaKey || e.ctrlKey) && (e.key === "y" || (e.key === "z" && e.shiftKey))) { e.preventDefault(); this.redo(); }
    if ((e.metaKey || e.ctrlKey) && e.key === "d") { e.preventDefault(); this.dupSelected(); }
    if ((e.metaKey || e.ctrlKey) && e.key === "a") {
      e.preventDefault();
      for (const n of this.nodes.values()) { n.selected = true; this.selectedNodes.add(n.data.id); }
    }
    if (e.key === "Delete" || e.key === "Backspace") this.deleteSelected();
    if (e.key === "f" || e.key === "F") this.toggleFocusMode();
    if ((e.code === "Space" && !e.altKey && !e.ctrlKey && !e.metaKey) || ((e.metaKey || e.ctrlKey) && e.key === "k")) {
      e.preventDefault(); this.onPaletteRequest?.();
    }
    if (e.key === "Escape") {
      this.pendingConn = null; this.isCutting = false; this.cutPath = [];
      this.pendingInsert = null; this.insertGhost = null;
      this.clearSelection(); this.el.style.cursor = "default";
    }
  }

  private onDown(e: MouseEvent) {
    e.preventDefault();
    const { sx, sy } = this.evSX(e);
    const { x: wx, y: wy } = this.s2w(sx, sy);

    if (e.button === 0 && e.altKey) { this.isCutting = true; this.cutPath = [{ x: wx, y: wy }]; this.el.style.cursor = "crosshair"; return; }
    if (e.button === 1) { this.startPan(sx, sy); return; }
    if (e.button !== 0) return;

    // Grab existing connector — input end (to_port) or output end (from_port) — to reroute or disconnect
    for (const conn of this.connectors.values()) {
      const tn = this.nodes.get(conn.data.to_node); if (!tn) continue;
      const tp = tn.ports.find(p => p.id === conn.data.to_port); if (!tp) continue;
      const fn = this.nodes.get(conn.data.from_node); if (!fn) continue;
      const fp = fn.ports.find(p => p.id === conn.data.from_port); if (!fp) continue;

      // Input-end grab: grab near the destination input port, wire floats from source
      if (Math.hypot(wx - tp.x, wy - tp.y) < PORT_RADIUS + 8) {
        this.reconnEdge = { conn }; this.connectors.delete(conn.data.id);
        this.clearDynamicPortExpr(conn);
        this.pendingConn = new PendingConnector(conn.data.from_node, conn.data.from_port, fp.x, fp.y);
        this.pendingConn.toX = wx; this.pendingConn.toY = wy;
        this.el.style.cursor = "crosshair"; return;
      }
      // Output-end grab: grab near the source output port, wire floats from source
      if (Math.hypot(wx - fp.x, wy - fp.y) < PORT_RADIUS + 8) {
        this.connectors.delete(conn.data.id);
        this.clearDynamicPortExpr(conn);
        this.reconnEdge = { conn, fromEnd: true };
        this.pendingConn = new PendingConnector(conn.data.from_node, conn.data.from_port, fp.x, fp.y);
        this.pendingConn.toX = wx; this.pendingConn.toY = wy;
        this.el.style.cursor = "crosshair"; return;
      }
    }

    // Output port → new connector
    for (const node of this.nodes.values()) {
      const p = node.portAtPoint(wx, wy);
      if (p && !p.isInput) { this.pendingConn = new PendingConnector(node.data.id, p.id, p.x, p.y); this.el.style.cursor = "crosshair"; return; }
    }

    // Node drag
    const arr = Array.from(this.nodes.values());
    for (let i = arr.length - 1; i >= 0; i--) {
      const n = arr[i];
      if (!n.containsPoint(wx, wy)) continue;
      if (!e.shiftKey && !this.selectedNodes.has(n.data.id)) this.clearSelection();
      this.selectNode(n);
      this.onNodeClicked?.(n);  // single-click: reveal output/error without opening config
      this.draggingNode = n;
      this.dragOffX = wx - n.data.position.x; this.dragOffY = wy - n.data.position.y;
      this.dragFromX = n.data.position.x; this.dragFromY = n.data.position.y;
      if (this.selectedNodes.size > 1) {
        this._multiDragFrom = new Map();
        for (const id of this.selectedNodes) {
          const mn = this.nodes.get(id);
          if (mn) this._multiDragFrom.set(id, { ...mn.data.position });
        }
      } else {
        this._multiDragFrom = null;
      }
      this.didDrag = false; this.el.style.cursor = "grab"; return;
    }

    // Wire click
    for (const conn of this.connectors.values()) { if (conn.containsPoint(wx, wy, this.nodes)) { this.selectConn(conn); return; } }

    // Empty space: Shift = box-select, plain = pan + deselect
    if (e.shiftKey) {
      this.isBoxSel = true; this.bx0 = wx; this.by0 = wy; this.bx1 = wx; this.by1 = wy;
      this.el.style.cursor = "crosshair";
    } else {
      this.clearSelection();
      this.startPan(sx, sy);
    }
  }

  private startPan(sx: number, sy: number) {
    this.isPanning = true; this.panStartX = sx; this.panStartY = sy;
    this.panOriginX = this.panX; this.panOriginY = this.panY;
    this.el.style.cursor = "grabbing";
  }

  private onMove(e: MouseEvent) { const { sx, sy } = this.evSX(e); this._move(e, sx, sy); }
  private onWinMove(e: MouseEvent) {
    if (!this.isPanning && !this.draggingNode && !this.pendingConn && !this.isCutting && !this.isBoxSel) return;
    const r = this.el.getBoundingClientRect(); this._move(e, e.clientX - r.left, e.clientY - r.top);
  }

  private _move(_e: MouseEvent, sx: number, sy: number) {
    const { x: wx, y: wy } = this.s2w(sx, sy);
    if (this.pendingInsert) this.insertGhost = { x: wx, y: wy };

    if (this.isPanning) { this.panX = this.panOriginX + (sx - this.panStartX); this.panY = this.panOriginY + (sy - this.panStartY); this.el.style.cursor = "grabbing"; return; }
    if (this.isCutting) { this.cutPath.push({ x: wx, y: wy }); return; }

    if (this.draggingNode) {
      this.didDrag = true;
      let nx = wx - this.dragOffX, ny = wy - this.dragOffY;
      if (localStorage.getItem("flowo_grid_snap") === "true") {
        const G = 20;
        nx = Math.round(nx / G) * G;
        ny = Math.round(ny / G) * G;
      }
      if (this.selectedNodes.size > 1 && this.selectedNodes.has(this.draggingNode.data.id)) {
        const dx = nx - this.draggingNode.data.position.x, dy = ny - this.draggingNode.data.position.y;
        for (const id of this.selectedNodes) { const n = this.nodes.get(id); if (n) { n.data.position.x += dx; n.data.position.y += dy; n.updatePortPositions(); } }
      } else {
        this.draggingNode.data.position.x = nx; this.draggingNode.data.position.y = ny;
        this.draggingNode.updatePortPositions(); this.computeSnap(this.draggingNode);
      }
      this.onCanvasChanged?.(); this.el.style.cursor = "grabbing"; return;
    }

    if (this.pendingConn) {
      const snap = this.nearestIn(wx, wy, this.pendingConn.fromNode);
      this.pendingConn.toX = snap ? snap.x : wx; this.pendingConn.toY = snap ? snap.y : wy; return;
    }

    if (this.isBoxSel) {
      this.bx1 = wx; this.by1 = wy;
      const x1 = Math.min(this.bx0, wx), y1 = Math.min(this.by0, wy), x2 = Math.max(this.bx0, wx), y2 = Math.max(this.by0, wy);
      for (const n of this.nodes.values()) {
        const ins = n.data.position.x >= x1 && n.data.position.x <= x2 && n.data.position.y >= y1 && n.data.position.y <= y2;
        n.selected = ins; if (ins) this.selectedNodes.add(n.data.id); else this.selectedNodes.delete(n.data.id);
      }
      return;
    }

    let any = false;
    for (const n of this.nodes.values()) {
      n.hovered = n.containsPoint(wx, wy);
      if (n.hovered) { any = true; this.el.style.cursor = n.portAtPoint(wx, wy) ? "crosshair" : "grab"; }
    }
    if (!any) this.el.style.cursor = this.shiftHeld ? "crosshair" : "default";
  }

  private onUp(e: MouseEvent) { const { sx, sy } = this.evSX(e); this._up(e, sx, sy); }
  private onWinUp(e: MouseEvent) {
    if (!this.isPanning && !this.draggingNode && !this.pendingConn && !this.isCutting && !this.isBoxSel) return;
    const r = this.el.getBoundingClientRect(); this._up(e, e.clientX - r.left, e.clientY - r.top);
  }

  private _up(_e: MouseEvent, sx: number, sy: number) {
    const { x: wx, y: wy } = this.s2w(sx, sy);

    if (this.pendingInsert && sx >= 0 && sy >= 0) {
      this.placeNode(this.pendingInsert, sx, sy);
      this.pendingInsert = null; this.insertGhost = null; this.el.style.cursor = "default"; return;
    }
    this.pendingInsert = null; this.insertGhost = null;

    if (this.isPanning) { this.isPanning = false; this.el.style.cursor = "default"; return; }

    if (this.isCutting) {
      const cut = this.doCut();
      if (cut.length) { this.pushUndo({ type: "cut_edges", connectors: cut }); this.onCanvasChanged?.(); }
      this.isCutting = false; this.cutPath = []; this.el.style.cursor = "default"; return;
    }

    if (this.pendingConn) {
      const snap = this.nearestIn(wx, wy, this.pendingConn.fromNode);
      if (snap) {
        this.finishConn(snap.nodeId, snap.portId);
      } else if (this.reconnEdge) {
        // Either end dropped on empty space = disconnect (Ctrl+Z to undo)
        this.pushUndo({ type: "delete_edge", connector: this.reconnEdge.conn });
        this.onCanvasChanged?.();
      } else {
        // Wire dropped on empty canvas → open node picker with wire context
        const { fromNode, fromPort } = this.pendingConn;
        this._pendingWireDrop = { fromNode, fromPort, wx, wy };
        this.onWireDropRequest?.(fromNode, fromPort, wx, wy);
      }
      this.reconnEdge = null; this.pendingConn = null; this.el.style.cursor = "default"; return;
    }

    if (this.draggingNode) {
      this.snapGuides = [];
      if (this.didDrag) {
        if (this.selectedNodes.size > 1 && this._multiDragFrom) {
          const moves: MoveEntry[] = [];
          for (const [id, from] of this._multiDragFrom) {
            const mn = this.nodes.get(id);
            if (mn && (mn.data.position.x !== from.x || mn.data.position.y !== from.y))
              moves.push({ nodeId: id, from, to: { ...mn.data.position } });
          }
          if (moves.length) this.pushUndo({ type: "move_nodes", moves });
        } else {
          const p = this.draggingNode.data.position;
          if (p.x !== this.dragFromX || p.y !== this.dragFromY)
            this.pushUndo({ type: "move_node", nodeId: this.draggingNode.data.id, from: { x: this.dragFromX, y: this.dragFromY }, to: { ...p } });
        }
      }
      this._multiDragFrom = null;
      this.draggingNode = null; this.el.style.cursor = "default"; return;
    }

    if (this.isBoxSel) { this.isBoxSel = false; this.el.style.cursor = this.shiftHeld ? "crosshair" : "default"; }
  }

  private onLeave() {
    if (!this.isPanning && !this.draggingNode && !this.isCutting)
      for (const n of this.nodes.values()) n.hovered = false;
  }

  // Right-click: node context menu, wire delete, or deselect
  private onRightClick(e: MouseEvent) {
    const { sx, sy } = this.evSX(e);
    const { x: wx, y: wy } = this.s2w(sx, sy);

    // Right-click on a node → context menu
    const arr = Array.from(this.nodes.values());
    for (let i = arr.length - 1; i >= 0; i--) {
      const n = arr[i];
      if (n.containsPoint(wx, wy)) {
        if (!this.selectedNodes.has(n.data.id)) {
          this.clearSelection();
          this.selectNode(n);
        }
        this.showNodeContextMenu(e.clientX, e.clientY, n);
        return;
      }
    }

    // Right-click on a wire → delete it
    for (const [id, conn] of this.connectors) {
      if (conn.containsPoint(wx, wy, this.nodes)) {
        this.connectors.delete(id);
        this.clearDynamicPortExpr(conn);
        this.pushUndo({ type: "delete_edge", connector: conn });
        this.onCanvasChanged?.(); return;
      }
    }

    // Right-click on empty canvas → deselect
    this.clearSelection();
  }

  private showNodeContextMenu(cx: number, cy: number, node: CanvasNode): void {
    // Remove any existing menu
    document.getElementById("canvas-ctx-menu")?.remove();

    const isMultiSelect = this.selectedNodes.size > 1;

    const menu = document.createElement("div");
    menu.id = "canvas-ctx-menu";
    menu.className = "ctx-menu";

    const addItem = (label: string, icon: string, danger: boolean, action: () => void) => {
      const item = document.createElement("button");
      item.className = "ctx-menu-item" + (danger ? " ctx-menu-item--danger" : "");
      item.innerHTML = `<span class="ctx-menu-icon">${icon}</span><span>${label}</span>`;
      item.addEventListener("mousedown", (e) => { e.preventDefault(); menu.remove(); action(); });
      menu.appendChild(item);
    };

    const addSep = () => {
      const s = document.createElement("div");
      s.className = "ctx-menu-sep";
      menu.appendChild(s);
    };

    // Duplicate
    addItem(
      isMultiSelect ? `Duplicate (${this.selectedNodes.size})` : "Duplicate",
      `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>`,
      false,
      () => { this.dupSelected(); this.onCanvasChanged?.(); }
    );

    // Rename — only for single selection
    if (!isMultiSelect) {
      addItem(
        "Rename",
        `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>`,
        false,
        () => this.startInlineRename(node)
      );
    }

    // Disable / Enable
    addItem(
      node.disabled ? "Enable" : "Disable",
      node.disabled
        ? `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="20 6 9 17 4 12"/></svg>`
        : `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`,
      false,
      () => {
        const ids = isMultiSelect ? [...this.selectedNodes] : [node.data.id];
        for (const id of ids) {
          const n = this.nodes.get(id);
          if (n) n.disabled = !n.disabled;
        }
        this.onCanvasChanged?.();
      }
    );

    // Run node — only for single selection, not trigger-less nodes
    if (!isMultiSelect) {
      addItem(
        "Run from here",
        `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg>`,
        false,
        () => { this.onRunNode?.(node.data.id); }
      );
    }

    addSep();

    // Delete
    addItem(
      isMultiSelect ? `Delete (${this.selectedNodes.size})` : "Delete",
      `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/><path d="M9 6V4h6v2"/></svg>`,
      true,
      () => { this.deleteSelected(); }
    );

    // Position the menu
    document.body.appendChild(menu);
    const mw = menu.offsetWidth || 180;
    const mh = menu.offsetHeight || 160;
    const left = cx + mw > window.innerWidth  - 8 ? cx - mw : cx;
    const top  = cy + mh > window.innerHeight - 8 ? cy - mh : cy;
    menu.style.left = `${left}px`;
    menu.style.top  = `${top}px`;

    // Close on any outside click
    const dismiss = (ev: MouseEvent) => {
      if (!menu.contains(ev.target as Node)) {
        menu.remove();
        document.removeEventListener("mousedown", dismiss, true);
      }
    };
    setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);
  }

  private startInlineRename(node: CanvasNode): void {
    document.getElementById("canvas-rename-input")?.remove();

    const r = this.el.getBoundingClientRect();
    const sx = node.data.position.x * this.zoom + this.panX + r.left;
    const sy = node.data.position.y * this.zoom + this.panY + r.top;
    const sw = NODE_WIDTH * this.zoom;
    const sh = NODE_HEADER * this.zoom;

    const inp = document.createElement("input");
    inp.id = "canvas-rename-input";
    inp.type = "text";
    inp.value = node.data.name;
    inp.className = "canvas-rename-input";
    inp.style.cssText = `position:fixed;left:${sx + 30 * this.zoom}px;top:${sy + sh / 2 - 11}px;width:${Math.max(sw - 50 * this.zoom, 80)}px;`;
    document.body.appendChild(inp);
    inp.focus(); inp.select();

    const commit = () => {
      const v = inp.value.trim();
      if (v) { node.data.name = v; this.onCanvasChanged?.(); }
      inp.remove();
    };
    inp.addEventListener("blur", commit);
    inp.addEventListener("keydown", (e) => {
      if (e.key === "Enter")  { e.preventDefault(); commit(); }
      if (e.key === "Escape") { inp.remove(); }
    });
  }

  // ── Zoom / Touch ──────────────────────────────────────────────────────────

  private onWheel(e: WheelEvent) {
    e.preventDefault();
    const r = this.el.getBoundingClientRect();
    const sx = e.clientX - r.left, sy = e.clientY - r.top;
    const { x: wx, y: wy } = this.s2w(sx, sy);
    const isPinch = e.ctrlKey;
    const factor  = e.deltaY > 0 ? (isPinch ? 0.95 : 0.90) : (isPinch ? 1 / 0.95 : 1 / 0.90);
    const nz = Math.min(this.MAX_ZOOM, Math.max(this.MIN_ZOOM, this.zoom * factor));
    this.panX = sx - wx * nz; this.panY = sy - wy * nz; this.zoom = nz;
  }

  private onTouchStart(e: TouchEvent) {
    if (e.touches.length === 2) {
      e.preventDefault(); this.isPinching = true;
      this.lastPinchDist = Math.hypot(e.touches[0].clientX - e.touches[1].clientX, e.touches[0].clientY - e.touches[1].clientY);
    } else if (e.touches.length === 1) {
      const r = this.el.getBoundingClientRect();
      this.startPan(e.touches[0].clientX - r.left, e.touches[0].clientY - r.top);
    }
  }

  private onTouchMove(e: TouchEvent) {
    e.preventDefault();
    if (e.touches.length === 2 && this.isPinching) {
      const dist = Math.hypot(e.touches[0].clientX - e.touches[1].clientX, e.touches[0].clientY - e.touches[1].clientY);
      const r    = this.el.getBoundingClientRect();
      const cx   = ((e.touches[0].clientX + e.touches[1].clientX) / 2) - r.left;
      const cy   = ((e.touches[0].clientY + e.touches[1].clientY) / 2) - r.top;
      const { x: wx, y: wy } = this.s2w(cx, cy);
      const f  = dist / this.lastPinchDist;
      const nz = Math.min(this.MAX_ZOOM, Math.max(this.MIN_ZOOM, this.zoom * f));
      this.panX = cx - wx * nz; this.panY = cy - wy * nz; this.zoom = nz;
      this.lastPinchDist = dist;
    } else if (e.touches.length === 1 && this.isPanning) {
      const r  = this.el.getBoundingClientRect();
      const sx = e.touches[0].clientX - r.left, sy = e.touches[0].clientY - r.top;
      this.panX = this.panOriginX + (sx - this.panStartX);
      this.panY = this.panOriginY + (sy - this.panStartY);
    }
  }

  private onTouchEnd() { this.isPinching = false; this.isPanning = false; }

  private onDbl(e: MouseEvent) {
    const { sx, sy } = this.evSX(e);
    const { x: wx, y: wy } = this.s2w(sx, sy);
    for (const n of this.nodes.values()) {
      if (n.containsPoint(wx, wy)) {
        // Always open config popover on double-click
        this.selectNode(n);
        this.openNodeConfig(n);
        return;
      }
    }
    // Double-click on empty canvas: close panel if open, else open palette
    if (document.body.classList.contains("panel-open")) {
      this.onPanelClose?.();
    } else {
      this.onPaletteRequest?.();
    }
  }

  // ── Cut ───────────────────────────────────────────────────────────────────

  private edgeCrossesPath(c: Connector): boolean {
    const fn = this.nodes.get(c.data.from_node), tn = this.nodes.get(c.data.to_node); if (!fn || !tn) return false;
    const fp = fn.ports.find(p => p.id === c.data.from_port), tp = tn.ports.find(p => p.id === c.data.to_port); if (!fp || !tp) return false;
    const cp = Math.max(Math.abs(tp.x - fp.x) * 0.55, 80);
    const pts: { x: number; y: number }[] = [];
    for (let t = 0; t <= 1; t += 0.04) pts.push({ x: bz(fp.x, fp.x + cp, tp.x - cp, tp.x, t), y: bz(fp.y, fp.y, tp.y, tp.y, t) });
    for (let i = 0; i < this.cutPath.length - 1; i++) {
      const ca = this.cutPath[i], cb = this.cutPath[i + 1];
      for (let j = 0; j < pts.length - 1; j++) if (segsX(pts[j], pts[j + 1], ca, cb)) return true;
    }
    return false;
  }

  private doCut(): Connector[] {
    const out: Connector[] = [];
    for (const [id, c] of this.connectors) if (this.edgeCrossesPath(c)) { out.push(c); this.connectors.delete(id); }
    for (const c of out) this.clearDynamicPortExpr(c);
    return out;
  }

  // ── Connector ─────────────────────────────────────────────────────────────

  /**
   * Clears the source_expr for the config slot corresponding to conn.to_port
   * on the target node, then re-derives and rebuilds that node's ports.
   * No-op if target node is absent or not a dynamic-port node.
   */
  private clearDynamicPortExpr(conn: Connector): void {
    const target = this.nodes.get(conn.data.to_node);
    if (!target?.data.dynamic_ports) return;
    const config = target.data.config as Record<string, unknown>;
    type Slot = { id: string; source_expr?: string };
    const slots = [
      ...((config.subfolders as Slot[] | undefined) ?? []),
      ...((config.sources    as Slot[] | undefined) ?? []),
    ];
    const slot = slots.find(s => s.id === conn.data.to_port);
    if (slot) slot.source_expr = "";
    target.derivePorts(target.data.config as Record<string, unknown>);
    target.rebuildPorts();
  }

  /**
   * Injects source_expr into the config slot of conn.to_port on the target
   * node, using the current display name of conn.from_node. Then re-derives
   * and rebuilds that node's ports.
   * No-op if target is absent, not a dynamic-port node, or source is absent.
   */
  private injectDynamicPortExpr(conn: Connector): void {
    const target = this.nodes.get(conn.data.to_node);
    if (!target?.data.dynamic_ports) return;
    const fromNode = this.nodes.get(conn.data.from_node);
    // If source node is not in the map (e.g. not yet restored during undo),
    // skip rather than writing a stale node-ID expression.
    if (!fromNode) return;
    const expr = `{{${fromNode.data.name}.output.files}}`;
    const config = target.data.config as Record<string, unknown>;
    type Slot = { id: string; source_expr?: string };
    const slots = [
      ...((config.subfolders as Slot[] | undefined) ?? []),
      ...((config.sources    as Slot[] | undefined) ?? []),
    ];
    const slot = slots.find(s => s.id === conn.data.to_port);
    if (slot) slot.source_expr = expr;
    target.derivePorts(target.data.config as Record<string, unknown>);
    target.rebuildPorts();
  }

  private finishConn(nodeId: string, portId: string) {
    if (!this.pendingConn) return;
    if (Array.from(this.connectors.values()).some(c => c.data.to_node === nodeId && c.data.to_port === portId)) return;
    const conn = new Connector({ id: newConnectorId(), from_node: this.pendingConn.fromNode, from_port: this.pendingConn.fromPort, to_node: nodeId, to_port: portId, condition: null, on_success: null, on_failure: null });
    this.connectors.set(conn.data.id, conn);
    this.pushUndo({ type: "add_edge", connector: conn });

    // For dynamic-port target nodes: inject source expression into the config
    // slot corresponding to this port, then re-derive ports.
    const target = this.nodes.get(nodeId);
    if (target?.data.dynamic_ports) {
      const fromNode = this.nodes.get(this.pendingConn.fromNode);
      const expr = `{{${fromNode?.data.name ?? this.pendingConn.fromNode}.output.files}}`;
      const config = target.data.config as Record<string, unknown>;
      type Slot = { id: string; source_expr?: string };
      const slots = [
        ...((config.subfolders as Slot[] | undefined) ?? []),
        ...((config.sources    as Slot[] | undefined) ?? []),
      ];
      const slot = slots.find(s => s.id === portId);
      if (slot) slot.source_expr = expr;
      target.derivePorts(target.data.config as Record<string, unknown>);
      target.rebuildPorts();
    }

    this.onCanvasChanged?.();
  }

  // ── Selection ─────────────────────────────────────────────────────────────

  selectNode(n: CanvasNode) { n.selected = true; this.selectedNodes.add(n.data.id); this.selectedNode = n; }
  // Call this only when the user explicitly wants to open the config (double-click)
  openNodeConfig(n: CanvasNode) { this.onNodeSelected?.(n); }
  selectConn(c: Connector)  { this.clearSelection(); c.selected = true; this.selectedConn = c; }

  clearSelection() {
    for (const id of this.selectedNodes) { const n = this.nodes.get(id); if (n) n.selected = false; }
    if (this.selectedConn) this.selectedConn.selected = false;
    this.selectedNodes.clear(); this.selectedNode = null; this.selectedConn = null;
    this.onNodeSelected?.(null);
  }

  deleteSelected() {
    const dn: CanvasNode[] = [], dc: Connector[] = [];
    for (const id of this.selectedNodes) {
      const n = this.nodes.get(id); if (!n) continue; dn.push(n); this.nodes.delete(id);
      for (const [cid, c] of this.connectors) if (c.data.from_node === id || c.data.to_node === id) { dc.push(c); this.connectors.delete(cid); }
    }
    if (this.selectedConn && !dc.includes(this.selectedConn)) { dc.push(this.selectedConn); this.connectors.delete(this.selectedConn.data.id); }
    // Clear source_expr for dynamic-port targets still alive after node deletions
    for (const c of dc) this.clearDynamicPortExpr(c);
    for (const n of dn) this.pushUndo({ type: "delete_node", node: n, connectors: dc });
    if (!dn.length && dc.length) for (const c of dc) this.pushUndo({ type: "delete_edge", connector: c });
    this.clearSelection(); if (dn.length || dc.length) this.onCanvasChanged?.();
  }

  // ── Undo ──────────────────────────────────────────────────────────────────

  private pushUndo(a: UndoAction) { this.undoStack.push(a); if (this.undoStack.length > 100) this.undoStack.shift(); this.redoStack = []; }

  undo() {
    const a = this.undoStack.pop(); if (!a) return;
    this.redoStack.push(a);
    switch (a.type) {
      case "add_node":
        this.nodes.delete(a.node.data.id);
        break;
      case "delete_node":
        // Restore node first so injectDynamicPortExpr can resolve it as a source
        this.nodes.set(a.node.data.id, a.node);
        for (const c of a.connectors) {
          this.connectors.set(c.data.id, c);
          this.injectDynamicPortExpr(c);
        }
        break;
      case "add_edge":
        this.connectors.delete(a.connector.data.id);
        this.clearDynamicPortExpr(a.connector);
        break;
      case "delete_edge":
        this.connectors.set(a.connector.data.id, a.connector);
        this.injectDynamicPortExpr(a.connector);
        break;
      case "move_node":
        { const n = this.nodes.get(a.nodeId); if (n) { n.data.position = { ...a.from }; n.updatePortPositions(); } }
        break;
      case "move_nodes":
        for (const m of a.moves) { const n = this.nodes.get(m.nodeId); if (n) { n.data.position = { ...m.from }; n.updatePortPositions(); } }
        break;
      case "cut_edges":
        for (const c of a.connectors) {
          this.connectors.set(c.data.id, c);
          this.injectDynamicPortExpr(c);
        }
        break;
      case "split_edge":
        // Reverse: remove added1, added2, middle node; restore original edge
        this.connectors.delete(a.added1.data.id);
        this.connectors.delete(a.added2.data.id);
        this.clearDynamicPortExpr(a.added2); // added2 lands on original target
        this.nodes.delete(a.node.data.id);
        this.connectors.set(a.removed.data.id, a.removed);
        this.injectDynamicPortExpr(a.removed);
        break;
    }
    this.onCanvasChanged?.();
  }

  redo() {
    const a = this.redoStack.pop(); if (!a) return;
    this.undoStack.push(a);
    switch (a.type) {
      case "add_node":
        this.nodes.set(a.node.data.id, a.node);
        break;
      case "delete_node":
        this.nodes.delete(a.node.data.id);
        for (const c of a.connectors) {
          this.clearDynamicPortExpr(c);
          this.connectors.delete(c.data.id);
        }
        break;
      case "add_edge":
        this.connectors.set(a.connector.data.id, a.connector);
        this.injectDynamicPortExpr(a.connector);
        break;
      case "delete_edge":
        this.connectors.delete(a.connector.data.id);
        this.clearDynamicPortExpr(a.connector);
        break;
      case "move_node":
        { const n = this.nodes.get(a.nodeId); if (n) { n.data.position = { ...a.to }; n.updatePortPositions(); } }
        break;
      case "move_nodes":
        for (const m of a.moves) { const n = this.nodes.get(m.nodeId); if (n) { n.data.position = { ...m.to }; n.updatePortPositions(); } }
        break;
      case "cut_edges":
        for (const c of a.connectors) {
          this.clearDynamicPortExpr(c);
          this.connectors.delete(c.data.id);
        }
        break;
      case "split_edge":
        // Re-apply: remove original edge, add middle node and two new edges
        this.clearDynamicPortExpr(a.removed);
        this.connectors.delete(a.removed.data.id);
        this.nodes.set(a.node.data.id, a.node);
        this.connectors.set(a.added1.data.id, a.added1);
        this.connectors.set(a.added2.data.id, a.added2);
        this.injectDynamicPortExpr(a.added2); // added2 lands on original target
        break;
    }
    this.onCanvasChanged?.();
  }

  dupSelected() {
    const ids = this.selectedNodes.size > 0 ? [...this.selectedNodes] : this.selectedNode ? [this.selectedNode.data.id] : [];
    if (!ids.length) return;
    const news: CanvasNode[] = [];
    for (const id of ids) {
      const orig = this.nodes.get(id); if (!orig) continue;
      const copy = new CanvasNode({ ...JSON.parse(JSON.stringify(orig.data)), id: `node_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`, position: { x: orig.data.position.x + 30, y: orig.data.position.y + 30 } });
      this.nodes.set(copy.data.id, copy); this.pushUndo({ type: "add_node", node: copy }); news.push(copy);
    }
    this.clearSelection(); for (const n of news) this.selectNode(n); this.onCanvasChanged?.();
  }

  toggleFocusMode() {
    this.focusMode = !this.focusMode;
    document.getElementById("sidebar")?.classList.toggle("focus-hidden", this.focusMode);
    document.getElementById("right-panel")?.classList.toggle("focus-hidden", this.focusMode);
    document.body.classList.toggle("focus-mode", this.focusMode);
    // ResizeObserver on the canvas element handles the resize automatically
    // when the CSS grid columns change width. No manual setTimeout needed.
  }

  // ── Public API ────────────────────────────────────────────────────────────

  placeNode(desc: NodeDescriptor, sx: number, sy: number): CanvasNode {
    const { x, y } = this.s2w(sx, sy);
    const r = this.el.getBoundingClientRect();
    const atCenter = Math.abs(sx - r.width / 2) < 30 && Math.abs(sy - r.height / 2) < 30;
    let px = x - NODE_WIDTH / 2, py = y - 20;

    // Auto-offset when placing multiple nodes at center
    if (atCenter && this.nodes.size > 0) {
      let maxX = -1e9, avgY = 0, cnt = 0;
      for (const n of this.nodes.values()) { if (n.data.position.x > maxX) maxX = n.data.position.x; avgY += n.data.position.y; cnt++; }
      px = maxX + NODE_WIDTH + 80; py = avgY / cnt - 20;
    }

    const node = createNodeFromDescriptor(desc, px, py);
    this.nodes.set(node.data.id, node);
    this.pushUndo({ type: "add_node", node });
    // Apply preset config if set by palette-manager
    if (this._pendingPresetConfig) {
      Object.assign(node.data.config, this._pendingPresetConfig);
      this._pendingPresetConfig = undefined;
    }
    if (this._pendingPresetName) {
      node.data.name = this._pendingPresetName;
      this._pendingPresetName = undefined;
    }
    this.tryWireInsert(node);
    this.onCanvasChanged?.();
    this.centerOn(node.data.position.x + NODE_WIDTH / 2, node.data.position.y + node.height / 2);
    return node;
  }

  centerOn(wx: number, wy: number) {
    const r = this.el.getBoundingClientRect();
    this.panX = r.width  / 2 - wx * this.zoom;
    this.panY = r.height / 2 - wy * this.zoom;
  }

  setNodeStatus(id: string, s: "idle" | "running" | "success" | "error") { const n = this.nodes.get(id); if (n) n.status = s; }
  setNodeOutput(id: string, p: string) { const n = this.nodes.get(id); if (n) { n.outputPreview = p; n.status = "success"; } }
  resetAllStatus() { for (const n of this.nodes.values()) { n.status = "idle"; n.outputPreview = null; n.showOutput = false; n.updatePortPositions(); } }
  clearConnectorActive() { for (const c of this.connectors.values()) c.active = false; }
  toWorkflowJson(id: string, name: string, parallelExecution = false, maxConcurrentNodes = 8) {
    return serialize(id, name, this.nodes, this.connectors, parallelExecution, maxConcurrentNodes);
  }

  fitToScreen() {
    if (!this.nodes.size) return;
    let mnX = 1e9, mnY = 1e9, mxX = -1e9, mxY = -1e9;
    for (const n of this.nodes.values()) { mnX = Math.min(mnX, n.data.position.x); mnY = Math.min(mnY, n.data.position.y); mxX = Math.max(mxX, n.data.position.x + NODE_WIDTH); mxY = Math.max(mxY, n.data.position.y + n.height); }
    const r   = this.el.getBoundingClientRect(), pad = 80;
    this.zoom = Math.min((r.width - pad * 2) / ((mxX - mnX) || 1), (r.height - pad * 2) / ((mxY - mnY) || 1), 1.2);
    this.panX = r.width / 2 - ((mnX + mxX) / 2) * this.zoom;
    this.panY = r.height / 2 - ((mnY + mxY) / 2) * this.zoom;
  }

  // Called by app.ts after the user picks a node from the wire-drop palette
  completeWireDrop(desc: NodeDescriptor): void {
    const drop = this._pendingWireDrop;
    if (!drop) return;
    this._pendingWireDrop = null;

    // Safety: desc must have at least one input port to connect to
    if (!desc.ports.inputs.length) return;

    // Place the node directly at the world-coordinate drop point.
    // Do NOT use placeNode() here — it calls centerOn() which pans the
    // viewport away from where the user just dropped the wire.
    const px = drop.wx - NODE_WIDTH / 2;
    const py = drop.wy - 20;
    const node = createNodeFromDescriptor(desc, px, py);

    // Apply any pending preset config (set by palette-manager on drag)
    if (this._pendingPresetConfig) { Object.assign(node.data.config, this._pendingPresetConfig); this._pendingPresetConfig = undefined; }
    if (this._pendingPresetName)   { node.data.name = this._pendingPresetName; this._pendingPresetName = undefined; }

    this.nodes.set(node.data.id, node);
    this.pushUndo({ type: "add_node", node });
    this.onCanvasChanged?.();

    // Auto-connect using the descriptor's first input port id (authoritative)
    const inputPortId = desc.ports.inputs[0].id;
    const conn = new Connector({
      id: newConnectorId(),
      from_node: drop.fromNode, from_port: drop.fromPort,
      to_node: node.data.id, to_port: inputPortId,
      condition: null, on_success: null, on_failure: null,
    });
    this.connectors.set(conn.data.id, conn);
    this.pushUndo({ type: "add_edge", connector: conn });
    this.onCanvasChanged?.();
  }
}

function bz(p0: number, p1: number, p2: number, p3: number, t: number): number {
  const m = 1 - t; return m * m * m * p0 + 3 * m * m * t * p1 + 3 * m * t * t * p2 + t * t * t * p3;
}
function segsX(a: { x: number; y: number }, b: { x: number; y: number }, c: { x: number; y: number }, d: { x: number; y: number }): boolean {
  const dn = (b.x - a.x) * (d.y - c.y) - (b.y - a.y) * (d.x - c.x); if (Math.abs(dn) < 1e-10) return false;
  const t = ((c.x - a.x) * (d.y - c.y) - (c.y - a.y) * (d.x - c.x)) / dn;
  const u = ((c.x - a.x) * (b.y - a.y) - (c.y - a.y) * (b.x - a.x)) / dn;
  return t >= 0 && t <= 1 && u >= 0 && u <= 1;
}
