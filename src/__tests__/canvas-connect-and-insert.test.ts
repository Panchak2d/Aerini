/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { InputHandler } from "../canvas/InputHandler";
import { UndoManager } from "../canvas/UndoManager";
import { CanvasNode } from "../canvas/Node";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), convertFileSrc: vi.fn((p: string) => p) }));

const PI = "x-aerini-primary-input";
const PO = "x-aerini-primary-output";

function node(id: string, name: string, x: number, inSchema: Record<string, unknown> = {}, outSchema: Record<string, unknown> = {}): CanvasNode {
  return new CanvasNode({
    id, node_type_id: "action_x", node_type: "action", name, config: {}, credentials: {},
    position: { x, y: 0 },
    ports: {
      inputs: [{ id: "input", label: "input", position: "left" as const }],
      outputs: [{ id: "output", label: "output", position: "right" as const }, { id: "on_error", label: "on_error", position: "right" as const }],
    },
    input_schema: inSchema, output_schema: outSchema,
    retry: { max_attempts: 1, backoff_ms: 500 }, fallback_node: null, dynamic_ports: false,
  });
}

function makeCanvas() {
  const canvas = Object.create(Canvas.prototype) as Canvas & Record<string, unknown>;
  canvas.nodes = new Map();
  canvas.connectors = new Map();
  canvas.selectedNodes = new Set();
  canvas.selectedNode = null;
  canvas.selectedConn = null;
  canvas.el = {
    classList: { add: vi.fn(), remove: vi.fn() }, style: { cursor: "" },
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 1400, height: 900 }),
  } as unknown as HTMLCanvasElement;
  canvas.zoom = 1; canvas.panX = 0; canvas.panY = 0;
  canvas.snap = { avoidOverlap: vi.fn(), computeSnap: vi.fn(), snapGuides: [] } as never;
  canvas.onCanvasChanged = vi.fn();
  canvas.onWarn = vi.fn();
  canvas.undoMgr = new UndoManager(canvas);
  canvas.input = new InputHandler(canvas);
  return canvas;
}

const depth = (c: Canvas) => (c.undoMgr as unknown as { stack: unknown[] }).stack.length;
const warns = (c: Canvas) => (c.onWarn as unknown as ReturnType<typeof vi.fn>).mock.calls.map((a) => String(a[0]));

beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(1_000_000); localStorage.clear(); });
afterEach(() => { vi.useRealTimers(); });

function setup() {
  const c = makeCanvas();
  const src = node("src", "Fetch Data", 0, {}, { properties: { body: { type: "string", [PO]: true } } });
  const dst = node("dst", "Notify Me", 400, { properties: { title: { type: "string" }, body: { type: "string", [PI]: true } } });
  c.nodes.set("src", src);
  c.nodes.set("dst", dst);
  return { c, src, dst };
}

describe("connect and insert from the data picker", () => {
  it("wires_the_node_writes_the_field_and_undoes_as_one_step", () => {
    const { c, dst } = setup();
    const before = depth(c);
    const ok = c.connectAndInsert("src", "dst", () => { (dst.data.config as Record<string, unknown>).title = "News: {{Fetch Data.output.body}}"; });
    expect(ok).toBe(true);
    expect(c.connectors.size).toBe(1);
    expect(depth(c)).toBe(before + 1);
    expect(warns(c).some((w) => w.includes('Connected "Fetch Data" to "Notify Me"'))).toBe(true);

    c.undo();
    expect(c.connectors.size).toBe(0);
    expect((dst.data.config as Record<string, unknown>).title).toBeUndefined();
    expect((dst.data.config as Record<string, unknown>).body).toBeUndefined();
  });

  it("keeps_the_picked_field_text_when_the_wire_also_fills_the_primary_field", () => {
    const { c, dst } = setup();
    c.connectAndInsert("src", "dst", () => { (dst.data.config as Record<string, unknown>).body = "{{Fetch Data.output.body}}"; });
    const cfg = dst.data.config as Record<string, unknown>;
    expect(cfg.body).toBe("{{Fetch Data.output.body}}");
    expect(depth(c)).toBe(1);
  });

  it("refuses_a_node_downstream_of_the_target_and_changes_nothing", () => {
    const { c, dst } = setup();
    c.connectPorts("dst", "output", "src", "input");
    const before = depth(c);
    const apply = vi.fn();
    expect(c.connectableTo("dst")).toEqual([]);
    expect(c.connectAndInsert("src", "dst", apply)).toBe(false);
    expect(apply).not.toHaveBeenCalled();
    expect(c.connectors.size).toBe(1);
    expect(depth(c)).toBe(before);
    expect((dst.data.config as Record<string, unknown>).body).toBeUndefined();
  });

  it("offers_nothing_when_the_target_input_already_holds_a_wire_that_would_be_replaced", () => {
    const { c } = setup();
    c.nodes.set("other", node("other", "Other", 200));
    c.connectPorts("other", "output", "dst", "input");
    expect(c.connectableTo("dst")).toEqual([]);
  });
});
