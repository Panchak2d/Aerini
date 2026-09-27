import type { NodeDescriptor, PortDefinition } from "../ipc/workflow";
import { NODE_IDS, DANGEROUS_NODE_IDS, TRIGGER_NODE_IDS } from "../node-ids";
import { getIconBitmap } from "../icon-cache";
import { getCanvasColors, headerTint, resolveNoteColors } from "./theme-colors";
import { isPluginNodeType, isUnregisteredNodeType } from "./node-registry";

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
function typeMeta(nodeType: string): { accent: string; icon: string; label: string } {
  const key = (nodeType in TYPE_META_BASE ? nodeType : "utility") as Exclude<CatKey, "trigger">;
  const accent = getCanvasColors()[CAT_KEY_TO_COLORS_FIELD[key]];
  return { accent, ...TYPE_META_BASE[key] };
}


function statusAccent(status: CanvasNode["status"], colors: ReturnType<typeof getCanvasColors>): string | null {
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
  selected  = false;
  hovered   = false;
  status: "idle" | "running" | "success" | "error" = "idle";
  // True when this node has a required config field left empty after its
  // popover was closed. Cleared on a successful run (UX-7).
  missingRequired = false;

  // Output preview (set after execution)
  outputPreview: string | null = null;
  showOutput    = false;

  // Running animation phase (0–1, loops)
  private _runPhase = 0;

  // Backing field for `disabled`. The accessor below keeps `data.disabled`
  // in sync on every write, so ContextMenu's Disable/Enable toggle (which
  // only ever does `node.disabled = ...`) is automatically reflected in
  // toWorkflowNode()'s output with no separate sync step required.
  private _disabled = false;

  get disabled(): boolean { return this._disabled; }
  set disabled(v: boolean) {
    this._disabled = v;
    this.data.disabled = v;
  }

  get height(): number {
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
        id: p.id, label: p.label, isInput: false,
        x: x + NODE_WIDTH,
        y: y + NODE_HEADER + PORT_GAP * i + PORT_GAP / 2,
      });
    });
  }

  updatePortPositions(): void { this.rebuildPorts(); }

  containsPoint(wx: number, wy: number): boolean {
    const { x, y } = this.data.position;
    return wx >= x && wx <= x + NODE_WIDTH && wy >= y && wy <= y + this.height;
  }

  portAtPoint(wx: number, wy: number): Port | null {
    for (const p of this.ports) {
      if (Math.hypot(wx - p.x, wy - p.y) <= PORT_RADIUS + 6) return p;
    }
    return null;
  }

  draw(ctx: CanvasRenderingContext2D, dt = 0, connectedPorts: Set<string> = new Set()): void {
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

    // ── Trigger identity overlay (independent of NodeType/TYPE_META) ────────
    // TRIGGER_NODE_IDS is a plain id set, unrelated to the 4-value NodeType
    // union — schedule/webhook/manual_trigger are node_type "action" same as
    // everything else (verified against aerini-engine/src/nodes/{schedule,
    // webhook,manual_trigger}.rs and onboarding.ts's own fixture). This only
    // swaps the *identity* marks (left stripe + category label) to
    // --cat-trigger; it deliberately does not touch `accent` above, so ports,
    // selection, the running pulse, the status dot, and the node icon all stay
    // on the category/status accent, untouched. Two reasons: (1) that's what
    // "two independent systems, don't merge" means in practice, not just in
    // type signatures; (2) icon-cache.ts's bitmap cache is keyed by exact
    // color string and only preloads the 4 category colors + error red —
    // feeding it a 5th, unpreloaded color would silently degrade trigger
    // icons to the letter-fallback glyph, and icon-cache.ts is out of this
    // batch's file scope.
    const isTrigger      = TRIGGER_NODE_IDS.has(this.data.node_type_id);
    const identityAccent = isTrigger ? colors.catTrigger : meta.accent;
    const identityLabel  = isTrigger ? "TRIGGER" : meta.label;

    ctx.save();
    if (this.disabled) ctx.globalAlpha = 0.35;

    // ── Selection / hover outer indicator ────────────────────────────────────
  
    if (this.selected) {
      drawCornerBrackets(ctx, x, y, w, h, accent + "88", 2);
    } else if (this.hovered) {

      ctx.beginPath();
      roundedRect(ctx, x - 2, y - 2, w + 4, h + 4, r + 2);
      ctx.strokeStyle = colors.border + "b0";
      ctx.lineWidth   = 1.5;
      ctx.stroke();
    }

    // ── Node body ───────────────────────────────────────────────────────────
    ctx.beginPath();
    roundedRect(ctx, x, y, w, h, r);
    ctx.fillStyle = colors.surface1;
    ctx.fill();

    // ── Left accent stripe ──────────────────────────────────────────────────
    ctx.save();
    ctx.beginPath();
    roundedRect(ctx, x, y, w, h, r);
    ctx.clip();
    ctx.fillStyle = identityAccent;
    ctx.fillRect(x, y, 3, h);
    ctx.restore();

    // ── Header background ───────────────────────────────────────────────────
    // meta.accent (category-tied, not the trigger-swapped identityAccent —
    // same "two independent systems" split the stripe/label already keep,
    // see the trigger-overlay comment above) at low alpha over the opaque
    // body just filled. Was 4 hardcoded near-black hexes, theme-blind.
    ctx.save();
    ctx.beginPath();
    roundedRect(ctx, x, y, w, NODE_HEADER, r);
    ctx.rect(x, y + r, w, NODE_HEADER - r); // make bottom flat
    ctx.fillStyle = headerTint(meta.accent);
    ctx.fill();
    ctx.restore();

    // ── Header separator ───────────────────────────────────────────────────
    ctx.beginPath();
    ctx.moveTo(x + 3, y + NODE_HEADER);
    ctx.lineTo(x + w, y + NODE_HEADER);
    ctx.strokeStyle = colors.border + "40";
    ctx.lineWidth   = 1;
    ctx.stroke();

    // ── Border ─────────────────────────────────────────────────────────────
    ctx.beginPath();
    roundedRect(ctx, x, y, w, h, r);
    ctx.strokeStyle = this.selected
      ? accent
      : this.hovered
      ? colors.border + "99"
      : colors.border + "40";
    ctx.lineWidth = this.selected ? 1.5 : 1;
    ctx.stroke();

    // ── Running animation: pulsing top border ──────────────────────────────
    if (this.status === "running") {
      const grad = ctx.createLinearGradient(x, y, x + w, y);
      const p    = this._runPhase;
      grad.addColorStop(Math.max(0, p - 0.3), "transparent");
      grad.addColorStop(p, accent);
      grad.addColorStop(Math.min(1, p + 0.3), "transparent");
      ctx.beginPath();
      ctx.moveTo(x + r, y);
      ctx.lineTo(x + w - r, y);
      ctx.strokeStyle = grad;
      ctx.lineWidth   = 2;
      ctx.stroke();
    }

    // ── Status dot (top-right) ─────────────────────────────────────────────
    if (this.status !== "idle") {
      const dotX = x + w - 10;
      const dotY = y + 10;
      ctx.beginPath();
      ctx.arc(dotX, dotY, 4, 0, Math.PI * 2);
      ctx.fillStyle = accent;
      ctx.fill();
      // Pulse ring for running
      if (this.status === "running") {
        const alpha = Math.sin(this._runPhase * Math.PI * 2) * 0.5 + 0.5;
        ctx.beginPath();
        ctx.arc(dotX, dotY, 4 + 4 * alpha, 0, Math.PI * 2);
        ctx.strokeStyle = accent + Math.round(alpha * 255).toString(16).padStart(2,"0");
        ctx.lineWidth   = 1;
        ctx.stroke();
      }
    }

    // ── Incomplete-config badge (top-left, idle only) ───────────────────────
    if (this.missingRequired && this.status === "idle") {
      const bx = x + 10;
      const by = y + 10;
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

    // ── Node icon ──────────────────────────────────────────────────────────
    const bmp = getIconBitmap(this.data.node_type_id, accent);
    if (bmp) {
      ctx.drawImage(bmp, x + 12, y + NODE_HEADER / 2 - 8, 16, 16);
    } else {
      ctx.font         = "bold 13px monospace";
      ctx.fillStyle    = accent;
      ctx.textBaseline = "middle";
      ctx.textAlign    = "left";
      ctx.fillText(meta.icon, x + 14, y + NODE_HEADER / 2);
    }

    // ── Plugin badge (corner of the icon, any status) ───────────────────────
    // Looked up by type_id against the same registry the palette/config panel
    // use (node-registry's registerNodeDescriptors) -- CanvasNodeData never
    // carries is_plugin itself, so this needs zero changes to node placement,
    // serialization, or the .aerini file format.
    if (isPluginNodeType(this.data.node_type_id)) {
      drawTypeBadge(ctx, x + 26, y + NODE_HEADER / 2 + 6, "P");
    } else if (isUnregisteredNodeType(this.data.node_type_id)) {
      drawTypeBadge(ctx, x + 26, y + NODE_HEADER / 2 + 6, "?");
    }

    // ── Node name ──────────────────────────────────────────────────────────
    ctx.font         = "500 12px -apple-system, BlinkMacSystemFont, 'SF Pro Text', sans-serif";
    ctx.fillStyle    = colors.textPrimary;
    ctx.textBaseline = "middle";
    ctx.textAlign    = "left";
    ctx.fillText(trunc(this.data.name, 20), x + 34, y + NODE_HEADER / 2);

    // ── Category label (top right) ─────────────────────────────────────────
    ctx.font         = "500 8px -apple-system, BlinkMacSystemFont, sans-serif";
    ctx.fillStyle    = identityAccent + "99";
    ctx.textBaseline = "middle";
    ctx.textAlign    = "right";
    const labelRightEdge = x + w - (this.status !== "idle" ? 20 : 8);
    const labelY          = y + NODE_HEADER / 2;
    ctx.fillText(identityLabel, labelRightEdge, labelY);

    // ── Danger badge (top right, immediately left of the category label) ────
    // Measures identityLabel (not meta.label) so the badge stays correctly
    // positioned if a node is ever both trigger and dangerous — no overlap
    // exists today (TRIGGER_NODE_IDS and DANGEROUS_NODE_IDS are disjoint,
    // verified against node-ids.ts), but the two sets are independently
    // maintained elsewhere, so this shouldn't rely on them staying that way.
    if (DANGEROUS_NODE_IDS.has(this.data.node_type_id)) {
      const labelWidth = ctx.measureText(identityLabel).width;
      drawDangerBadge(ctx, labelRightEdge - labelWidth - 9, labelY);
    }

    // ── Disabled overlay label ─────────────────────────────────────────────
    if (this.disabled) {
      ctx.font      = "700 9px -apple-system, BlinkMacSystemFont, sans-serif";
      ctx.fillStyle = colors.error + "cc";
      ctx.textAlign = "center";
      ctx.fillText("DISABLED", x + w / 2, y + NODE_HEADER / 2);
    }

    // ── Ports ──────────────────────────────────────────────────────────────
    ctx.font         = "11px -apple-system, BlinkMacSystemFont, sans-serif";
    ctx.textBaseline = "middle";

    for (const port of this.ports) {
      const isConnected = connectedPorts.has(`${this.data.id}:${port.id}`);

      // Port outer ring — matches the node body it's cut out of
      ctx.beginPath();
      ctx.arc(port.x, port.y, PORT_RADIUS + 2, 0, Math.PI * 2);
      ctx.fillStyle = colors.surface1;
      ctx.fill();

      // Port fill — solid when connected, hollow when empty
      ctx.beginPath();
      ctx.arc(port.x, port.y, PORT_RADIUS, 0, Math.PI * 2);
      ctx.fillStyle   = isConnected ? accent + "cc" : colors.surface2;
      ctx.strokeStyle = accent + "cc";
      ctx.lineWidth   = 1.5;
      ctx.fill();
      ctx.stroke();

      // Port dot — full opacity when connected, dim when empty
      ctx.beginPath();
      ctx.arc(port.x, port.y, 2.5, 0, Math.PI * 2);
      ctx.fillStyle = isConnected ? accent : accent + "33";
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
      ctx.strokeStyle = colors.border + "30";
      ctx.lineWidth   = 1;
      ctx.stroke();

      // Preview text
      ctx.font         = "11px 'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace";
      ctx.fillStyle    = colors.actionRun + "88";
      ctx.textBaseline = "middle";
      ctx.textAlign    = "left";
      ctx.fillText(trunc(this.outputPreview, 28), x + 10, previewY + 17);
    }

    ctx.restore();
  }

  // ── Note node renderer ────────────────────────────────────────────────────

  private drawNote(ctx: CanvasRenderingContext2D): void {
    const { x, y } = this.data.position;
    const w = NODE_WIDTH;
    const text = String(this.data.config["text"] ?? "Double-click to edit");
    const color = String(this.data.config["color"] ?? "default");

    const noteColors = resolveNoteColors(getCanvasColors());
    const c = noteColors[color] ?? noteColors.default;

    ctx.font = "12px -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif";
    const maxW = w - 24;
    const lines = wrapText(ctx, text, maxW);
    const lineH = 18;
    const h = Math.max(60, 28 + lines.length * lineH);

    ctx.save();

    // Selection ring
    if (this.selected) {
      ctx.beginPath();
      roundedRect(ctx, x - 2, y - 2, w + 4, h + 4, CORNER_R + 2);
      ctx.strokeStyle = c.border;
      ctx.lineWidth = 2;
      ctx.stroke();
    }

    // Background
    ctx.beginPath();
    roundedRect(ctx, x, y, w, h, CORNER_R);
    ctx.fillStyle = c.bg;
    ctx.fill();
    ctx.strokeStyle = c.border;
    ctx.lineWidth = 1;
    ctx.stroke();

    // Top accent strip
    ctx.beginPath();
    roundedRect(ctx, x, y, w, h, CORNER_R);
    ctx.clip();
    ctx.fillStyle = c.border;
    ctx.fillRect(x, y, w, 3);
    ctx.restore();

    // Text
    ctx.save();
    ctx.font = "12px -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif";
    ctx.fillStyle = c.text;
    ctx.textBaseline = "top";
    lines.forEach((line, i) => {
      ctx.fillText(line, x + 12, y + 12 + i * lineH);
    });
    ctx.restore();
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

export function roundedRect(
  ctx: CanvasRenderingContext2D,
  x: number, y: number, w: number, h: number, r: number
): void {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y,     x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x,     y + h, r);
  ctx.arcTo(x,     y + h, x,     y,     r);
  ctx.arcTo(x,     y,     x + w, y,     r);
  ctx.closePath();
}


 
function drawCornerBrackets(
  ctx: CanvasRenderingContext2D,
  x: number, y: number, w: number, h: number,
  color: string, lineWidth: number
): void {
  const ARM = 10; // arm length, matches the 10px scale of the reference mockup's corner boxes
  const OFF = 3;  // outward offset — matches the previous ring's -3 offset, for visual continuity

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

export function wrapText(ctx: CanvasRenderingContext2D, text: string, maxWidth: number): string[] {
  const lines: string[] = [];
  for (const paragraph of text.split("\n")) {
    const words = paragraph.split(" ");
    let line = "";
    for (const word of words) {
      const test = line ? `${line} ${word}` : word;
      if (ctx.measureText(test).width > maxWidth && line) {
        lines.push(line);
        line = word;
      } else {
        line = test;
      }
    }
    if (line) lines.push(line);
  }
  return lines.length ? lines : [""];
}
