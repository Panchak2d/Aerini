import type { CanvasNode } from "./Node";
import { getCanvasColors } from "./theme-colors";

export interface ConnectorData {
  id: string;
  from_node: string;
  from_port: string;
  to_node: string;
  to_port: string;
  condition: string | null;
  on_success: string | null;
  on_failure: string | null;
}

export class Connector {
  data:        ConnectorData;
  selected     = false;
  hovered      = false;
  highlighted  = false; // cut preview
  active       = false; // execution flow

  // Flow animation phase (0–1, driven by Canvas dt)
  flowPhase    = 0;

  constructor(data: ConnectorData) { this.data = data; }

  draw(ctx: CanvasRenderingContext2D, nodes: Map<string, CanvasNode>, dt = 0): void {
    const fn = nodes.get(this.data.from_node);
    const tn = nodes.get(this.data.to_node);
    if (!fn || !tn) return;

    const fp = fn.ports.find(p => p.id === this.data.from_port && !p.isInput);
    const tp = tn.ports.find(p => p.id === this.data.to_port  &&  p.isInput);
    if (!fp || !tp) return;

    if (this.active) this.flowPhase = (this.flowPhase + dt * 0.6) % 1;
    else this.flowPhase = 0;

    this.drawCurve(ctx, fp.x, fp.y, tp.x, tp.y);

    // Draw success/failure branch labels at wire midpoint
    if (this.data.on_success !== null || this.data.on_failure !== null) {
      const colors = getCanvasColors();
      const dx = Math.abs(tp.x - fp.x);
      const cp = Math.max(dx * 0.55, 80);
      const mx = bez(fp.x, fp.x + cp, tp.x - cp, tp.x, 0.5);
      const my = bez(fp.y, fp.y, tp.y, tp.y, 0.5);

      const label = this.data.on_success !== null ? "\u2713" : "\u2717";
      const bg    = this.data.on_success !== null ? colors.actionRun : colors.error;

      ctx.save();
      ctx.font         = "bold 9px -apple-system, BlinkMacSystemFont, sans-serif";
      ctx.textAlign    = "center";
      ctx.textBaseline = "middle";
      const tw = ctx.measureText(label).width;
      const pw = tw + 8;
      const ph = 14;
      const rx = 5;
      const bx = mx - pw / 2;
      const by = my - ph / 2;

      ctx.beginPath();
      ctx.moveTo(bx + rx, by);
      ctx.lineTo(bx + pw - rx, by);
      ctx.quadraticCurveTo(bx + pw, by, bx + pw, by + rx);
      ctx.lineTo(bx + pw, by + ph - rx);
      ctx.quadraticCurveTo(bx + pw, by + ph, bx + pw - rx, by + ph);
      ctx.lineTo(bx + rx, by + ph);
      ctx.quadraticCurveTo(bx, by + ph, bx, by + ph - rx);
      ctx.lineTo(bx, by + rx);
      ctx.quadraticCurveTo(bx, by, bx + rx, by);
      ctx.closePath();
      ctx.fillStyle = bg;
      ctx.fill();
      ctx.fillStyle = "#ffffff";
      ctx.fillText(label, mx, my);
      ctx.restore();
    }
  }

  drawCurve(
    ctx: CanvasRenderingContext2D,
    x1: number, y1: number,
    x2: number, y2: number,
    overrideColor?: string
  ): void {
    const dx = Math.abs(x2 - x1);
    const cp = Math.max(dx * 0.55, 80);

    // ── Main curve ──────────────────────────────────────────────────────────
    const colors = getCanvasColors();
    let color: string;
    if (overrideColor)         color = overrideColor;
    else if (this.highlighted) color = colors.error;
    else if (this.selected)    color = colors.actionNav;
    else if (this.active)      color = colors.actionRun;
    else if (this.hovered)     color = colors.wireHover;
    else                       color = colors.wire;

    ctx.save();
    ctx.beginPath();
    ctx.moveTo(x1, y1);
    ctx.bezierCurveTo(x1 + cp, y1, x2 - cp, y2, x2, y2);
    ctx.strokeStyle = color;
    ctx.lineWidth   = this.selected || this.active ? 2 : 1.5;
    ctx.lineCap     = "round";
    // Resting-state wires read as schematic/blueprint (dashed), matching the
    // canvas's own dot-grid background. Active flow (moving glow-dot) and an
    // explicit selection/cut-highlight already carry their own strong solid
    // signal, so those stay solid rather than competing with the dash.
    // Subsumes the old condition-only dash special case — a conditional
    // edge is resting exactly as often as any other edge, so it no longer
    // needs a separate check to end up dashed.
    if (!this.active && !this.selected && !this.highlighted) ctx.setLineDash([5, 4]);
    ctx.stroke();
    ctx.setLineDash([]);

    // ── Flow pulse (active execution) ───────────────────────────────────────
    if (this.active) {
      const p = this.flowPhase;
      // Draw a moving glow dot along the bezier
      const bx = bez(x1, x1+cp, x2-cp, x2, p);
      const by = bez(y1, y1,    y2,    y2, p);

      // Glow
      const g = ctx.createRadialGradient(bx, by, 0, bx, by, 10);
      g.addColorStop(0, colors.actionRun + "cc");
      g.addColorStop(1, "transparent");
      ctx.beginPath();
      ctx.arc(bx, by, 10, 0, Math.PI * 2);
      ctx.fillStyle = g;
      ctx.fill();

      // Dot
      ctx.beginPath();
      ctx.arc(bx, by, 3, 0, Math.PI * 2);
      ctx.fillStyle = colors.actionRun;
      ctx.fill();
    }

    // ── Arrowhead at destination ────────────────────────────────────────────
    const arrowColor = this.selected ? colors.actionNav : this.active ? colors.actionRun : color;
    this.drawArrow(ctx, x1, y1, x2, y2, cp, arrowColor);

    ctx.restore();
  }

  private drawArrow(
    ctx: CanvasRenderingContext2D,
    x1: number, y1: number,
    x2: number, y2: number,
    cp: number,
    color: string
  ): void {
    const t     = 0.985;
    const prevX = bez(x1, x1+cp, x2-cp, x2, t);
    const prevY = bez(y1, y1,    y2,    y2, t);
    const angle = Math.atan2(y2 - prevY, x2 - prevX);
    const size  = 6;

    ctx.save();
    ctx.translate(x2, y2);
    ctx.rotate(angle);
    ctx.beginPath();
    ctx.moveTo(0, 0);
    ctx.lineTo(-size, -size * 0.5);
    ctx.lineTo(-size * 0.65, 0);
    ctx.lineTo(-size,  size * 0.5);
    ctx.closePath();
    ctx.fillStyle = color;
    ctx.setLineDash([]);
    ctx.fill();
    ctx.restore();
  }

  containsPoint(wx: number, wy: number, nodes: Map<string, CanvasNode>, tol = 8): boolean {
    const fn = nodes.get(this.data.from_node);
    const tn = nodes.get(this.data.to_node);
    if (!fn || !tn) return false;
    const fp = fn.ports.find(p => p.id === this.data.from_port && !p.isInput);
    const tp = tn.ports.find(p => p.id === this.data.to_port  &&  p.isInput);
    if (!fp || !tp) return false;
    const cp = Math.max(Math.abs(tp.x - fp.x) * 0.55, 80);
    for (let t = 0; t <= 1; t += 0.04) {
      if (Math.hypot(wx - bez(fp.x,fp.x+cp,tp.x-cp,tp.x,t),
                     wy - bez(fp.y,fp.y,   tp.y,   tp.y,t)) < tol) return true;
    }
    return false;
  }

  toWorkflowEdge(): object {
    return {
      id:         this.data.id,
      from_node:  this.data.from_node,
      from_port:  this.data.from_port,
      to_node:    this.data.to_node,
      to_port:    this.data.to_port,
      condition:  this.data.condition,
      on_success: this.data.on_success,
      on_failure: this.data.on_failure,
    };
  }
}

// ── Pending connector while dragging ─────────────────────────────────────────

export class PendingConnector {
  fromNode: string;
  fromPort: string;
  fromX: number; fromY: number;
  toX:   number; toY:   number;

  constructor(fromNode: string, fromPort: string, x: number, y: number) {
    this.fromNode = fromNode; this.fromPort = fromPort;
    this.fromX = x; this.fromY = y; this.toX = x; this.toY = y;
  }

  draw(ctx: CanvasRenderingContext2D): void {
    const dx = Math.abs(this.toX - this.fromX);
    const cp = Math.max(dx * 0.55, 80);
    ctx.save();
    ctx.beginPath();
    ctx.moveTo(this.fromX, this.fromY);
    ctx.bezierCurveTo(this.fromX+cp, this.fromY, this.toX-cp, this.toY, this.toX, this.toY);
    ctx.strokeStyle = getCanvasColors().actionNav;
    ctx.lineWidth   = 1.5;
    ctx.setLineDash([5, 4]);
    ctx.stroke();
    ctx.restore();
  }
}

let _ec = 0;
export function newConnectorId(): string { return `e_${Date.now()}_${_ec++}`; }

function bez(p0:number,p1:number,p2:number,p3:number,t:number):number {
  const m=1-t; return m*m*m*p0+3*m*m*t*p1+3*m*t*t*p2+t*t*t*p3;
}
