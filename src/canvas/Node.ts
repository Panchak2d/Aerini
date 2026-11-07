import type { NodeDescriptor, PortDefinition } from "../ipc/workflow";
import { NODE_IDS, DANGEROUS_NODE_IDS } from "../node-ids";
import { getIconBitmap } from "../icon-cache";
import { getCanvasColors, tileTint, resolveNoteColors, withAlpha } from "./theme-colors";
import { markDirty } from "./render-state";
import { isPluginNodeType, isUnregisteredNodeType, isTriggerNodeType } from "./node-registry";
import { displayPortLabel } from "../port-labels";
import { parseNote, layoutNote, type LaidLine } from "./note-format";
import {
  noteStyleOf, minSize, clampDim, frameHit, handleAt, NOTE_DEFAULT_W, NOTE_MAX_SIZE, FRAME_DEFAULT_W,
  FRAME_DEFAULT_H, FRAME_HEADER, noteSizeOf, noteMetrics, type Handle, type NoteMetrics, type NoteStyle,
} from "./note-geometry";

export interface CanvasNodeData {
  id: string;
  node_type_id: string;
  node_type: "action" | "ai" | "logic" | "utility";
  name: string;
  config: Record<string, unknown>;
  credentials: Record<string, string>;
  position: { x: number; y: number };
  ports: { inputs: PortDefinition[]; outputs: PortDefinition[] };
  input_schema: Record<string, unknown>;
  output_schema: Record<string, unknown>;
  retry: { max_attempts: number; backoff_ms: number };
  fallback_node: string | null;
  /** Mirrors WorkflowNode.disabled (model.rs) — engine skips execution when true.
   *  Optional so callers that construct fresh nodes (createNodeFromDescriptor)
   *  need not set it; absent/undefined means false. */
  disabled?: boolean;
  /** True when this node's input ports are derived from config slots at runtime. */
  dynamic_ports: boolean;
}

export interface Port {
  id: string;
  label: string;
  isInput: boolean;
  x: number;
  y: number;
}

export const NODE_WIDTH  = 220;
export const NODE_HEADER = 40;
export const PORT_RADIUS = 6;
export const PORT_GAP    = 28;
const PADDING            = 14;
const CORNER_R           = 10;
const NOTE_FAMILY        = "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif";
const NOTE_PAD_X         = 12;
const BOX_REF            = 12;
const TILE_SIZE          = 26;
const TILE_INSET         = 10;
const TILE_CORNER_R      = 7;
const TRIGGER_CORNER_R   = 22;

type CatKey = "action" | "ai" | "logic" | "utility" | "trigger";


const TYPE_META_BASE: Record<Exclude<CatKey, "trigger">, {
  icon:  string;
  label: string;
}> = {
  action:  { icon: "A",  label: "ACTION"  },
  ai:      { icon: "AI", label: "AI"      },
  logic:   { icon: "L",  label: "LOGIC"   },
  utility: { icon: "U",  label: "UTILITY" },
};

const CAT_KEY_TO_COLORS_FIELD = {
  action: "catAction", ai: "catAI", logic: "catLogic", utility: "catUtility", trigger: "catTrigger",
} as const satisfies Record<CatKey, keyof ReturnType<typeof getCanvasColors>>;

/** Per-kind chrome (icon/label) plus its live, theme-reactive accent color. */
export function typeMeta(nodeType: string): { accent: string; icon: string; label: string } {
  const key = (nodeType in TYPE_META_BASE ? nodeType : "utility") as Exclude<CatKey, "trigger">;
  const accent = getCanvasColors()[CAT_KEY_TO_COLORS_FIELD[key]];
  return { accent, ...TYPE_META_BASE[key] };
}


export function statusAccent(status: CanvasNode["status"], colors: ReturnType<typeof getCanvasColors>): string | null {
  switch (status) {
    case "running": return colors.warning;
    case "success": return colors.actionRun;
    case "error":   return colors.error;
    default:        return null;
  }
}


export class CanvasNode {
  data: CanvasNodeData;
  ports: Port[] = [];
  private _selected = false;
  private _hovered = false;
  private _status: "idle" | "running" | "success" | "error" = "idle";
  private _missingRequired = false;
  private _hasProblem = false;
  private _outputPreview: string | null = null;
  private _showOutput = false;
  private _cached = false;

  get cached(): boolean { return this._cached; }
  set cached(v: boolean) { if (v !== this._cached) { this._cached = v; markDirty(); } }

  get selected(): boolean { return this._selected; }
  set selected(v: boolean) { if (v !== this._selected) { this._selected = v; markDirty(); } }

  private _dropTarget = false;
  /** True on a frame while a dragged set would land fully inside it. */
  get dropTarget(): boolean { return this._dropTarget; }
  set dropTarget(v: boolean) { if (v !== this._dropTarget) { this._dropTarget = v; markDirty(); } }

  get hovered(): boolean { return this._hovered; }
  set hovered(v: boolean) { if (v !== this._hovered) { this._hovered = v; markDirty(); } }

  get status(): "idle" | "running" | "success" | "error" { return this._status; }
  set status(v: "idle" | "running" | "success" | "error") { if (v !== this._status) { this._status = v; markDirty(); } }

  // True when this node has a required config field left empty after its
  // popover was closed. Cleared on a successful run.
  get missingRequired(): boolean { return this._missingRequired; }
  set missingRequired(v: boolean) { if (v !== this._missingRequired) { this._missingRequired = v; markDirty(); } }

  // True when a reference or name in this node's config is known to be broken.
  get hasProblem(): boolean { return this._hasProblem; }
  set hasProblem(v: boolean) { if (v !== this._hasProblem) { this._hasProblem = v; markDirty(); } }

  // Output preview (set after execution)
  get outputPreview(): string | null { return this._outputPreview; }
  set outputPreview(v: string | null) { if (v !== this._outputPreview) { this._outputPreview = v; markDirty(); } }

  get showOutput(): boolean { return this._showOutput; }
  set showOutput(v: boolean) { if (v !== this._showOutput) { this._showOutput = v; markDirty(); } }

  // Running animation phase (0–1, loops)
  private _runPhase = 0;

  private _noteLayout: { key: string; lines: LaidLine[] } | null = null;

  // Backing field for `disabled`. The accessor below keeps `data.disabled`
  // in sync on every write, so ContextMenu's Disable/Enable toggle (which
  // only ever does `node.disabled = ...`) is automatically reflected in
  // toWorkflowNode()'s output with no separate sync step required.
  private _disabled = false;

  get disabled(): boolean { return this._disabled; }
  set disabled(v: boolean) {
    if (v !== this._disabled) markDirty();
    this._disabled = v;
    this.data.disabled = v;
  }

  get isNote(): boolean { return this.data.node_type_id === NODE_IDS.NOTE; }
  get noteStyle(): NoteStyle { return noteStyleOf(this.data.config); }

  get width(): number {
    if (!this.isNote) return NODE_WIDTH;
    const frame = this.noteStyle === "frame";
    return clampDim(this.data.config["width"], minSize(this.noteStyle).w, NOTE_MAX_SIZE, frame ? FRAME_DEFAULT_W : NOTE_DEFAULT_W);
  }

  get minHeight(): number {
    if (this.noteStyle === "frame") return minSize("frame").h;
    return Math.max(minSize("note").h, 28 + this.noteLayoutFor().lines.length * this.noteMetrics.line);
  }

  get height(): number {
    if (this.isNote) {
      const set = this.data.config["height"];
      if (this.noteStyle === "frame") return clampDim(set, minSize("frame").h, NOTE_MAX_SIZE, FRAME_DEFAULT_H);
      return Math.max(this.minHeight, clampDim(set, 0, NOTE_MAX_SIZE, 0));
    }
    const maxPorts = Math.max(
      this.data.ports.inputs.length,
      this.data.ports.outputs.length
    );
    const bodyRows = Math.max(maxPorts, 1);
    const hasPreview = this.showOutput && this.outputPreview;
    return NODE_HEADER + bodyRows * PORT_GAP + PADDING + (hasPreview ? 36 : 0);
  }

  constructor(data: CanvasNodeData) {
    this.data = data;
    this.disabled = data.disabled ?? false;
    // For dynamic-port nodes, derive ports from config before building
    // position-aware port list. This ensures ports are correct on first render
    // and after loading a saved workflow.
    if (this.data.dynamic_ports) {
      this.derivePorts(this.data.config);
    }
    this.rebuildPorts();
  }

  /**
   * Rebuilds data.ports.inputs from the node's config slots.
   * Only has effect when data.dynamic_ports is true.
   *
   * Config shapes supported:
   *   config.subfolders: Array<{ id: string; name: string }> — save_to_folder
   *   config.sources:    Array<{ id: string; name: string }> — collect_files
   *
   * When no slots are present, falls back to a single "input" port.
   * Output ports are preserved as-is (always the static "output" port).
   */
  derivePorts(config: Record<string, unknown>): void {
    if (!this.data.dynamic_ports) return;

    type Slot = { id: string; name: string };
    const subfolders = (config.subfolders as Slot[] | undefined) ?? [];
    const sources    = (config.sources    as Slot[] | undefined) ?? [];
    const slots      = subfolders.length > 0 ? subfolders : sources;

    if (slots.length > 0) {
      this.data.ports.inputs = slots.map(s => ({
        id:        s.id,
        label:     s.name,
        position:  "left" as const,
        port_type: "files" as const,
      }));
    } else {
      // No slots configured yet — show a single placeholder input
      this.data.ports.inputs = [{ id: "input", label: "In", position: "left" as const, port_type: "files" as const }];
    }

    // Ensure at least one output port exists
    if (!this.data.ports.outputs.length) {
      this.data.ports.outputs = [{ id: "output", label: "Out", position: "right" as const }];
    }
  }

  rebuildPorts(): void {
    this.ports = [];
    const { x, y } = this.data.position;
    this.data.ports.inputs.forEach((p, i) => {
      this.ports.push({
        id: p.id, label: p.label, isInput: true,
        x: x,
        y: y + NODE_HEADER + PORT_GAP * i + PORT_GAP / 2,
      });
    });
    this.data.ports.outputs.forEach((p, i) => {
      this.ports.push({
        id: p.id, label: displayPortLabel(this.data.node_type_id, p.id, p.label), isInput: false,
        x: x + NODE_WIDTH,
        y: y + NODE_HEADER + PORT_GAP * i + PORT_GAP / 2,
      });
    });
  }

  updatePortPositions(): void { this.rebuildPorts(); }

  containsPoint(wx: number, wy: number): boolean {
    const { x, y } = this.data.position;
    if (this.isNote && this.noteStyle === "frame") return frameHit({ x, y, w: this.width, h: this.height }, wx, wy);
    return wx >= x && wx <= x + this.width && wy >= y && wy <= y + this.height;
  }

  resizeHandleAt(wx: number, wy: number, zoom: number): Handle | null {
    const { x, y } = this.data.position;
    return handleAt({ x, y, w: this.width, h: this.height }, wx, wy, zoom);
  }

  get noteMetrics(): NoteMetrics {
    return noteMetrics(noteSizeOf(this.data.config));
  }

  noteTargetAt(wx: number, wy: number): { kind: "link"; href: string } | { kind: "check"; src: number } | null {
    if (!this.isNote || this.noteStyle !== "note") return null;
    const { x, y } = this.data.position;
    const lines = this.noteLayoutFor().lines;
    const top = y + 12;
    const m = this.noteMetrics;
    const i = Math.floor((wy - top) / m.line);
    if (i < 0 || i >= lines.length || wx < x) return null;
    const line = lines[i];
    const lx = x + NOTE_PAD_X + line.indent;
    if (line.box && wx >= x + NOTE_PAD_X && wx <= x + NOTE_PAD_X + m.box + 4) return { kind: "check", src: line.src };
    for (const run of line.runs) {
      if (run.href && wx >= lx + run.x && wx <= lx + run.x + run.w) return { kind: "link", href: run.href };
    }
    return null;
  }

  private noteLayoutFor(): { lines: LaidLine[] } {
    const text = String(this.data.config["text"] ?? "");
    const w = this.width;
    const m = this.noteMetrics;
    const key = `${w}\u0000${m.body}\u0000${text}`;
    if (this._noteLayout?.key === key) return this._noteLayout;
    const lines = layoutNote(parseNote(text), (t, bold, heading) => measureNote(t, bold, heading, m), w - NOTE_PAD_X * 2, m.indent);
    this._noteLayout = { key, lines };
    return this._noteLayout;
  }

  portAtPoint(wx: number, wy: number): Port | null {
    for (const p of this.ports) {
      if (Math.hypot(wx - p.x, wy - p.y) <= PORT_RADIUS + 6) return p;
    }
    return null;
  }

  draw(ctx: CanvasRenderingContext2D, dt = 0, connectedPorts: Set<string> = new Set(), overfedPorts: Set<string> = new Set()): void {
    // Note node gets its own minimal sticky-note rendering
    if (this.data.node_type_id === NODE_IDS.NOTE) {
      this.drawNote(ctx);
      return;
    }

    if (this.status === "running") {
      this._runPhase = (this._runPhase + dt * 0.8) % 1;
    } else {
      this._runPhase = 0;
    }

    const { x, y } = this.data.position;
    const w = NODE_WIDTH;
    const h = this.height;
    const r = CORNER_R;
    const colors = getCanvasColors();
    const meta = typeMeta(this.data.node_type);
    const accent = statusAccent(this.status, colors) ?? meta.accent;

    // Trigger identity is independent of NodeType: schedule/webhook/manual_trigger are
    // node_type "action" in the engine, so the check goes by node_type_id (built-in ids
    // plus trigger-capable plugins). It swaps only the shape (rounded left edge, round
    // icon tile) and the subtitle; `accent` stays on the category/status color because
    // icon-cache.ts preloads bitmaps for exactly those colors and nothing else.
    const isTrigger      = isTriggerNodeType(this.data.node_type_id);
    const identityAccent = isTrigger ? colors.catTrigger : meta.accent;
    const identityLabel  = isTrigger ? "TRIGGER" : meta.label;
    const radii: CornerRadii = isTrigger
      ? { tl: TRIGGER_CORNER_R, tr: r, br: r, bl: TRIGGER_CORNER_R }
      : { tl: r, tr: r, br: r, bl: r };

    ctx.save();
    if (this.disabled) ctx.globalAlpha = 0.35;

    if (this.selected) {
      drawCornerBrackets(ctx, x, y, w, h, withAlpha(accent, "88"), 2);
    } else if (this.hovered) {
      roundedRect(ctx, x - 2, y - 2, w + 4, h + 4, radii);
      ctx.strokeStyle = withAlpha(colors.border, "b0");
      ctx.lineWidth   = 1.5;
      ctx.stroke();
    }

    roundedRect(ctx, x, y, w, h, radii);
    ctx.fillStyle = colors.surface1;
    ctx.fill();
    ctx.strokeStyle = this.selected
      ? accent
      : this.hovered
      ? withAlpha(colors.border, "99")
      : withAlpha(colors.border, "80");
    ctx.lineWidth = this.selected ? 1.5 : 1;
    ctx.stroke();

    if (this.status === "running") {
      const grad = ctx.createLinearGradient(x, y, x + w, y);
      const p    = this._runPhase;
      grad.addColorStop(Math.max(0, p - 0.3), "transparent");
      grad.addColorStop(p, accent);
      grad.addColorStop(Math.min(1, p + 0.3), "transparent");
      ctx.beginPath();
      ctx.moveTo(x + radii.tl, y);
      ctx.lineTo(x + w - radii.tr, y);
      ctx.strokeStyle = grad;
      ctx.lineWidth   = 2;
      ctx.stroke();
    }

    // ── Icon tile ──────────────────────────────────────────────────────────
    const tileX = x + TILE_INSET;
    const tileY = y + (NODE_HEADER - TILE_SIZE) / 2;
    const tileCx = tileX + TILE_SIZE / 2;
    const tileCy = tileY + TILE_SIZE / 2;
    roundedRect(ctx, tileX, tileY, TILE_SIZE, TILE_SIZE, isTrigger ? TILE_SIZE / 2 : TILE_CORNER_R);
    ctx.fillStyle = tileTint(accent);
    ctx.fill();

    const bmp = getIconBitmap(this.data.node_type_id, accent);
    if (bmp) {
      ctx.drawImage(bmp, tileCx - 8, tileCy - 8, 16, 16);
    } else {
      ctx.font         = "bold 12px monospace";
      ctx.fillStyle    = accent;
      ctx.textBaseline = "middle";
      ctx.textAlign    = "center";
      ctx.fillText(meta.icon, tileCx, tileCy);
    }

    // ── Status dot (top-right) ─────────────────────────────────────────────
    if (this.status !== "idle") {
      const dotX = x + w - 12;
      const dotY = y + 14;
      ctx.beginPath();
      ctx.arc(dotX, dotY, 4, 0, Math.PI * 2);
      ctx.fillStyle = accent;
      ctx.fill();
      if (this.status === "running") {
        const alpha = Math.sin(this._runPhase * Math.PI * 2) * 0.5 + 0.5;
        ctx.beginPath();
        ctx.arc(dotX, dotY, 4 + 4 * alpha, 0, Math.PI * 2);
        ctx.strokeStyle = accent + Math.round(alpha * 255).toString(16).padStart(2,"0");
        ctx.lineWidth   = 1;
        ctx.stroke();
      }
    }

    // ── Incomplete-config badge (tile top-right, idle only) ─────────────────
    if ((this.missingRequired || this.hasProblem) && this.status === "idle") {
      const bx = tileX + TILE_SIZE;
      const by = tileY;
      ctx.beginPath();
      ctx.arc(bx, by, 6, 0, Math.PI * 2);
      ctx.fillStyle = colors.surface2;
      ctx.fill();
      ctx.strokeStyle = colors.warning;
      ctx.lineWidth   = 1.5;
      ctx.stroke();
      ctx.font         = "bold 9px -apple-system, BlinkMacSystemFont, sans-serif";
      ctx.fillStyle    = colors.warning;
      ctx.textAlign    = "center";
      ctx.textBaseline = "middle";
      ctx.fillText("!", bx, by + 0.5);
    }

    // The registry lookup is by type_id, so CanvasNodeData and the .aerini format
    // never carry a plugin flag.
    if (isPluginNodeType(this.data.node_type_id)) {
      drawTypeBadge(ctx, tileX + TILE_SIZE - 2, tileY + TILE_SIZE - 2, "P");
    } else if (isUnregisteredNodeType(this.data.node_type_id)) {
      drawTypeBadge(ctx, tileX + TILE_SIZE - 2, tileY + TILE_SIZE - 2, "?");
    }

    // ── Name + subtitle ────────────────────────────────────────────────────
    const textX = tileX + TILE_SIZE + 10;
    ctx.font         = "500 13px -apple-system, BlinkMacSystemFont, 'SF Pro Text', sans-serif";
    ctx.fillStyle    = colors.textPrimary;
    ctx.textBaseline = "middle";
    ctx.textAlign    = "left";
    ctx.fillText(trunc(this.data.name, 20), textX, y + 14);

    ctx.font      = "600 9px -apple-system, BlinkMacSystemFont, sans-serif";
    ctx.fillStyle = identityAccent;
    ctx.fillText(identityLabel, textX, y + 28);

    // The right end of the subtitle row holds the DISABLED tag, with the danger badge beside it.
    let rightEdge = x + w - 12;
    if (this.disabled) {
      ctx.font      = "700 9px -apple-system, BlinkMacSystemFont, sans-serif";
      ctx.fillStyle = colors.error;
      ctx.textAlign = "right";
      ctx.fillText("DISABLED", rightEdge, y + 28);
      rightEdge -= ctx.measureText("DISABLED").width + 10;
    }
    if (DANGEROUS_NODE_IDS.has(this.data.node_type_id)) {
      drawDangerBadge(ctx, rightEdge - 2, y + 28);
    }

    // ── Ports ──────────────────────────────────────────────────────────────
    ctx.font         = "11px -apple-system, BlinkMacSystemFont, sans-serif";
    ctx.textBaseline = "middle";

    for (const port of this.ports) {
      const isConnected = connectedPorts.has(`${this.data.id}:${port.id}`);

      if (port.isInput && overfedPorts.has(`${this.data.id}:${port.id}`)) {
        ctx.beginPath();
        ctx.arc(port.x, port.y, PORT_RADIUS + 5, 0, Math.PI * 2);
        ctx.strokeStyle = colors.error;
        ctx.lineWidth   = 2;
        ctx.stroke();
      }

      // Port outer ring — matches the node body it's cut out of
      ctx.beginPath();
      ctx.arc(port.x, port.y, PORT_RADIUS + 2, 0, Math.PI * 2);
      ctx.fillStyle = colors.surface1;
      ctx.fill();

      // Port fill — solid when connected, hollow when empty
      ctx.beginPath();
      ctx.arc(port.x, port.y, PORT_RADIUS, 0, Math.PI * 2);
      ctx.fillStyle   = isConnected ? withAlpha(accent, "cc") : colors.surface2;
      ctx.strokeStyle = withAlpha(accent, "cc");
      ctx.lineWidth   = 1.5;
      ctx.fill();
      ctx.stroke();

      // Port dot — full opacity when connected, dim when empty
      ctx.beginPath();
      ctx.arc(port.x, port.y, 2.5, 0, Math.PI * 2);
      ctx.fillStyle = isConnected ? accent : withAlpha(accent, "33");
      ctx.fill();

      // Port label
      ctx.fillStyle = colors.textSecondary;
      if (port.isInput) {
        ctx.textAlign = "left";
        ctx.fillText(port.label, port.x + PORT_RADIUS + 7, port.y);
      } else {
        ctx.textAlign = "right";
        ctx.fillText(port.label, port.x - PORT_RADIUS - 7, port.y);
      }
    }

    // ── Output preview strip ───────────────────────────────────────────────
    if (this.showOutput && this.outputPreview) {
      const previewY = y + h - 34;
      // Separator
      ctx.beginPath();
      ctx.moveTo(x + 3, previewY);
      ctx.lineTo(x + w, previewY);
      ctx.strokeStyle = withAlpha(colors.border, "30");
      ctx.lineWidth   = 1;
      ctx.stroke();

      // Preview text
      ctx.font         = "11px 'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace";
      ctx.fillStyle    = withAlpha(colors.actionRun, "88");
      ctx.textBaseline = "middle";
      ctx.textAlign    = "left";
      ctx.fillText(trunc(this.outputPreview, this._cached ? 18 : 28), x + 10, previewY + 17);
      if (this._cached) {
        ctx.font      = "10px system-ui, sans-serif";
        ctx.fillStyle = withAlpha(colors.border, "cc");
        ctx.textAlign = "right";
        ctx.fillText("cached", x + w - 10, previewY + 17);
      }
    }

    ctx.restore();
  }

  // ── Note node renderer ────────────────────────────────────────────────────

  private drawNote(ctx: CanvasRenderingContext2D): void {
    const { x, y } = this.data.position;
    const w = this.width;
    const h = this.height;
    const color = String(this.data.config["color"] ?? "default");
    const colors = getCanvasColors();
    const noteColors = resolveNoteColors(colors);
    const c = noteColors[color] ?? noteColors.default;

    ctx.save();
    if (this.noteStyle === "frame") this.drawFrame(ctx, x, y, w, h, c, colors);
    else this.drawSticky(ctx, x, y, w, h, c);
    if (this.selected) drawResizeHandles(ctx, x, y, w, h, c.text, colors.surface1);
    ctx.restore();
  }

  private drawSticky(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, c: { bg: string; border: string; text: string }): void {
    if (this.selected) {
      roundedRect(ctx, x - 2, y - 2, w + 4, h + 4, CORNER_R + 2);
      ctx.strokeStyle = c.border;
      ctx.lineWidth = 2;
      ctx.stroke();
    }

    roundedRect(ctx, x, y, w, h, CORNER_R);
    ctx.fillStyle = c.bg;
    ctx.fill();
    ctx.strokeStyle = c.border;
    ctx.lineWidth = 1;
    ctx.stroke();

    ctx.save();
    roundedRect(ctx, x, y, w, h, CORNER_R);
    ctx.clip();
    ctx.fillStyle = c.border;
    ctx.fillRect(x, y, w, 3);
    ctx.restore();

    const m = this.noteMetrics;
    const text = String(this.data.config["text"] ?? "");
    if (!text) {
      ctx.font = noteFont(m, false, false);
      ctx.fillStyle = withAlpha(c.text, "99");
      ctx.textBaseline = "top";
      ctx.textAlign = "left";
      ctx.fillText("Double-click to edit", x + NOTE_PAD_X, y + 12);
      return;
    }

    ctx.textBaseline = "top";
    ctx.textAlign = "left";
    const lines = this.noteLayoutFor().lines;
    lines.forEach((line, i) => {
      const ly = y + 12 + i * m.line;
      const lx = x + NOTE_PAD_X + line.indent;
      const done = line.indent > 0 && line.checked;
      if (line.box) drawCheckbox(ctx, x + NOTE_PAD_X, ly + 1, m.box, line.checked, c.text);
      for (const run of line.runs) {
        ctx.font = noteFont(m, run.bold, line.heading);
        ctx.fillStyle = done ? withAlpha(c.text, "88") : c.text;
        ctx.fillText(run.text, lx + run.x, ly);
        if (run.href) {
          ctx.fillRect(lx + run.x, ly + (line.heading ? m.heading : m.body) + 2, run.w, 1);
        }
        if (done) ctx.fillRect(lx + run.x, ly + Math.round(m.body * 0.58), run.w, 1);
      }
    });
  }

  private drawFrame(
    ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number,
    c: { bg: string; border: string; text: string }, colors: ReturnType<typeof getCanvasColors>,
  ): void {
    roundedRect(ctx, x, y, w, h, CORNER_R + 2);
    ctx.fillStyle = withAlpha(c.text, this.dropTarget ? "1c" : "10");
    ctx.fill();
    const solid = this.selected || this.dropTarget;
    ctx.setLineDash(solid ? [] : [6, 4]);
    ctx.strokeStyle = solid ? c.text : withAlpha(c.text, "77");
    ctx.lineWidth = this.dropTarget ? 2.5 : this.selected ? 1.5 : 1;
    ctx.stroke();
    ctx.setLineDash([]);

    ctx.save();
    roundedRect(ctx, x, y, w, h, CORNER_R + 2);
    ctx.clip();
    ctx.fillStyle = withAlpha(c.text, "24");
    ctx.fillRect(x, y, w, FRAME_HEADER);
    ctx.restore();

    const title = String(this.data.config["text"] ?? "").replace(/\s*\n\s*/g, " ").trim();
    ctx.font = `600 ${this.noteMetrics.title}px ${NOTE_FAMILY}`;
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    ctx.fillStyle = title ? (c.text === colors.textSecondary ? colors.textPrimary : c.text) : withAlpha(c.text, "99");
    ctx.fillText(fitText(ctx, title || "Group title", w - 24), x + 12, y + FRAME_HEADER / 2 + 1);
  }

  // ── Serialization ──────────────────────────────────────────────────────────

  toWorkflowNode(): object {
    return {
      id:            this.data.id,
      node_type_id:  this.data.node_type_id,
      node_type:     this.data.node_type,
      name:          this.data.name,
      config:        this.data.config,
      credentials:   this.data.credentials,
      ports:         this.data.ports,
      input_schema:  this.data.input_schema,
      output_schema: this.data.output_schema,
      retry:         this.data.retry,
      fallback_node: this.data.fallback_node,
      disabled:      this.data.disabled ?? false,
      position:      this.data.position,
      dynamic_ports: this.data.dynamic_ports,
    };
  }
}

let _counter = 0;

export function createNodeFromDescriptor(
  descriptor: NodeDescriptor,
  x: number,
  y: number
): CanvasNode {
  return new CanvasNode({
    id:            `node_${Date.now()}_${_counter++}`,
    node_type_id:  descriptor.type_id,
    node_type:     descriptor.node_type,
    name:          descriptor.display_name,
    config:        {},
    credentials:   {},
    position:      { x, y },
    ports:         descriptor.ports,
    input_schema:  descriptor.input_schema as Record<string, unknown>,
    output_schema: descriptor.output_schema as Record<string, unknown>,
    retry:         { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: descriptor.dynamic_ports ?? false,
  });
}

function trunc(s: string, max: number): string {
  return s.length > max ? s.slice(0, max - 1) + "…" : s;
}

/**
 * Small filled warning-triangle glyph, centered at (cx, cy). Drawn immediately
 * left of the category label for node types in DANGEROUS_NODE_IDS. Cosmetic
 * only — draws a marker, never blocks or warns on its own (the real gate is
 * aerini_engine::nodes::DANGEROUS_NODE_TYPE_IDS).
 */
function drawDangerBadge(ctx: CanvasRenderingContext2D, cx: number, cy: number): void {
  const s = 5;
  const colors = getCanvasColors();
  ctx.save();
  ctx.beginPath();
  ctx.moveTo(cx, cy - s);
  ctx.lineTo(cx - s, cy + s * 0.8);
  ctx.lineTo(cx + s, cy + s * 0.8);
  ctx.closePath();
  ctx.fillStyle = colors.error;
  ctx.fill();

  ctx.font         = "bold 7px -apple-system, BlinkMacSystemFont, sans-serif";
  ctx.fillStyle    = colors.surface1; // node body color — contrasts against the fill, any theme
  ctx.textAlign    = "center";
  ctx.textBaseline = "middle";
  ctx.fillText("!", cx, cy + s * 0.15);
  ctx.restore();
}

function drawTypeBadge(ctx: CanvasRenderingContext2D, cx: number, cy: number, glyph: string): void {
  const r = 5;
  const colors = getCanvasColors();
  ctx.save();
  ctx.beginPath();
  ctx.arc(cx, cy, r, 0, Math.PI * 2);
  ctx.fillStyle = colors.surface2;
  ctx.fill();
  ctx.strokeStyle = colors.textSecondary;
  ctx.lineWidth   = 1.25;
  ctx.stroke();

  ctx.font         = "bold 7px -apple-system, BlinkMacSystemFont, sans-serif";
  ctx.fillStyle    = colors.textSecondary;
  ctx.textAlign    = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(glyph, cx, cy + 0.5);
  ctx.restore();
}

export interface CornerRadii { tl: number; tr: number; br: number; bl: number }

export function roundedRect(
  ctx: CanvasRenderingContext2D,
  x: number, y: number, w: number, h: number, r: number | CornerRadii
): void {
  const { tl, tr, br, bl } = typeof r === "number" ? { tl: r, tr: r, br: r, bl: r } : r;
  ctx.beginPath();
  ctx.moveTo(x + tl, y);
  ctx.arcTo(x + w, y,     x + w, y + h, tr);
  ctx.arcTo(x + w, y + h, x,     y + h, br);
  ctx.arcTo(x,     y + h, x,     y,     bl);
  ctx.arcTo(x,     y,     x + w, y,     tl);
  ctx.closePath();
}

 
function drawCornerBrackets(
  ctx: CanvasRenderingContext2D,
  x: number, y: number, w: number, h: number,
  color: string, lineWidth: number
): void {
  const ARM = 10; // length of each corner bracket arm
  const OFF = 3;  // how far the brackets sit outside the node edge

  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth   = lineWidth;
  ctx.lineCap     = "butt";

  // [cornerX, cornerY, armDirX, armDirY] — arms extend from the corner back
  // toward the node's own edges (dir = +1) or away from them (dir = -1).
  const corners: Array<[number, number, number, number]> = [
    [x,     y,     1,  1], // top-left
    [x + w, y,    -1,  1], // top-right
    [x,     y + h, 1, -1], // bottom-left
    [x + w, y + h,-1, -1], // bottom-right
  ];

  for (const [cx, cy, dx, dy] of corners) {
    const ox = cx - dx * OFF;
    const oy = cy - dy * OFF;
    ctx.beginPath();
    ctx.moveTo(ox + dx * ARM, oy);
    ctx.lineTo(ox, oy);
    ctx.lineTo(ox, oy + dy * ARM);
    ctx.stroke();
  }

  ctx.restore();
}

let measureCtx: CanvasRenderingContext2D | null | undefined;

function noteFont(m: NoteMetrics, bold: boolean, heading: boolean): string {
  if (heading) return `700 ${m.heading}px ${NOTE_FAMILY}`;
  return `${bold ? "600 " : ""}${m.body}px ${NOTE_FAMILY}`;
}

function measureNote(text: string, bold: boolean, heading: boolean, m: NoteMetrics): number {
  if (measureCtx === undefined) {
    try { measureCtx = document.createElement("canvas").getContext("2d"); } catch { measureCtx = null; }
  }
  if (!measureCtx) return text.length * (heading ? 8 : bold ? 7 : 6.5) * (m.body / 12);
  measureCtx.font = noteFont(m, bold, heading);
  return measureCtx.measureText(text).width;
}

function fitText(ctx: CanvasRenderingContext2D, text: string, maxW: number): string {
  if (ctx.measureText(text).width <= maxW) return text;
  let t = text;
  while (t.length > 1 && ctx.measureText(t + "…").width > maxW) t = t.slice(0, -1);
  return t + "…";
}

function drawCheckbox(ctx: CanvasRenderingContext2D, x: number, y: number, size: number, checked: boolean, color: string): void {
  ctx.save();
  ctx.translate(x, y);
  ctx.scale(size / BOX_REF, size / BOX_REF);
  roundedRect(ctx, 0, 0, BOX_REF, BOX_REF, 3);
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.25;
  ctx.stroke();
  if (checked) {
    ctx.beginPath();
    ctx.moveTo(2.5, 6.5);
    ctx.lineTo(5, 9);
    ctx.lineTo(9.5, 3.5);
    ctx.stroke();
  }
  ctx.restore();
}

function drawResizeHandles(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, stroke: string, fill: string): void {
  const pts: Array<[number, number]> = [
    [x, y], [x + w / 2, y], [x + w, y], [x, y + h / 2], [x + w, y + h / 2], [x, y + h], [x + w / 2, y + h], [x + w, y + h],
  ];
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = stroke;
  ctx.fillStyle = fill;
  for (const [px, py] of pts) {
    ctx.beginPath();
    ctx.rect(px - 3.5, py - 3.5, 7, 7);
    ctx.fill();
    ctx.stroke();
  }
}
