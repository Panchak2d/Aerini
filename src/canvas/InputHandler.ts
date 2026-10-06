import type { Canvas } from "./Canvas";
import { CanvasNode, NODE_WIDTH } from "./Node";
import { PendingConnector, Connector } from "./Connector";
import type { MoveEntry, UndoAction } from "./UndoManager";
import { isMonitorModeActive } from "../monitor-mode";

/** Screen pixels a pressed port must travel before the press becomes a drag. */
const PORT_DRAG_THRESHOLD = 4;

type PortPress = {
  kind: "out" | "in" | "wire";
  node: string;
  port: string;
  /** The wire a drag picks up: a selected wire's handle, or the sole wire on a single-arity input. */
  conn?: Connector;
  sx: number;
  sy: number;
  /** Ctrl/Cmd held on an output: the drag moves every wire of that output. */
  moveAll: boolean;
};

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
  reconnEdge:  { conn: Connector } | null = null;
  portPress:   PortPress | null = null;
  moveGroup:   Connector[] | null = null;

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
    if (isMonitorModeActive()) return;
    const activeEl = document.activeElement;
    const inInput = activeEl instanceof HTMLInputElement || activeEl instanceof HTMLTextAreaElement;
    if (e.key === "Shift") { this.shiftHeld = true; this.canvas.el.classList.add("shift-held"); }
    if (inInput) return;

    const c = this.canvas;

    // ── Canvas-focused node navigation ──────────────────────────────────────
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
    const isFitToScreenChord = (e.ctrlKey || e.metaKey) && e.shiftKey;
    if ((e.key === "f" || e.key === "F") && !isFitToScreenChord) {
      if (e.ctrlKey || e.metaKey) e.preventDefault(); // stop the browser/webview's native Find
      c.toggleFocusMode();
    }

    const spaceKey = e.code === "Space" && !e.altKey && !e.ctrlKey && !e.metaKey;
    const focusOwnsSpace = spaceKey && activeEl instanceof HTMLElement && activeEl !== c.el &&
      (activeEl instanceof HTMLButtonElement || activeEl.getAttribute("role") === "button");
    if ((spaceKey && !focusOwnsSpace) || ((e.metaKey || e.ctrlKey) && e.key === "k")) {
      e.preventDefault(); c.onPaletteRequest?.();
    }
    if (e.key === "Escape") {
      this.portPress = null; this.moveGroup = null;
      this.reconnEdge = null;
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

    // Pressing a port only arms it; nothing happens until the pointer travels
    // PORT_DRAG_THRESHOLD, and a release before that is a click.
    const handle = c.wireHandleAt(wx, wy);
    if (handle) {
      this.portPress = { kind: "wire", node: handle.data.to_node, port: handle.data.to_port, conn: handle, sx, sy, moveAll: false };
      return;
    }

    for (const node of c.nodes.values()) {
      const p = node.portAtPoint(wx, wy);
      if (!p) continue;
      this.portPress = p.isInput
        ? { kind: "in",  node: node.data.id, port: p.id, conn: this.pickupWire(node.data.id, p.id), sx, sy, moveAll: false }
        : { kind: "out", node: node.data.id, port: p.id, sx, sy, moveAll: e.ctrlKey || e.metaKey };
      return;
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

  /** The wire a drag from this input would pick up: the sole wire on a single-arity input. */
  private pickupWire(nodeId: string, portId: string): Connector | undefined {
    const c = this.canvas;
    if (c.isMultiInput(nodeId, portId)) return undefined;
    const wires = c.wiresIntoPort(nodeId, portId);
    return wires.length === 1 ? wires[0] : undefined;
  }

  /**
   * Turns an armed port press into a drag once the pointer has travelled far
   * enough. Returns true when a drag is now in progress.
   */
  private promotePortPress(sx: number, sy: number, wx: number, wy: number): boolean {
    const press = this.portPress!;
    if (Math.hypot(sx - press.sx, sy - press.sy) < PORT_DRAG_THRESHOLD) return false;
    this.portPress = null;
    const c = this.canvas;
    const port = c.nodes.get(press.node)?.ports.find(p => p.id === press.port && p.isInput === (press.kind !== "out"));
    if (!port) return false;

    const wire = press.conn;
    if (wire) {
      const source = c.nodes.get(wire.data.from_node)?.ports.find(p => p.id === wire.data.from_port && !p.isInput);
      if (!source) return false;
      this.reconnEdge = { conn: wire };
      this.pendingConn = new PendingConnector(wire.data.from_node, wire.data.from_port, source.x, source.y);
    } else if (press.kind === "in") {
      this.pendingConn = new PendingConnector(press.node, press.port, port.x, port.y, "reverse");
    } else {
      const wires = press.moveAll ? c.wiresFromPort(press.node, press.port) : [];
      this.moveGroup = wires.length ? wires : null;
      this.pendingConn = new PendingConnector(press.node, press.port, port.x, port.y, this.moveGroup ? "move" : "forward");
    }
    this.pendingConn.toX = wx; this.pendingConn.toY = wy;
    c.el.style.cursor = "crosshair";
    return true;
  }

  onMove(e: MouseEvent): void { const { sx, sy } = this.canvas.evSX(e); this._move(e, sx, sy); }

  onWinMove(e: MouseEvent): void {
    if (!this.isPanning && !this.draggingNode && !this.pendingConn && !this.portPress && !this.isCutting && !this.isBoxSel) return;
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

    if (this.portPress && !this.promotePortPress(sx, sy, wx, wy)) return;

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
      const snap = c.snapTarget(this.pendingConn, wx, wy);
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

    if (c.wireHandleAt(wx, wy)) { c.el.style.cursor = "grab"; return; }

    let any = false;
    for (const n of c.nodes.values()) {
      n.hovered = n.containsPoint(wx, wy);
      if (n.hovered) {
        any = true;
        const port = n.portAtPoint(wx, wy);
        c.el.style.cursor = !port ? "grab" : port.isInput && this.pickupWire(n.data.id, port.id) ? "grab" : "crosshair";
      }
    }
    if (!any) c.el.style.cursor = this.shiftHeld ? "crosshair" : "default";
  }

  onUp(e: MouseEvent): void { const { sx, sy } = this.canvas.evSX(e); this._up(e, sx, sy); }

  onWinUp(e: MouseEvent): void {
    if (!this.isPanning && !this.draggingNode && !this.pendingConn && !this.portPress && !this.isCutting && !this.isBoxSel) return;
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

    if (this.portPress) {
      const press = this.portPress;
      this.portPress = null;
      if (press.kind === "in") {
        const wires = c.wiresIntoPort(press.node, press.port);
        if (wires.length === 1) c.selectConn(wires[0]);
      }
      return;
    }

    if (this.isPanning) { this.isPanning = false; c.el.style.cursor = "default"; c.onViewportChange?.(); return; }

    if (this.isCutting) {
      const cut = c.doCut();
      if (cut.length) { c.pushUndo({ type: "cut_edges", connectors: cut }); c.onCanvasChanged?.(); }
      this.isCutting = false; this.cutPath = []; c.el.style.cursor = "default"; return;
    }

    if (this.pendingConn) {
      const pending = this.pendingConn;
      const snap = c.snapTarget(pending, wx, wy);
      const { fromNode, fromPort } = pending;
      if (this.reconnEdge) {
        c.rerouteConn(this.reconnEdge.conn, snap && { nodeId: snap.nodeId, portId: snap.portId });
      } else if (pending.direction === "move") {
        if (snap && this.moveGroup) c.moveSources(this.moveGroup, { nodeId: snap.nodeId, portId: snap.portId });
      } else if (pending.direction === "reverse") {
        if (snap) {
          c.connectOrReplace(snap.nodeId, snap.portId, fromNode, fromPort);
        } else {
          c._pendingInputWireDrop = { toNode: fromNode, toPort: fromPort, wx, wy };
          c.onInputWireDropRequest?.(fromNode, fromPort, wx, wy);
        }
      } else if (snap) {
        c.finishConn(snap.nodeId, snap.portId);
      } else {
        c._pendingWireDrop = { fromNode, fromPort, wx, wy };
        c.onWireDropRequest?.(fromNode, fromPort, wx, wy);
      }
      this.reconnEdge = null; this.pendingConn = null; this.moveGroup = null; c.el.style.cursor = "default"; return;
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
    if (this.portPress || this.pendingConn) return;
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
