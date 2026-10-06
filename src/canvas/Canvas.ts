import { CanvasNode, createNodeFromDescriptor, NODE_WIDTH, PORT_RADIUS, roundedRect } from "./Node";
import { Connector, newConnectorId, type PendingConnector } from "./Connector";
import { serialize } from "./CanvasSerializer";
import { NODE_IDS } from "../node-ids";
import type { NodeDescriptor } from "../ipc/workflow";
import { UndoManager } from "./UndoManager";
import type { UndoAction, ConfigPatch } from "./UndoManager";
import { Minimap } from "./Minimap";
import { SnapEngine } from "./SnapEngine";
import { ContextMenu } from "./ContextMenu";
import { InputHandler } from "./InputHandler";
import { getCanvasColors } from "./theme-colors";
import { getNodeDescriptor } from "./node-registry";

/** #output-drawer is a `position: fixed` overlay (workspace.css) — it sits on
 * top of #canvas without shrinking #canvas's own box, so a canvas element's
 * getBoundingClientRect() alone still reports full height even while the
 * drawer visually covers the bottom of it. Returns how much of `r`, measured
 * from its own top edge, is actually unobstructed by the drawer. */
function unobstructedHeight(r: DOMRect): number {
  const drawer = document.getElementById("output-drawer");
  if (!drawer || drawer.classList.contains("hidden")) return r.height;
  const dr = drawer.getBoundingClientRect();
  if (dr.right <= r.left || dr.left >= r.right) return r.height;
  return Math.max(0, Math.min(r.bottom, dr.top) - r.top);
}

export class Canvas {
  // Internal canvas element and rendering context (accessed by sub-modules)
  el:  HTMLCanvasElement;
  ctx: CanvasRenderingContext2D;
  private dpr = 1;

  nodes:      Map<string, CanvasNode> = new Map();
  connectors: Map<string, Connector>  = new Map();

  panX = 0; panY = 0; zoom = 1;
  readonly MIN_ZOOM = 0.05;
  readonly MAX_ZOOM = 4;

  pendingInsert: NodeDescriptor | null = null;
  insertGhost:   { x: number; y: number } | null = null;

  private focusMode = false;

  _pendingWireDrop: { fromNode: string; fromPort: string; wx: number; wy: number } | null = null;
  _pendingInputWireDrop: { toNode: string; toPort: string; wx: number; wy: number } | null = null;
  _pendingPresetConfig: Record<string, unknown> | undefined = undefined;
  _pendingPresetName:   string | undefined = undefined;

  selectedNodes: Set<string> = new Set();
  selectedNode:  CanvasNode | null = null;
  selectedConn:  Connector  | null = null;

  private emptyEl: HTMLElement | null = null;
  private wasEmpty: boolean | null = null;

  onPaletteRequest: (() => void) | null = null;
  onWireDropRequest: ((fromNode: string, fromPort: string, wx: number, wy: number) => void) | null = null;
  onInputWireDropRequest: ((toNode: string, toPort: string, wx: number, wy: number) => void) | null = null;
  onNodeSelected:   ((n: CanvasNode | null) => void) | null = null;
  onNodeClicked:    ((n: CanvasNode) => void) | null = null;
  onCanvasChanged:  (() => void) | null = null;
  onRunNode:        ((nodeId: string) => void) | null = null;
  onZoomChange:     ((zoom: number) => void) | null = null;
  onViewportChange: (() => void) | null = null;
  /** Called when a connection is made between potentially incompatible ports. Non-blocking. */
  onWarn:           ((msg: string) => void) | null = null;

  private lastTs = 0;

  // Sub-modules — not private so peers can cross-reference via canvas ref
  input:   InputHandler;
  undoMgr: UndoManager;
  minimap: Minimap;
  snap:    SnapEngine;
  ctxMenu: ContextMenu;

  constructor(canvasEl: HTMLCanvasElement) {
    this.el  = canvasEl;
    this.ctx = canvasEl.getContext("2d")!;

    const mmEl = document.getElementById("minimap-canvas") as HTMLCanvasElement | null;

    this.undoMgr = new UndoManager(this);
    this.minimap = new Minimap(this, mmEl);
    this.snap    = new SnapEngine(this);
    this.ctxMenu = new ContextMenu(this);
    this.input   = new InputHandler(this);

    this.emptyEl = document.getElementById("canvas-empty-state");
    this.resize();
    this.bind();
    requestAnimationFrame((t) => this.loop(t));
  }

  // ── Resize ────────────────────────────────────────────────────────────────

  resize() {
    this.dpr = window.devicePixelRatio || 1;
    const r  = this.el.getBoundingClientRect();
    this.el.width  = Math.round(r.width  * this.dpr);
    this.el.height = Math.round(r.height * this.dpr);
    if (this.panX === 0 && this.panY === 0) {
      this.panX = r.width  / 2;
      this.panY = r.height / 2;
    }
  }

  s2w(sx: number, sy: number) {
    return { x: (sx - this.panX) / this.zoom, y: (sy - this.panY) / this.zoom };
  }

  evSX(e: MouseEvent) {
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
    const ih  = this.input;

    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    ctx.setTransform(dpr * this.zoom, 0, 0, dpr * this.zoom, this.panX * dpr, this.panY * dpr);

    // Draw connectors (wires being moved stay visible but dimmed)
    const moving = ih.reconnEdge?.conn;
    const movingGroup = ih.moveGroup;
    for (const c of this.connectors.values()) {
      c.highlighted = ih.isCutting && this.edgeCrossesPath(c);
      if (c === moving || movingGroup?.includes(c)) ctx.globalAlpha = 0.25;
      c.draw(ctx, this.nodes, dt);
      ctx.globalAlpha = 1;
    }
    if (ih.pendingConn) ih.pendingConn.draw(ctx);

    const colors = getCanvasColors();

    // Port snap ring
    if (ih.pendingConn) {
      const snap = this.snapTarget(ih.pendingConn, ih.pendingConn.toX, ih.pendingConn.toY);
      if (snap) {
        ctx.save();
        ctx.beginPath();
        ctx.arc(snap.x, snap.y, PORT_RADIUS + 7, 0, Math.PI * 2);
        ctx.strokeStyle = colors.actionNav + "cc";
        ctx.lineWidth   = 2 / this.zoom;
        ctx.stroke();
        ctx.beginPath();
        ctx.arc(snap.x, snap.y, PORT_RADIUS + 3, 0, Math.PI * 2);
        ctx.fillStyle = colors.actionNav + "22";
        ctx.fill();
        ctx.restore();
      }

      // Source port glow
      const srcNode = this.nodes.get(ih.pendingConn.fromNode);
      const srcIsInput = ih.pendingConn.direction === "reverse";
      const srcPort = srcNode?.ports.find(p => p.id === ih.pendingConn!.fromPort && p.isInput === srcIsInput);
      if (srcPort) {
        ctx.save();
        ctx.beginPath();
        ctx.arc(srcPort.x, srcPort.y, PORT_RADIUS + 6, 0, Math.PI * 2);
        ctx.strokeStyle = colors.actionNav + "88";
        ctx.lineWidth   = 2 / this.zoom;
        ctx.stroke();
        ctx.restore();
      }
    }

    // Drag handle on the selected wire's target end
    if (this.selectedConn && !moving) {
      const tp = this.nodes.get(this.selectedConn.data.to_node)?.ports
        .find(p => p.id === this.selectedConn!.data.to_port && p.isInput);
      if (tp) {
        ctx.save();
        ctx.beginPath();
        ctx.arc(tp.x, tp.y, PORT_RADIUS + 5, 0, Math.PI * 2);
        ctx.strokeStyle = colors.actionNav;
        ctx.lineWidth   = 2 / this.zoom;
        ctx.stroke();
        ctx.restore();
      }
    }

    // One pass over the connectors builds the connected-port lookup (port
    // fill state) and the per-port wire counts (output badges, overfed inputs).
    const connectedPorts = new Set<string>();
    const outCount = new Map<string, number>();
    const inCount = new Map<string, number>();
    for (const c of this.connectors.values()) {
      const fromKey = `${c.data.from_node}:${c.data.from_port}`;
      const toKey   = `${c.data.to_node}:${c.data.to_port}`;
      connectedPorts.add(fromKey);
      connectedPorts.add(toKey);
      outCount.set(fromKey, (outCount.get(fromKey) ?? 0) + 1);
      inCount.set(toKey, (inCount.get(toKey) ?? 0) + 1);
    }

    // Draw nodes
    const overfedPorts = this.overfedFromCounts(inCount);
    for (const n of this.nodes.values()) n.draw(ctx, dt, connectedPorts, overfedPorts);

    // ── Output port connection-count badges ───────────────────────────
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
      ctx.fillStyle   = colors.actionNav;
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

    // Hover tooltip for nodes with truncated names
    this.drawNodeTooltips();

    // Draw ghost
    if (this.pendingInsert && this.insertGhost) {
      ctx.globalAlpha = 0.35;
      const d = createNodeFromDescriptor(this.pendingInsert, this.insertGhost.x - NODE_WIDTH / 2, this.insertGhost.y - 20);
      d.draw(ctx, 0);
      ctx.globalAlpha = 1;
    }

    // Box select rect
    if (ih.isBoxSel) {
      ctx.save();
      const bx = Math.min(ih.bx0, ih.bx1), by = Math.min(ih.by0, ih.by1);
      const bw = Math.abs(ih.bx1 - ih.bx0), bh = Math.abs(ih.by1 - ih.by0);
      ctx.fillStyle   = colors.actionNav + "0f";
      ctx.strokeStyle = colors.actionNav;
      ctx.lineWidth   = 1.5 / this.zoom;
      ctx.fillRect(bx, by, bw, bh);
      ctx.strokeRect(bx, by, bw, bh);
      ctx.restore();
    }

    // Cut path
    if (ih.isCutting && ih.cutPath.length > 1) {
      ctx.save();
      ctx.beginPath();
      ctx.moveTo(ih.cutPath[0].x, ih.cutPath[0].y);
      for (let i = 1; i < ih.cutPath.length; i++) ctx.lineTo(ih.cutPath[i].x, ih.cutPath[i].y);
      ctx.strokeStyle = colors.error;
      ctx.lineWidth   = 2 / this.zoom;
      ctx.setLineDash([5 / this.zoom, 3 / this.zoom]);
      ctx.stroke();
      ctx.restore();
    }

    // Snap guides
    if (this.snap.snapGuides.length && ih.draggingNode) {
      ctx.save();
      ctx.strokeStyle = colors.actionNav + "66";
      ctx.lineWidth   = 1 / this.zoom;
      for (const g of this.snap.snapGuides) {
        ctx.beginPath();
        ctx.moveTo(g.x1, g.y1);
        ctx.lineTo(g.x2, g.y2);
        ctx.stroke();
      }
      ctx.restore();
    }

    // Empty state
    if (this.emptyEl) {
      const isEmpty = this.nodes.size === 0;
      this.emptyEl.style.display = isEmpty ? "flex" : "none";
      if (this.wasEmpty !== null && !this.wasEmpty && isEmpty) {
        const ann = document.getElementById("a11y-announcer");
        if (ann) ann.textContent = "Canvas is empty. Add nodes from the sidebar to build your workflow.";
      }
      this.wasEmpty = isEmpty;
    }

    this.minimap.draw(W, H);
  }

  // Screen-space tooltip for a hovered node whose name is truncated.
  private drawNodeTooltips(): void {
    const ctx = this.ctx;
    const dpr = this.dpr;
    const W   = this.el.getBoundingClientRect().width;

    for (const n of this.nodes.values()) {
      if (!n.hovered) continue;
      if (n.data.node_type_id === NODE_IDS.NOTE) continue;
      if (n.data.name.length <= 20) continue;

      const sx = (n.data.position.x + NODE_WIDTH / 2) * this.zoom + this.panX;
      const sy = n.data.position.y * this.zoom + this.panY;

      ctx.save();
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

      ctx.font = "11px -apple-system, BlinkMacSystemFont, sans-serif";
      const padX = 8, padY = 4, gap = 8;
      const maxBoxW = Math.max(60, W - 16);
      let label = n.data.name;
      let boxW  = ctx.measureText(label).width + padX * 2;
      if (boxW > maxBoxW) {
        const maxTextW = maxBoxW - padX * 2;
        while (label.length > 1 && ctx.measureText(label + "…").width > maxTextW) {
          label = label.slice(0, -1);
        }
        label += "…";
        boxW = ctx.measureText(label).width + padX * 2;
      }
      const boxH = 11 + padY * 2;

      const bx = Math.max(4, Math.min(sx - boxW / 2, W - boxW - 4));
      const by = Math.max(4, sy - gap - boxH);

      const colors = getCanvasColors();
      ctx.beginPath();
      roundedRect(ctx, bx, by, boxW, boxH, 4);
      ctx.fillStyle = colors.surface3;
      ctx.fill();
      ctx.strokeStyle = colors.border;
      ctx.lineWidth = 1;
      ctx.stroke();

      ctx.fillStyle = colors.textPrimary;
      ctx.textAlign = "left";
      ctx.textBaseline = "middle";
      ctx.fillText(label, bx + padX, by + boxH / 2);

      ctx.restore();
    }
  }

  // ── Events ────────────────────────────────────────────────────────────────

  private bind() {
    const ro = new ResizeObserver(() => this.resize());
    ro.observe(this.el);
    const ih = this.input;
    this.el.addEventListener("mousedown",   (e) => ih.onDown(e));
    this.el.addEventListener("mousemove",   (e) => ih.onMove(e));
    this.el.addEventListener("mouseup",     (e) => ih.onUp(e));
    this.el.addEventListener("mouseleave",  ()  => ih.onLeave());
    window.addEventListener("mousemove",    ih._winMoveH);
    window.addEventListener("mouseup",      ih._winUpH);
    this.el.addEventListener("wheel",       (e) => ih.onWheel(e), { passive: false });
    this.el.addEventListener("dblclick",    (e) => ih.onDbl(e));
    this.el.addEventListener("contextmenu", (e) => { e.preventDefault(); ih.onRightClick(e); });
    this.el.addEventListener("touchstart",  (e) => ih.onTouchStart(e), { passive: false });
    this.el.addEventListener("touchmove",   (e) => ih.onTouchMove(e),  { passive: false });
    this.el.addEventListener("touchend",    ()  => ih.onTouchEnd());
    window.addEventListener("keydown", ih._keyH);
    window.addEventListener("keyup",   ih._keyUpH);
  }

  // No destroy()/teardown method: Canvas is instantiated exactly once, at
  // startup, and never torn down, so nothing calls one. Don't add a partial
  // version — detaching only some of bind()'s listeners without also calling
  // ro.disconnect() on the ResizeObserver is worse than no teardown at all.
  // A correct version needs stored, removable handler references for every
  // listener bind() adds, plus ro.disconnect().

  // ── Port snap ─────────────────────────────────────────────────────────────

  nearestIn(wx: number, wy: number, excludeId?: string) {
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

  /**
   * Nearest output port within snap range. Skips every port of `excludeNodeId`,
   * or only `excludePortId` on that node when one is given.
   */
  nearestOut(wx: number, wy: number, excludeNodeId?: string, excludePortId?: string) {
    const SNAP = 56; let best: { x: number; y: number; nodeId: string; portId: string } | null = null; let bestD = SNAP;
    for (const n of this.nodes.values()) {
      for (const p of n.ports) {
        if (p.isInput) continue;
        if (n.data.id === excludeNodeId && (excludePortId === undefined || p.id === excludePortId)) continue;
        const d = Math.hypot(wx - p.x, wy - p.y);
        if (d < bestD) { bestD = d; best = { x: p.x, y: p.y, nodeId: n.data.id, portId: p.id }; }
      }
    }
    return best;
  }

  /**
   * The port a pending wire would land on at (wx, wy): an input for a forward
   * wire, an output on another node for a reverse wire, and an output other
   * than the start port for a move.
   */
  snapTarget(pending: PendingConnector, wx: number, wy: number) {
    switch (pending.direction) {
      case "forward": return this.nearestIn(wx, wy, pending.fromNode);
      case "reverse": return this.nearestOut(wx, wy, pending.fromNode);
      case "move":    return this.nearestOut(wx, wy, pending.fromNode, pending.fromPort);
    }
  }

  /** The selected wire, when (wx, wy) is on its target-end drag handle. */
  wireHandleAt(wx: number, wy: number): Connector | null {
    const conn = this.selectedConn;
    if (!conn) return null;
    const tp = this.nodes.get(conn.data.to_node)?.ports.find(p => p.id === conn.data.to_port && p.isInput);
    return tp && Math.hypot(wx - tp.x, wy - tp.y) < PORT_RADIUS + 8 ? conn : null;
  }

  /** Every wire feeding an input port. */
  wiresIntoPort(nodeId: string, portId: string): Connector[] {
    return Array.from(this.connectors.values()).filter(c => c.data.to_node === nodeId && c.data.to_port === portId);
  }

  /** Every wire leaving an output port. */
  wiresFromPort(nodeId: string, portId: string): Connector[] {
    return Array.from(this.connectors.values()).filter(c => c.data.from_node === nodeId && c.data.from_port === portId);
  }

  /**
   * True when an input port accepts any number of wires. The live node
   * descriptor decides; the node's own port list is the fallback for ports a
   * descriptor does not list (config-driven ports). Saved workflows can carry
   * stale ports without `arity`, so the default is single.
   */
  isMultiInput(nodeId: string, portId: string): boolean {
    const node = this.nodes.get(nodeId);
    if (!node) return false;
    const listed = getNodeDescriptor(node.data.node_type_id)?.ports.inputs.find(p => p.id === portId);
    const def = listed ?? node.data.ports.inputs.find(p => p.id === portId);
    return def?.arity === "multi";
  }

  // ── Wire-drop insert ──────────────────────────────────────────────────────

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

  // ── Cut ───────────────────────────────────────────────────────────────────

  private edgeCrossesPath(c: Connector): boolean {
    const fn = this.nodes.get(c.data.from_node), tn = this.nodes.get(c.data.to_node); if (!fn || !tn) return false;
    const fp = fn.ports.find(p => p.id === c.data.from_port), tp = tn.ports.find(p => p.id === c.data.to_port); if (!fp || !tp) return false;
    const cp = Math.max(Math.abs(tp.x - fp.x) * 0.55, 80);
    const pts: { x: number; y: number }[] = [];
    for (let t = 0; t <= 1; t += 0.04) pts.push({ x: bz(fp.x, fp.x + cp, tp.x - cp, tp.x, t), y: bz(fp.y, fp.y, tp.y, tp.y, t) });
    const cutPath = this.input.cutPath;
    for (let i = 0; i < cutPath.length - 1; i++) {
      const ca = cutPath[i], cb = cutPath[i + 1];
      for (let j = 0; j < pts.length - 1; j++) if (segsX(pts[j], pts[j + 1], ca, cb)) return true;
    }
    return false;
  }

  doCut(): Connector[] {
    const out: Connector[] = [];
    for (const [id, c] of this.connectors) if (this.edgeCrossesPath(c)) { out.push(c); this.connectors.delete(id); }
    for (const c of out) this.clearDynamicPortExpr(c);
    return out;
  }

  pruneOrphanedConnectors(nodeId: string): void {
    const node = this.nodes.get(nodeId);
    if (!node) return;
    const validPorts = new Set(node.ports.map(p => p.id));
    for (const [cid, c] of this.connectors) {
      if (c.data.to_node === nodeId && !validPorts.has(c.data.to_port)) {
        if (c.data.to_port === "input") {
          (node.data.config as Record<string, unknown>).files = "";
        }
        this.connectors.delete(cid);
      } else if (c.data.from_node === nodeId && !validPorts.has(c.data.from_port)) {
        this.connectors.delete(cid);
      }
    }
  }

  // ── Connector logic ───────────────────────────────────────────────────────

  clearDynamicPortExpr(conn: Connector): void {
    const target = this.nodes.get(conn.data.to_node);
    if (target?.data.node_type_id === NODE_IDS.AI_PROMPT && conn.data.to_port === "attachments") {
      (target.data.config as Record<string, unknown>).attachments_expr = "";
      return;
    }
    if (target?.data.node_type_id === NODE_IDS.IMAGE_GEN && conn.data.to_port === "reference_images") {
      (target.data.config as Record<string, unknown>).reference_images_expr = "";
      return;
    }
    if (!target?.data.dynamic_ports) return;
    const config = target.data.config as Record<string, unknown>;
    type Slot = { id: string; source_expr?: string };
    const slots = [
      ...((config.subfolders as Slot[] | undefined) ?? []),
      ...((config.sources    as Slot[] | undefined) ?? []),
    ];
    const slot = slots.find(s => s.id === conn.data.to_port);
    if (slot) {
      slot.source_expr = "";
    } else if (conn.data.to_port === "input") {
      config.files = "";
    }
    target.derivePorts(target.data.config as Record<string, unknown>);
    target.rebuildPorts();
  }

  private sourceHasFilesOutput(schema: Record<string, unknown> | null | undefined): boolean {
    if (!schema) return false;
    const props = schema.properties as Record<string, unknown> | undefined;
    return props != null && "files" in props;
  }

  injectDynamicPortExpr(conn: Connector): void {
    const target = this.nodes.get(conn.data.to_node);
    if (target?.data.node_type_id === NODE_IDS.AI_PROMPT && conn.data.to_port === "attachments") {
      const fromNode = this.nodes.get(conn.data.from_node);
      if (!fromNode) return;
      const expr = this.sourceHasFilesOutput(fromNode.data.output_schema)
        ? `{{${fromNode.data.name}.output.files}}`
        : `{{${fromNode.data.name}.output}}`;
      (target.data.config as Record<string, unknown>).attachments_expr = expr;
      return;
    }
    if (target?.data.node_type_id === NODE_IDS.IMAGE_GEN && conn.data.to_port === "reference_images") {
      const fromNode = this.nodes.get(conn.data.from_node);
      if (!fromNode) return;
      const expr = this.sourceHasFilesOutput(fromNode.data.output_schema)
        ? `{{${fromNode.data.name}.output.files}}`
        : `{{${fromNode.data.name}.output}}`;
      (target.data.config as Record<string, unknown>).reference_images_expr = expr;
      return;
    }
    if (!target?.data.dynamic_ports) return;
    const fromNode = this.nodes.get(conn.data.from_node);
    if (!fromNode) return;
    const expr = this.sourceHasFilesOutput(fromNode.data.output_schema)
      ? `{{${fromNode.data.name}.output.files}}`
      : `{{${fromNode.data.name}.output}}`;
    const config = target.data.config as Record<string, unknown>;
    type Slot = { id: string; source_expr?: string };
    const slots = [
      ...((config.subfolders as Slot[] | undefined) ?? []),
      ...((config.sources    as Slot[] | undefined) ?? []),
    ];
    const slot = slots.find(s => s.id === conn.data.to_port);
    if (slot) {
      slot.source_expr = expr;
    } else if (conn.data.to_port === "input") {
      config.files = expr;
    }
    target.derivePorts(target.data.config as Record<string, unknown>);
    target.rebuildPorts();
  }

  finishConn(nodeId: string, portId: string) {
    const pending = this.input.pendingConn;
    if (!pending) return;
    this.connectOrReplace(pending.fromNode, pending.fromPort, nodeId, portId);
  }

  /**
   * Wires an output to an input as one undoable step, with the side effects of
   * dropping a wire (expression injection, change notification). On a
   * single-arity input any wire already there is replaced and a toast names the
   * source it displaced; a multi-arity input just gains the wire. Returns true,
   * changing nothing, when the identical wire exists. Returns false, changing
   * nothing, when either port does not exist or the wire would loop a node back
   * into itself.
   */
  connectOrReplace(srcNode: string, srcPort: string, toNode: string, toPort: string): boolean {
    if (srcNode === toNode) return false;
    if (!this.nodes.get(srcNode)?.ports.some(p => p.id === srcPort && !p.isInput)) return false;
    if (!this.nodes.get(toNode)?.ports.some(p => p.id === toPort && p.isInput)) return false;

    const existing = this.wiresIntoPort(toNode, toPort);
    if (existing.some(c => c.data.from_node === srcNode && c.data.from_port === srcPort)) return true;

    const displaced = this.isMultiInput(toNode, toPort) ? [] : existing;
    const steps: UndoAction[] = [];
    for (const old of displaced) {
      this.connectors.delete(old.data.id);
      this.clearDynamicPortExpr(old);
      steps.push({ type: "delete_edge", connector: old });
    }
    const conn = new Connector({ id: newConnectorId(), from_node: srcNode, from_port: srcPort, to_node: toNode, to_port: toPort, condition: null, on_success: null, on_failure: null });
    this.connectors.set(conn.data.id, conn);
    steps.push({ type: "add_edge", connector: conn });
    if (displaced.length) this.onWarn?.(this.replacedNotice(displaced, toNode, toPort));
    this.applyConnectionDefaults(srcNode, toNode, toPort);
    this.pushUndo(steps.length === 1 ? steps[0] : { type: "batch", actions: steps });
    this.onCanvasChanged?.();
    return true;
  }

  private replacedNotice(displaced: Connector[], toNode: string, toPort: string): string {
    const sources = [...new Set(displaced.map(c => `"${this.nodes.get(c.data.from_node)?.data.name ?? c.data.from_node}"`))];
    const target  = this.nodes.get(toNode);
    const label   = target?.ports.find(p => p.id === toPort && p.isInput)?.label ?? toPort;
    return `Replaced the wire from ${sources.join(", ")} into "${target?.data.name ?? toNode}" (${label}).`;
  }

  /**
   * Wires `fromPort` of one node to `toPort` of another without a drag, exactly
   * as a drop would (see connectOrReplace). Returns false, changing nothing,
   * when either port does not exist.
   */
  connectPorts(fromNode: string, fromPort: string, toNode: string, toPort: string): boolean {
    return this.connectOrReplace(fromNode, fromPort, toNode, toPort);
  }

  /**
   * Moves the target end of an existing wire to `target` as one undoable step.
   * On a single-arity target any other wire there is replaced (recorded on the
   * undo step, with a toast naming its source); a multi-arity target keeps its
   * wires. Nothing is changed (returns false) when there is no target, the
   * target is the wire's current port or its own source node, or the target
   * already has an identical wire.
   */
  rerouteConn(conn: Connector, target: { nodeId: string; portId: string } | null): boolean {
    if (!target) return false;
    const { nodeId, portId } = target;
    const prev = { node: conn.data.to_node, port: conn.data.to_port };
    if (nodeId === prev.node && portId === prev.port) return false;
    if (nodeId === conn.data.from_node) return false;
    const others = this.wiresIntoPort(nodeId, portId).filter(w => w !== conn);
    if (others.some(w => w.data.from_node === conn.data.from_node && w.data.from_port === conn.data.from_port)) {
      this.onWarn?.("That input already has this connection.");
      return false;
    }
    const displaced = this.isMultiInput(nodeId, portId) ? [] : others;
    const touched = [prev.node, nodeId];
    const before = this.configKeys(touched);
    this.clearDynamicPortExpr(conn);
    for (const old of displaced) {
      this.connectors.delete(old.data.id);
      this.clearDynamicPortExpr(old);
    }
    conn.data.to_node = nodeId;
    conn.data.to_port = portId;
    if (displaced.length) this.onWarn?.(this.replacedNotice(displaced, nodeId, portId));
    this.applyConnectionDefaults(conn.data.from_node, nodeId, portId);
    this.pushUndo({
      type: "reroute_edge", connector: conn,
      from: prev, to: { node: nodeId, port: portId },
      configs: this.diffConfigKeys(before, this.configKeys(touched)),
      ...(displaced.length ? { replaced: displaced } : {}),
    });
    this.onCanvasChanged?.();
    return true;
  }

  /**
   * Re-sources every live wire in `group` onto output `to` as one undoable
   * step. Refused with a warning, changing nothing, when a wire would loop a
   * node back into itself or duplicate a wire that already leaves `to`.
   */
  moveSources(group: Connector[], to: { nodeId: string; portId: string }): boolean {
    const wires = group.filter(c => this.connectors.get(c.data.id) === c);
    if (!wires.length) return false;
    if (!this.nodes.get(to.nodeId)?.ports.some(p => p.id === to.portId && !p.isInput)) return false;
    const from = { node: wires[0].data.from_node, port: wires[0].data.from_port };
    if (from.node === to.nodeId && from.port === to.portId) return false;
    if (wires.some(w => w.data.to_node === to.nodeId)) {
      this.onWarn?.("Can't move the wires there: one of them would loop back into its own node.");
      return false;
    }
    const taken = this.wiresFromPort(to.nodeId, to.portId);
    if (taken.some(t => wires.some(w => w.data.to_node === t.data.to_node && w.data.to_port === t.data.to_port))) {
      this.onWarn?.("Can't move the wires there: that output already feeds one of the same inputs.");
      return false;
    }
    const touched = [from.node, to.nodeId, ...wires.map(w => w.data.to_node)];
    const before = this.configKeys(touched);
    for (const w of wires) {
      this.clearDynamicPortExpr(w);
      w.data.from_node = to.nodeId;
      w.data.from_port = to.portId;
      this.applyConnectionDefaults(to.nodeId, w.data.to_node, w.data.to_port);
    }
    this.pushUndo({
      type: "move_sources", connectors: wires,
      from, to: { node: to.nodeId, port: to.portId },
      configs: this.diffConfigKeys(before, this.configKeys(touched)),
    });
    this.onCanvasChanged?.();
    return true;
  }

  private countInputWires(): Map<string, number> {
    const counts = new Map<string, number>();
    for (const c of this.connectors.values()) {
      const key = `${c.data.to_node}:${c.data.to_port}`;
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return counts;
  }

  /** "nodeId:portId" of every single-arity input that more than one wire feeds. */
  private overfedFromCounts(counts: Map<string, number>): Set<string> {
    const out = new Set<string>();
    for (const [key, count] of counts) {
      if (count < 2) continue;
      const i = key.indexOf(":");
      if (!this.isMultiInput(key.slice(0, i), key.slice(i + 1))) out.add(key);
    }
    return out;
  }

  /** Warning text when a single-arity input is fed by more than one wire, else null. */
  arityWarning(): string | null {
    const n = this.overfedFromCounts(this.countInputWires()).size;
    if (!n) return null;
    return `${n === 1 ? "One input has" : `${n} inputs have`} more than one wire (marked in red). The workflow will not run until you remove the extras or route them through a Merge node.`;
  }

  /** Raises arityWarning() through onWarn, once, for a freshly loaded workflow. */
  warnArityViolations(): void {
    const message = this.arityWarning();
    if (message) this.onWarn?.(message);
  }

  private configKeys(nodeIds: string[]): Map<string, Map<string, string>> {
    const out = new Map<string, Map<string, string>>();
    for (const id of new Set(nodeIds)) {
      const n = this.nodes.get(id);
      if (!n) continue;
      const keys = new Map<string, string>();
      for (const [k, v] of Object.entries(n.data.config)) {
        const json = JSON.stringify(v);
        if (json !== undefined) keys.set(k, json);
      }
      out.set(id, keys);
    }
    return out;
  }

  private diffConfigKeys(before: Map<string, Map<string, string>>, after: Map<string, Map<string, string>>): ConfigPatch[] {
    const patches: ConfigPatch[] = [];
    for (const [nodeId, b] of before) {
      const a = after.get(nodeId) ?? new Map<string, string>();
      const keys: ConfigPatch["keys"] = {};
      for (const k of new Set([...b.keys(), ...a.keys()])) {
        const bv = b.get(k) ?? null, av = a.get(k) ?? null;
        if (bv !== av) keys[k] = { before: bv, after: av };
      }
      if (Object.keys(keys).length) patches.push({ nodeId, keys });
    }
    return patches;
  }

  applyConfigPatches(patches: ConfigPatch[], side: "before" | "after"): void {
    for (const patch of patches) {
      const n = this.nodes.get(patch.nodeId);
      if (!n) continue;
      const cfg = n.data.config as Record<string, unknown>;
      for (const [k, v] of Object.entries(patch.keys)) {
        const json = v[side];
        if (json === null) delete cfg[k]; else cfg[k] = JSON.parse(json);
      }
      n.derivePorts(cfg);
      n.rebuildPorts();
    }
  }

  private applyConnectionDefaults(srcId: string, nodeId: string, portId: string) {
    const target = this.nodes.get(nodeId);
    if (target?.data.node_type_id === "text_to_file") {
      const fromNode = this.nodes.get(srcId);
      if (fromNode) {
        const cfg = target.data.config as Record<string, unknown>;
        if (!cfg.content) {
          cfg.content = `{{${fromNode.data.name}.output.content}}`;
        }
      }
    }

    if (target?.data.node_type_id === NODE_IDS.AI_PROMPT && portId === "attachments") {
      const fromNode = this.nodes.get(srcId);
      if (fromNode) {
        const hasFiles = this.sourceHasFilesOutput(fromNode.data.output_schema);
        const expr = hasFiles
          ? `{{${fromNode.data.name}.output.files}}`
          : `{{${fromNode.data.name}.output}}`;
        (target.data.config as Record<string, unknown>).attachments_expr = expr;
        if (!hasFiles) {
          this.onWarn?.(
            'The "Files" port expects a files array. The connected node may not produce one — expression set to {{…output}}, update if needed.'
          );
        }
      }
    }

    if (target?.data.node_type_id === NODE_IDS.IMAGE_GEN && portId === "reference_images") {
      const fromNode = this.nodes.get(srcId);
      if (fromNode) {
        const hasFiles = this.sourceHasFilesOutput(fromNode.data.output_schema);
        const expr = hasFiles
          ? `{{${fromNode.data.name}.output.files}}`
          : `{{${fromNode.data.name}.output}}`;
        (target.data.config as Record<string, unknown>).reference_images_expr = expr;
        if (!hasFiles) {
          this.onWarn?.(
            'The "Reference Images" port expects a files array. The connected node may not produce one — expression set to {{…output}}, update if needed.'
          );
        }
      }
    }

    if (target?.data.dynamic_ports) {
      const fromNode = this.nodes.get(srcId);
      const nodeName = fromNode?.data.name ?? srcId;
      const hasFiles = fromNode != null && this.sourceHasFilesOutput(fromNode.data.output_schema);
      const expr     = hasFiles ? `{{${nodeName}.output.files}}` : `{{${nodeName}.output}}`;
      const toPortDef = target.data.ports.inputs.find(p => p.id === portId);
      if (toPortDef?.port_type === "files" && !hasFiles) {
        this.onWarn?.(
          "This port expects a files array. The connected node may not produce one — expression set to {{…output}}, update if needed."
        );
      }
      const config = target.data.config as Record<string, unknown>;
      type Slot = { id: string; source_expr?: string };
      const slots = [
        ...((config.subfolders as Slot[] | undefined) ?? []),
        ...((config.sources    as Slot[] | undefined) ?? []),
      ];
      const slot = slots.find(s => s.id === portId);
      if (slot) {
        slot.source_expr = expr;
      } else if (portId === "input") {
        config.files = expr;
      }
      target.derivePorts(target.data.config as Record<string, unknown>);
      target.rebuildPorts();
    }
  }

  // ── Selection ─────────────────────────────────────────────────────────────

  selectNode(n: CanvasNode) { n.selected = true; this.selectedNodes.add(n.data.id); this.selectedNode = n; }
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
    for (const c of dc) this.clearDynamicPortExpr(c);
    // Each node's undo action only carries the connectors that actually touch
    // that node, not the full shared dc array: undoing one node from a
    // multi-node delete must not also resurrect connectors whose other
    // endpoint is a still-deleted sibling — a dangling from_node/to_node
    // reference.
    for (const n of dn) {
      const ownConns = dc.filter(c => c.data.from_node === n.data.id || c.data.to_node === n.data.id);
      this.pushUndo({ type: "delete_node", node: n, connectors: ownConns });
    }
    if (!dn.length && dc.length) for (const c of dc) this.pushUndo({ type: "delete_edge", connector: c });
    this.clearSelection(); if (dn.length || dc.length) this.onCanvasChanged?.();
  }

  // ── Undo ──────────────────────────────────────────────────────────────────

  pushUndo(a: UndoAction) { this.undoMgr.push(a); }
  undo() { this.undoMgr.undo(); }
  redo() { this.undoMgr.redo(); }

  // ── Placement ─────────────────────────────────────────────────────────────

  dupSelected() {
    const ids = this.selectedNodes.size > 0 ? [...this.selectedNodes] : this.selectedNode ? [this.selectedNode.data.id] : [];
    if (!ids.length) return;
    const news: CanvasNode[] = [];
    for (const id of ids) {
      const orig = this.nodes.get(id); if (!orig) continue;
      const copy = new CanvasNode({ ...JSON.parse(JSON.stringify(orig.data)), id: `node_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`, position: { x: orig.data.position.x + 30, y: orig.data.position.y + 30 } });
      this.snap.avoidOverlap(copy);
      this.nodes.set(copy.data.id, copy); this.pushUndo({ type: "add_node", node: copy }); news.push(copy);
    }
    this.clearSelection(); for (const n of news) this.selectNode(n); this.onCanvasChanged?.();
  }

  toggleFocusMode() {
    this.focusMode = !this.focusMode;
    document.getElementById("sidebar")?.classList.toggle("focus-hidden", this.focusMode);
    document.body.classList.toggle("focus-mode", this.focusMode);
  }

  // ── Public API ────────────────────────────────────────────────────────────

  placeNode(desc: NodeDescriptor, sx: number, sy: number): CanvasNode {
    const { x, y } = this.s2w(sx, sy);
    const r = this.el.getBoundingClientRect();
    const atCenter = Math.abs(sx - r.width / 2) < 30 && Math.abs(sy - r.height / 2) < 30;
    let px = x - NODE_WIDTH / 2, py = y - 20;

    if (atCenter && this.nodes.size > 0) {
      let maxX = -1e9, avgY = 0, cnt = 0;
      for (const n of this.nodes.values()) { if (n.data.position.x > maxX) maxX = n.data.position.x; avgY += n.data.position.y; cnt++; }
      px = maxX + NODE_WIDTH + 80; py = avgY / cnt - 20;
    }

    const node = createNodeFromDescriptor(desc, px, py);
    this.nodes.set(node.data.id, node);
    this.pushUndo({ type: "add_node", node });
    if (this._pendingPresetConfig) {
      Object.assign(node.data.config, this._pendingPresetConfig);
      this._pendingPresetConfig = undefined;
    }
    if (this._pendingPresetName) {
      node.data.name = this._pendingPresetName;
      this._pendingPresetName = undefined;
    }
    this.snap.avoidOverlap(node);
    this.tryWireInsert(node);
    this.onCanvasChanged?.();
    this.centerOn(node.data.position.x + NODE_WIDTH / 2, node.data.position.y + node.height / 2);
    return node;
  }

  centerOn(wx: number, wy: number) {
    const r = this.el.getBoundingClientRect();
    this.panX = r.width / 2 - wx * this.zoom;
    this.panY = unobstructedHeight(r) / 2 - wy * this.zoom;
  }

  setNodeStatus(id: string, s: "idle" | "running" | "success" | "error") { const n = this.nodes.get(id); if (n) { n.status = s; if (s === "success") n.missingRequired = false; } }
  setNodeOutput(id: string, p: string) { const n = this.nodes.get(id); if (n) { n.outputPreview = p; n.status = "success"; n.missingRequired = false; } }
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
    // Floor guards against the drawer covering nearly the whole canvas on a
    // short viewport (e.g. maximized drawer at 70vh) driving the height term
    // to zero/negative, which would invert the zoom instead of just fitting
    // tighter than usual.
    const h   = Math.max(unobstructedHeight(r), pad * 2 + 40);
    this.zoom = Math.min((r.width - pad * 2) / ((mxX - mnX) || 1), (h - pad * 2) / ((mxY - mnY) || 1), 1.2);
    this.panX = r.width / 2 - ((mnX + mxX) / 2) * this.zoom;
    this.panY = h / 2 - ((mnY + mxY) / 2) * this.zoom;
    // Notify zoom-hint and viewport-persistence listeners, same as onWheel
    // does after every zoom change -- fitToScreen is just another way the
    // zoom/pan can change.
    this.onZoomChange?.(this.zoom);
    this.onViewportChange?.();
  }

  /** Zoom by `factor` anchored on the viewport's visual center — the same
   * clamp (MIN_ZOOM/MAX_ZOOM) and anchor-preserving pan formula InputHandler's
   * onWheel already uses, just anchored at the viewport center instead of the
   * cursor position (a button click has no cursor-over-canvas position to
   * anchor on). No new zoom math — mirrors the existing formula exactly. */
  private zoomAtCenter(factor: number) {
    const r = this.el.getBoundingClientRect();
    const cx = r.width / 2, cy = unobstructedHeight(r) / 2;
    const { x: wx, y: wy } = this.s2w(cx, cy);
    const nz = Math.min(this.MAX_ZOOM, Math.max(this.MIN_ZOOM, this.zoom * factor));
    this.panX = cx - wx * nz;
    this.panY = cy - wy * nz;
    this.zoom = nz;
    this.onZoomChange?.(this.zoom);
    this.onViewportChange?.();
  }

  /** Same step factor as onWheel's non-pinch wheel tick (1/0.90). */
  zoomIn()  { this.zoomAtCenter(1 / 0.90); }
  zoomOut() { this.zoomAtCenter(0.90); }


  completeWireDrop(desc: NodeDescriptor): void {
    const drop = this._pendingWireDrop;
    if (!drop) return;
    this._pendingWireDrop = null;

    if (!desc.ports.inputs.length) return;

    const px = drop.wx - NODE_WIDTH / 2;
    const py = drop.wy - 20;
    const node = createNodeFromDescriptor(desc, px, py);

    if (this._pendingPresetConfig) { Object.assign(node.data.config, this._pendingPresetConfig); this._pendingPresetConfig = undefined; }
    if (this._pendingPresetName)   { node.data.name = this._pendingPresetName; this._pendingPresetName = undefined; }

    this.snap.avoidOverlap(node);
    this.nodes.set(node.data.id, node);
    this.pushUndo({ type: "add_node", node });
    this.onCanvasChanged?.();

    this.connectOrReplace(drop.fromNode, drop.fromPort, node.data.id, desc.ports.inputs[0].id);
  }

  completeInputWireDrop(desc: NodeDescriptor): void {
    const drop = this._pendingInputWireDrop;
    if (!drop) return;
    this._pendingInputWireDrop = null;

    if (!desc.ports.outputs.length) return;

    const px = drop.wx - NODE_WIDTH * 1.5;
    const py = drop.wy - 20;
    const node = createNodeFromDescriptor(desc, px, py);

    this.snap.avoidOverlap(node);
    this.nodes.set(node.data.id, node);
    this.pushUndo({ type: "add_node", node });
    this.onCanvasChanged?.();

    this.connectOrReplace(node.data.id, desc.ports.outputs[0].id, drop.toNode, drop.toPort);
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
