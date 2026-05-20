import type { NodeDescriptor, PortDefinition } from "../ipc/workflow";
import { NODE_ICONS } from "../utils";

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

const TYPE_META: Record<string, {
  accent: string;
  dim:    string;
  icon:   string;
  label:  string;
}> = {
  action:  { accent: "#4d9eff", dim: "#0f1f35", icon: "A",  label: "ACTION"  },
  ai:      { accent: "#a78bfa", dim: "#1a1035", icon: "AI", label: "AI"      },
  logic:   { accent: "#34d399", dim: "#0b2820", icon: "L",  label: "LOGIC"   },
  utility: { accent: "#f59e0b", dim: "#211600", icon: "U",  label: "UTILITY" },
};

const STATUS_COLOR = {
  idle:    null,
  running: "#f59e0b",
  success: "#34d399",
  error:   "#f87171",
};

// Note color palette for note-type nodes
const NOTE_COLORS: Record<string, { bg: string; border: string; text: string }> = {
  default: { bg: "#1c2128", border: "#30363d",  text: "#8b949e" },
  yellow:  { bg: "#2a2200", border: "#f59e0b44", text: "#f59e0b" },
  blue:    { bg: "#0f1f35", border: "#4d9eff44", text: "#4d9eff" },
  green:   { bg: "#0b2820", border: "#34d39944", text: "#34d399" },
  red:     { bg: "#2a0a0a", border: "#f8717144", text: "#f87171" },
};

export class CanvasNode {
  data: CanvasNodeData;
  ports: Port[] = [];
  selected  = false;
  hovered   = false;
  disabled  = false;
  status: "idle" | "running" | "success" | "error" = "idle";

  // Output preview (set after execution)
  outputPreview: string | null = null;
  showOutput    = false;

  // Running animation phase (0–1, loops)
  private _runPhase = 0;

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
        id:       s.id,
        label:    s.name,
        position: "left" as const,
      }));
    } else {
      // No slots configured yet — show a single placeholder input
      this.data.ports.inputs = [{ id: "input", label: "In", position: "left" as const }];
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

  draw(ctx: CanvasRenderingContext2D, dt = 0): void {
    // Note node gets its own minimal sticky-note rendering
    if (this.data.node_type_id === "note") {
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
    const meta = TYPE_META[this.data.node_type] ?? TYPE_META.utility;
    const accent = STATUS_COLOR[this.status] ?? meta.accent;

    ctx.save();
    if (this.disabled) ctx.globalAlpha = 0.35;

    // ── Selection / hover outer ring ────────────────────────────────────────
    if (this.selected) {
      ctx.beginPath();
      roundedRect(ctx, x - 3, y - 3, w + 6, h + 6, r + 3);
      ctx.strokeStyle = accent + "88";
      ctx.lineWidth   = 2;
      ctx.stroke();
    } else if (this.hovered) {
      ctx.beginPath();
      roundedRect(ctx, x - 2, y - 2, w + 4, h + 4, r + 2);
      ctx.strokeStyle = "#ffffff18";
      ctx.lineWidth   = 1.5;
      ctx.stroke();
    }

    // ── Node body ───────────────────────────────────────────────────────────
    ctx.beginPath();
    roundedRect(ctx, x, y, w, h, r);
    ctx.fillStyle = "#161b22";
    ctx.fill();

    // ── Left accent stripe ──────────────────────────────────────────────────
    ctx.save();
    ctx.beginPath();
    roundedRect(ctx, x, y, w, h, r);
    ctx.clip();
    ctx.fillStyle = accent;
    ctx.fillRect(x, y, 3, h);
    ctx.restore();

    // ── Header background ───────────────────────────────────────────────────
    ctx.save();
    ctx.beginPath();
    roundedRect(ctx, x, y, w, NODE_HEADER, r);
    ctx.rect(x, y + r, w, NODE_HEADER - r); // make bottom flat
    ctx.fillStyle = meta.dim;
    ctx.fill();
    ctx.restore();

    // ── Header separator ───────────────────────────────────────────────────
    ctx.beginPath();
    ctx.moveTo(x + 3, y + NODE_HEADER);
    ctx.lineTo(x + w, y + NODE_HEADER);
    ctx.strokeStyle = "#ffffff0f";
    ctx.lineWidth   = 1;
    ctx.stroke();

    // ── Border ─────────────────────────────────────────────────────────────
    ctx.beginPath();
    roundedRect(ctx, x, y, w, h, r);
    ctx.strokeStyle = this.selected
      ? accent
      : this.hovered
      ? "#ffffff22"
      : "#ffffff0f";
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

    // ── Node icon ──────────────────────────────────────────────────────────
    const icon = NODE_ICONS[this.data.node_type_id] ?? meta.icon;
    ctx.font         = "bold 13px monospace";
    ctx.fillStyle    = accent;
    ctx.textBaseline = "middle";
    ctx.textAlign    = "left";
    ctx.fillText(icon, x + 14, y + NODE_HEADER / 2);

    // ── Node name ──────────────────────────────────────────────────────────
    ctx.font         = "500 12px -apple-system, BlinkMacSystemFont, 'SF Pro Text', sans-serif";
    ctx.fillStyle    = "#e6edf3";
    ctx.textBaseline = "middle";
    ctx.textAlign    = "left";
    ctx.fillText(trunc(this.data.name, 20), x + 34, y + NODE_HEADER / 2);

    // ── Category label (top right) ─────────────────────────────────────────
    ctx.font         = "500 8px -apple-system, BlinkMacSystemFont, sans-serif";
    ctx.fillStyle    = accent + "99";
    ctx.textBaseline = "middle";
    ctx.textAlign    = "right";
    ctx.fillText(meta.label, x + w - (this.status !== "idle" ? 20 : 8), y + NODE_HEADER / 2);

    // ── Disabled overlay label ─────────────────────────────────────────────
    if (this.disabled) {
      ctx.font      = "700 9px -apple-system, BlinkMacSystemFont, sans-serif";
      ctx.fillStyle = "#f87171cc";
      ctx.textAlign = "center";
      ctx.fillText("DISABLED", x + w / 2, y + NODE_HEADER / 2);
    }

    // ── Ports ──────────────────────────────────────────────────────────────
    ctx.font         = "11px -apple-system, BlinkMacSystemFont, sans-serif";
    ctx.textBaseline = "middle";

    for (const port of this.ports) {
      // Port outer ring
      ctx.beginPath();
      ctx.arc(port.x, port.y, PORT_RADIUS + 2, 0, Math.PI * 2);
      ctx.fillStyle = "#161b22";
      ctx.fill();

      // Port fill
      ctx.beginPath();
      ctx.arc(port.x, port.y, PORT_RADIUS, 0, Math.PI * 2);
      ctx.fillStyle   = "#1c2128";
      ctx.strokeStyle = accent + "cc";
      ctx.lineWidth   = 1.5;
      ctx.fill();
      ctx.stroke();

      // Port dot (filled if has connection — visual only hint)
      ctx.beginPath();
      ctx.arc(port.x, port.y, 2.5, 0, Math.PI * 2);
      ctx.fillStyle = accent + "66";
      ctx.fill();

      // Port label
      ctx.fillStyle = "#8b949e";
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
      ctx.strokeStyle = "#ffffff0a";
      ctx.lineWidth   = 1;
      ctx.stroke();

      // Preview text
      ctx.font         = "11px 'SF Mono', 'Fira Code', monospace";
      ctx.fillStyle    = "#34d39988";
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

    const c = NOTE_COLORS[color] ?? NOTE_COLORS.default;

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

function wrapText(ctx: CanvasRenderingContext2D, text: string, maxWidth: number): string[] {
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
