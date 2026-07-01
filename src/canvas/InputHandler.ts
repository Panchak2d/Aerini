import type { Canvas } from "./Canvas";
import { CanvasNode, PORT_RADIUS, NODE_WIDTH } from "./Node";
import { PendingConnector, Connector } from "./Connector";
import type { MoveEntry, UndoAction } from "./UndoManager";

export class InputHandler {
  private canvas: Canvas;

  isPanning  = false;
  panStartX  = 0; panStartY  = 0;
  panOriginX = 0; panOriginY = 0;

  draggingNode: CanvasNode | null = null;
  dragOffX = 0; dragOffY = 0;
  dragFromX = 0; dragFromY = 0;
  didDrag = false;

  pendingConn: PendingConnector | null = null;
  reconnEdge:  { conn: Connector; fromEnd?: boolean } | null = null;

  isCutting = false;
  cutPath: { x: number; y: number }[] = [];

  isBoxSel = false;
  bx0 = 0; by0 = 0; bx1 = 0; by1 = 0;

  shiftHeld = false;

  _multiDragFrom: Map<string, { x: number; y: number }> | null = null;

  lastPinchDist = 0;
  isPinching    = false;

  readonly _winMoveH = (e: MouseEvent)    => this.onWinMove(e);
  readonly _winUpH   = (e: MouseEvent)    => this.onWinUp(e);
  readonly _keyH     = (e: KeyboardEvent) => this.onKey(e);
  readonly _keyUpH   = (e: KeyboardEvent) => {
    if (e.key === "Shift") { this.shiftHeld = false; this.canvas.el.classList.remove("shift-held"); }
  };

  constructor(canvas: Canvas) {
    this.canvas = canvas;
  }

  onKey(e: KeyboardEvent): void {
    const inInput = document.activeElement instanceof HTMLInputElement || document.activeElement instanceof HTMLTextAreaElement;
    if (e.key === "Shift") { this.shiftHeld = true; this.canvas.el.classList.add("shift-held"); }
    if (inInput) return;

    const c = this.canvas;

    // ── Canvas-focused node navigation (H1) ────────────────────────────────
    // Only active when the canvas element itself has focus (role="application")
    const canvasFocused = document.activeElement === c.el;
    if (canvasFocused) {
      if (e.key === "ArrowRight" || e.key === "ArrowLeft" || e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        this._navigateNodes(e.key);
        return;
      }
      if (e.key === "Enter" && c.selectedNode) {
        e.preventDefault();
        c.openNodeConfig(c.selectedNode);
        return;
      }
    }
    // ── End canvas navigation ───────────────────────────────────────────────

    if ((e.metaKey || e.ctrlKey) && e.key === "z" && !e.shiftKey) { e.preventDefault(); c.undo(); }
    if ((e.metaKey || e.ctrlKey) && (e.key === "y" || (e.key === "z" && e.shiftKey))) { e.preventDefault(); c.redo(); }
    if ((e.metaKey || e.ctrlKey) && e.key === "d") { e.preventDefault(); c.dupSelected(); }
    if ((e.metaKey || e.ctrlKey) && e.key === "a") {
      e.preventDefault();
      for (const n of c.nodes.values()) { n.selected = true; c.selectedNodes.add(n.data.id); }
    }
    if (e.key === "Delete" || e.key === "Backspace") c.deleteSelected();
    if (e.key === "f" || e.key === "F") c.toggleFocusMode();
    if ((e.code === "Space" && !e.altKey && !e.ctrlKey && !e.metaKey) || ((e.metaKey || e.ctrlKey) && e.key === "k")) {
      e.preventDefault(); c.onPaletteRequest?.();
    }
    if (e.key === "Escape") {
      this.pendingConn = null; this.isCutting = false; this.cutPath = [];
      c.pendingInsert = null; c.insertGhost = null;
      c._pendingInputWireDrop = null;
      c.clearSelection(); c.el.style.cursor = "default";
    }
  }

  /** Navigate to the nearest node in the given arrow direction. */
  private _navigateNodes(key: string): void {
    const c = this.canvas;
    const nodes = Array.from(c.nodes.values());
    if (!nodes.length) return;

    if (!c.selectedNode) {
      // No current selection — pick first node in insertion order
      const first = nodes[0];
      c.clearSelection();
      c.selectNode(first);
      c.centerOn(first.data.position.x + NODE_WIDTH / 2, first.data.position.y + first.height / 2);
      return;
    }

    const cur = c.selectedNode;
    const cx = cur.data.position.x + NODE_WIDTH / 2;
    const cy = cur.data.position.y + cur.height / 2;

    let best: CanvasNode | null = null;
    let bestDist = Infinity;

    for (const n of nodes) {
      if (n === cur) continue;
      const nx = n.data.position.x + NODE_WIDTH / 2;
      const ny = n.data.position.y + n.height / 2;
      const dx = nx - cx;
      const dy = ny - cy;

      // Each arrow direction requires the candidate to be meaningfully in that direction.
      // A small 10px threshold avoids selecting nodes that are almost exactly aligned.
      const inDir =
        key === "ArrowRight" ? dx > 10 :
        key === "ArrowLeft"  ? dx < -10 :
        key === "ArrowDown"  ? dy > 10 :
        /* ArrowUp */           dy < -10;

      if (!inDir) continue;

      const dist = Math.hypot(dx, dy);
      if (dist < bestDist) { bestDist = dist; best = n; }
    }

    if (best) {
      c.clearSelection();
      c.selectNode(best);
      c.centerOn(best.data.position.x + NODE_WIDTH / 2, best.data.position.y + best.height / 2);
    }
  }

  onDown(e: MouseEvent): void {
    e.preventDefault();
    const c = this.canvas;
    const { sx, sy } = c.evSX(e);
    const { x: wx, y: wy } = c.s2w(sx, sy);

    if (e.button === 0 && e.altKey) { this.isCutting = true; this.cutPath = [{ x: wx, y: wy }]; c.el.style.cursor = "crosshair"; return; }
    if (e.button === 1) { this.startPan(sx, sy); return; }
    if (e.button !== 0) return;

    // Grab existing connector — input end or output end — to reroute or disconnect
    for (const conn of c.connectors.values()) {
      const tn = c.nodes.get(conn.data.to_node); if (!tn) continue;
      const tp = tn.ports.find(p => p.id === conn.data.to_port); if (!tp) continue;
      const fn = c.nodes.get(conn.data.from_node); if (!fn) continue;
      const fp = fn.ports.find(p => p.id === conn.data.from_port); if (!fp) continue;

      if (Math.hypot(wx - tp.x, wy - tp.y) < PORT_RADIUS + 8) {
        this.reconnEdge = { conn }; c.connectors.delete(conn.data.id);
        c.clearDynamicPortExpr(conn);
        this.pendingConn = new PendingConnector(conn.data.from_node, conn.data.from_port, fp.x, fp.y);
        this.pendingConn.toX = wx; this.pendingConn.toY = wy;
        c.el.style.cursor = "crosshair"; return;
      }
      if (Math.hypot(wx - fp.x, wy - fp.y) < PORT_RADIUS + 8) {
        c.connectors.delete(conn.data.id);
        c.clearDynamicPortExpr(conn);
        this.reconnEdge = { conn, fromEnd: true };
        this.pendingConn = new PendingConnector(conn.data.from_node, conn.data.from_port, fp.x, fp.y);
        this.pendingConn.toX = wx; this.pendingConn.toY = wy;
        c.el.style.cursor = "crosshair"; return;
      }
    }

    // Output port → new connector
    for (const node of c.nodes.values()) {
      const p = node.portAtPoint(wx, wy);
      if (p && !p.isInput) { this.pendingConn = new PendingConnector(node.data.id, p.id, p.x, p.y); c.el.style.cursor = "crosshair"; return; }
      if (p && p.isInput) {
        c._pendingInputWireDrop = { toNode: node.data.id, toPort: p.id, wx: p.x, wy: p.y };
        c.onInputWireDropRequest?.(node.data.id, p.id, p.x, p.y);
        return;
      }
    }

    // Node drag
    const arr = Array.from(c.nodes.values());
    for (let i = arr.length - 1; i >= 0; i--) {
      const n = arr[i];
      if (!n.containsPoint(wx, wy)) continue;
      if (!e.shiftKey && !c.selectedNodes.has(n.data.id)) c.clearSelection();
      c.selectNode(n);
      c.onNodeClicked?.(n);
      this.draggingNode = n;
      this.dragOffX = wx - n.data.position.x; this.dragOffY = wy - n.data.position.y;
      this.dragFromX = n.data.position.x; this.dragFromY = n.data.position.y;
      if (c.selectedNodes.size > 1) {
        this._multiDragFrom = new Map();
        for (const id of c.selectedNodes) {
          const mn = c.nodes.get(id);
          if (mn) this._multiDragFrom.set(id, { ...mn.data.position });
        }
      } else {
        this._multiDragFrom = null;
      }
      this.didDrag = false; c.el.style.cursor = "grab"; return;
    }

    // Wire click
    for (const conn of c.connectors.values()) {
      if (conn.containsPoint(wx, wy, c.nodes, 8 / c.zoom)) { c.selectConn(conn); return; }
    }

    // Empty space: Shift = box-select, plain = pan + deselect
    if (e.shiftKey) {
      this.isBoxSel = true; this.bx0 = wx; this.by0 = wy; this.bx1 = wx; this.by1 = wy;
      c.el.style.cursor = "crosshair";
    } else {
      c.clearSelection();
      this.startPan(sx, sy);
    }
  }

  startPan(sx: number, sy: number): void {
    this.isPanning = true; this.panStartX = sx; this.panStartY = sy;
    this.panOriginX = this.canvas.panX; this.panOriginY = this.canvas.panY;
    this.canvas.el.style.cursor = "grabbing";
  }

  onMove(e: MouseEvent): void { const { sx, sy } = this.canvas.evSX(e); this._move(e, sx, sy); }

  onWinMove(e: MouseEvent): void {
    if (!this.isPanning && !this.draggingNode && !this.pendingConn && !this.isCutting && !this.isBoxSel) return;
    const r = this.canvas.el.getBoundingClientRect();
    this._move(e, e.clientX - r.left, e.clientY - r.top);
  }

  _move(_e: MouseEvent, sx: number, sy: number): void {
    const c = this.canvas;
    const { x: wx, y: wy } = c.s2w(sx, sy);
    if (c.pendingInsert) c.insertGhost = { x: wx, y: wy };

    if (this.isPanning) {
      c.panX = this.panOriginX + (sx - this.panStartX);
      c.panY = this.panOriginY + (sy - this.panStartY);
      c.el.style.cursor = "grabbing"; return;
    }
    if (this.isCutting) { this.cutPath.push({ x: wx, y: wy }); return; }

    if (this.draggingNode) {
      this.didDrag = true;
      let nx = wx - this.dragOffX, ny = wy - this.dragOffY;
      if (localStorage.getItem("aerini_grid_snap") === "true") {
        const G = 20;
        nx = Math.round(nx / G) * G;
        ny = Math.round(ny / G) * G;
      }
      if (c.selectedNodes.size > 1 && c.selectedNodes.has(this.draggingNode.data.id)) {
        const dx = nx - this.draggingNode.data.position.x, dy = ny - this.draggingNode.data.position.y;
        for (const id of c.selectedNodes) { const n = c.nodes.get(id); if (n) { n.data.position.x += dx; n.data.position.y += dy; n.updatePortPositions(); } }
      } else {
        this.draggingNode.data.position.x = nx; this.draggingNode.data.position.y = ny;
        this.draggingNode.updatePortPositions(); c.snap.computeSnap(this.draggingNode);
      }
      c.onCanvasChanged?.(); c.el.style.cursor = "grabbing"; return;
    }

    if (this.pendingConn) {
      const snap = c.nearestIn(wx, wy, this.pendingConn.fromNode);
      this.pendingConn.toX = snap ? snap.x : wx; this.pendingConn.toY = snap ? snap.y : wy; return;
    }

    if (this.isBoxSel) {
      this.bx1 = wx; this.by1 = wy;
      const x1 = Math.min(this.bx0, wx), y1 = Math.min(this.by0, wy), x2 = Math.max(this.bx0, wx), y2 = Math.max(this.by0, wy);
      for (const n of c.nodes.values()) {
        const ins = n.data.position.x >= x1 && n.data.position.x <= x2 && n.data.position.y >= y1 && n.data.position.y <= y2;
        n.selected = ins; if (ins) c.selectedNodes.add(n.data.id); else c.selectedNodes.delete(n.data.id);
      }
      return;
    }

    let any = false;
    for (const n of c.nodes.values()) {
      n.hovered = n.containsPoint(wx, wy);
      if (n.hovered) { any = true; c.el.style.cursor = n.portAtPoint(wx, wy) ? "crosshair" : "grab"; }
    }
    if (!any) c.el.style.cursor = this.shiftHeld ? "crosshair" : "default";
  }

  onUp(e: MouseEvent): void { const { sx, sy } = this.canvas.evSX(e); this._up(e, sx, sy); }

  onWinUp(e: MouseEvent): void {
    if (!this.isPanning && !this.draggingNode && !this.pendingConn && !this.isCutting && !this.isBoxSel) return;
    const r = this.canvas.el.getBoundingClientRect();
    this._up(e, e.clientX - r.left, e.clientY - r.top);
  }

  _up(_e: MouseEvent, sx: number, sy: number): void {
    const c = this.canvas;
    const { x: wx, y: wy } = c.s2w(sx, sy);

    if (c.pendingInsert && sx >= 0 && sy >= 0) {
      c.placeNode(c.pendingInsert, sx, sy);
      c.pendingInsert = null; c.insertGhost = null; c.el.style.cursor = "default"; return;
    }
    c.pendingInsert = null; c.insertGhost = null;

    if (this.isPanning) { this.isPanning = false; c.el.style.cursor = "default"; c.onViewportChange?.(); return; }

    if (this.isCutting) {
      const cut = c.doCut();
      if (cut.length) { c.pushUndo({ type: "cut_edges", connectors: cut }); c.onCanvasChanged?.(); }
      this.isCutting = false; this.cutPath = []; c.el.style.cursor = "default"; return;
    }

    if (this.pendingConn) {
      const snap = c.nearestIn(wx, wy, this.pendingConn.fromNode);
      if (snap) {
        c.finishConn(snap.nodeId, snap.portId);
      } else if (this.reconnEdge) {
        c.pushUndo({ type: "delete_edge", connector: this.reconnEdge.conn });
        c.onCanvasChanged?.();
      } else {
        const { fromNode, fromPort } = this.pendingConn;
        c._pendingWireDrop = { fromNode, fromPort, wx, wy };
        c.onWireDropRequest?.(fromNode, fromPort, wx, wy);
      }
      this.reconnEdge = null; this.pendingConn = null; c.el.style.cursor = "default"; return;
    }

    if (this.draggingNode) {
      c.snap.snapGuides = [];
      if (this.didDrag) {
        if (c.selectedNodes.size > 1 && this._multiDragFrom) {
          const moves: MoveEntry[] = [];
          for (const [id, from] of this._multiDragFrom) {
            const mn = c.nodes.get(id);
            if (mn && (mn.data.position.x !== from.x || mn.data.position.y !== from.y))
              moves.push({ nodeId: id, from, to: { ...mn.data.position } });
          }
          if (moves.length) c.pushUndo({ type: "move_nodes", moves } as UndoAction);
        } else {
          const p = this.draggingNode.data.position;
          if (p.x !== this.dragFromX || p.y !== this.dragFromY) {
            c.pushUndo({ type: "move_node", nodeId: this.draggingNode.data.id, from: { x: this.dragFromX, y: this.dragFromY }, to: { ...p } });
            c.tryWireInsert(this.draggingNode);
          }
        }
      }
      this._multiDragFrom = null;
      this.draggingNode = null; c.el.style.cursor = "default"; return;
    }

    if (this.isBoxSel) { this.isBoxSel = false; c.el.style.cursor = this.shiftHeld ? "crosshair" : "default"; }
  }

  onLeave(): void {
    if (!this.isPanning && !this.draggingNode && !this.isCutting)
      for (const n of this.canvas.nodes.values()) n.hovered = false;
  }

  onRightClick(e: MouseEvent): void {
    const c = this.canvas;
    const { sx, sy } = c.evSX(e);
    const { x: wx, y: wy } = c.s2w(sx, sy);

    const arr = Array.from(c.nodes.values());
    for (let i = arr.length - 1; i >= 0; i--) {
      const n = arr[i];
      if (n.containsPoint(wx, wy)) {
        if (!c.selectedNodes.has(n.data.id)) {
          c.clearSelection();
          c.selectNode(n);
        }
        c.ctxMenu.show(e.clientX, e.clientY, n);
        return;
      }
    }

    for (const [id, conn] of c.connectors) {
      if (conn.containsPoint(wx, wy, c.nodes, 8 / c.zoom)) {
        c.connectors.delete(id);
        c.clearDynamicPortExpr(conn);
        c.pushUndo({ type: "delete_edge", connector: conn });
        c.onCanvasChanged?.(); return;
      }
    }

    c.clearSelection();
  }

  onDbl(e: MouseEvent): void {
    const c = this.canvas;
    const { sx, sy } = c.evSX(e);
    const { x: wx, y: wy } = c.s2w(sx, sy);
    for (const n of c.nodes.values()) {
      if (n.containsPoint(wx, wy)) {
        c.selectNode(n);
        c.openNodeConfig(n);
        return;
      }
    }
    if (document.body.classList.contains("panel-open")) {
      c.onPanelClose?.();
    } else {
      c.onPaletteRequest?.();
    }
  }

  onWheel(e: WheelEvent): void {
    e.preventDefault();
    const c = this.canvas;
    const r = c.el.getBoundingClientRect();
    const sx = e.clientX - r.left, sy = e.clientY - r.top;
    const { x: wx, y: wy } = c.s2w(sx, sy);
    const isPinch = e.ctrlKey;
    const factor  = e.deltaY > 0 ? (isPinch ? 0.95 : 0.90) : (isPinch ? 1 / 0.95 : 1 / 0.90);
    const nz = Math.min(c.MAX_ZOOM, Math.max(c.MIN_ZOOM, c.zoom * factor));
    c.panX = sx - wx * nz; c.panY = sy - wy * nz; c.zoom = nz;
    c.onZoomChange?.(c.zoom);
    c.onViewportChange?.();
  }

  onTouchStart(e: TouchEvent): void {
    const c = this.canvas;
    if (e.touches.length === 2) {
      e.preventDefault(); this.isPinching = true;
      this.lastPinchDist = Math.hypot(e.touches[0].clientX - e.touches[1].clientX, e.touches[0].clientY - e.touches[1].clientY);
    } else if (e.touches.length === 1) {
      const r = c.el.getBoundingClientRect();
      this.startPan(e.touches[0].clientX - r.left, e.touches[0].clientY - r.top);
    }
  }

  onTouchMove(e: TouchEvent): void {
    e.preventDefault();
    const c = this.canvas;
    if (e.touches.length === 2 && this.isPinching) {
      const dist = Math.hypot(e.touches[0].clientX - e.touches[1].clientX, e.touches[0].clientY - e.touches[1].clientY);
      const r    = c.el.getBoundingClientRect();
      const cx   = ((e.touches[0].clientX + e.touches[1].clientX) / 2) - r.left;
      const cy   = ((e.touches[0].clientY + e.touches[1].clientY) / 2) - r.top;
      const { x: wx, y: wy } = c.s2w(cx, cy);
      const f  = dist / this.lastPinchDist;
      const nz = Math.min(c.MAX_ZOOM, Math.max(c.MIN_ZOOM, c.zoom * f));
      c.panX = cx - wx * nz; c.panY = cy - wy * nz; c.zoom = nz;
      this.lastPinchDist = dist;
      c.onZoomChange?.(c.zoom);
      c.onViewportChange?.();
    } else if (e.touches.length === 1 && this.isPanning) {
      const r  = c.el.getBoundingClientRect();
      const sx = e.touches[0].clientX - r.left, sy = e.touches[0].clientY - r.top;
      c.panX = this.panOriginX + (sx - this.panStartX);
      c.panY = this.panOriginY + (sy - this.panStartY);
    }
  }

  onTouchEnd(): void { this.isPinching = false; this.isPanning = false; }
}
