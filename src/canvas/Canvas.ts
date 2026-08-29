import { CanvasNode, createNodeFromDescriptor, NODE_WIDTH, PORT_RADIUS, roundedRect } from "./Node";
import { Connector, newConnectorId } from "./Connector";
import { serialize } from "./CanvasSerializer";
import { NODE_IDS } from "../node-ids";
import type { NodeDescriptor } from "../ipc/workflow";
import { UndoManager } from "./UndoManager";
import type { UndoAction } from "./UndoManager";
import { Minimap } from "./Minimap";
import { SnapEngine } from "./SnapEngine";
import { ContextMenu } from "./ContextMenu";
import { InputHandler } from "./InputHandler";
import { getCanvasColors } from "./theme-colors";

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

    // Draw connectors
    for (const c of this.connectors.values()) {
      c.highlighted = ih.isCutting && this.edgeCrossesPath(c);
      c.draw(ctx, this.nodes, dt);
    }
    if (ih.pendingConn) ih.pendingConn.draw(ctx);

    const colors = getCanvasColors();

    // Port snap ring
    if (ih.pendingConn) {
      const snap = this.nearestIn(ih.pendingConn.toX, ih.pendingConn.toY, ih.pendingConn.fromNode);
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
      const srcPort = srcNode?.ports.find(p => p.id === ih.pendingConn!.fromPort);
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

    // Build connected-port lookup for live port fill state (UX-4) and the
    // per-output-port connection count for badges (G12) in a single pass —
    // both are derived from the same connector list, which is unchanged
    // frame-to-frame outside an edit; this loop still runs every frame, but
    // now walks `this.connectors` once instead of twice.
    const connectedPorts = new Set<string>();
    const outCount = new Map<string, number>();
    for (const c of this.connectors.values()) {
      const fromKey = `${c.data.from_node}:${c.data.from_port}`;
      connectedPorts.add(fromKey);
      connectedPorts.add(`${c.data.to_node}:${c.data.to_port}`);
      outCount.set(fromKey, (outCount.get(fromKey) ?? 0) + 1);
    }

    // Draw nodes
    for (const n of this.nodes.values()) n.draw(ctx, dt, connectedPorts);

    // ── G12: output port connection-count badges ───────────────────────────
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

    // Hover tooltip for nodes with truncated names (UX-1)
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

  // Screen-space tooltip for a hovered node whose name is truncated (UX-1).
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

  // destroy was removed here — it had zero callers
  // anywhere in the app (Canvas is instantiated exactly once, at startup,
  // and never torn down) and was incomplete even if it had been called:
  // it only removed 5 of bind()'s 15 registered listeners and never
  // disconnected the ResizeObserver. No concrete near-term feature needs
  // teardown; reintroduce a correct version (stored, removable handler
  // references for every listener bind() adds, plus ro.disconnect()) in
  // the same patch that actually needs it.

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
    if (!this.input.pendingConn) return;
    if (Array.from(this.connectors.values()).some(c => c.data.to_node === nodeId && c.data.to_port === portId)) return;
    const conn = new Connector({ id: newConnectorId(), from_node: this.input.pendingConn.fromNode, from_port: this.input.pendingConn.fromPort, to_node: nodeId, to_port: portId, condition: null, on_success: null, on_failure: null });
    this.connectors.set(conn.data.id, conn);
    this.pushUndo({ type: "add_edge", connector: conn });

    const target = this.nodes.get(nodeId);
    if (target?.data.node_type_id === "text_to_file") {
      const fromNode = this.nodes.get(this.input.pendingConn.fromNode);
      if (fromNode) {
        const cfg = target.data.config as Record<string, unknown>;
        if (!cfg.content) {
          cfg.content = `{{${fromNode.data.name}.output.content}}`;
        }
      }
    }

    if (target?.data.node_type_id === NODE_IDS.AI_PROMPT && portId === "attachments") {
      const fromNode = this.nodes.get(this.input.pendingConn.fromNode);
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
      const fromNode = this.nodes.get(this.input.pendingConn.fromNode);
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
      const fromNode = this.nodes.get(this.input.pendingConn.fromNode);
      const nodeName = fromNode?.data.name ?? this.input.pendingConn.fromNode;
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

    this.onCanvasChanged?.();
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
    this.panX = r.width  / 2 - wx * this.zoom;
    this.panY = r.height / 2 - wy * this.zoom;
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
    this.zoom = Math.min((r.width - pad * 2) / ((mxX - mnX) || 1), (r.height - pad * 2) / ((mxY - mnY) || 1), 1.2);
    this.panX = r.width / 2 - ((mnX + mxX) / 2) * this.zoom;
    this.panY = r.height / 2 - ((mnY + mxY) / 2) * this.zoom;
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
    const cx = r.width / 2, cy = r.height / 2;
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

    const outputPortId = desc.ports.outputs[0].id;
    const conn = new Connector({
      id: newConnectorId(),
      from_node: node.data.id, from_port: outputPortId,
      to_node: drop.toNode, to_port: drop.toPort,
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
