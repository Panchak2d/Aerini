/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { InputHandler } from "../canvas/InputHandler";
import { UndoManager } from "../canvas/UndoManager";
import { Connector } from "../canvas/Connector";
import { CanvasNode } from "../canvas/Node";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function node(id: string, typeId: string, x: number, inputs: string[], outputs: string[]): CanvasNode {
  return new CanvasNode({
    id, node_type_id: typeId, node_type: "action", name: id, config: {}, credentials: {},
    position: { x, y: 0 },
    ports: {
      inputs:  inputs.map(i => ({ id: i, label: i, position: "left" as const })),
      outputs: outputs.map(o => ({ id: o, label: o, position: "right" as const })),
    },
    input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: false,
  });
}

function wire(id: string, from: string, to: string, toPort: string): Connector {
  return new Connector({ id, from_node: from, from_port: "output", to_node: to, to_port: toPort, condition: "ok", on_success: null, on_failure: null });
}

function makeCanvas() {
  const canvas = Object.create(Canvas.prototype) as Canvas & Record<string, unknown>;
  const hook = node("hook", "webhook", 0, [], ["output"]);
  const mem  = node("mem", "ai_memory", 400, ["input"], ["output"]);
  const ai   = node("ai", "ai_prompt", 800, ["input", "attachments"], ["output"]);
  canvas.nodes = new Map([["hook", hook], ["mem", mem], ["ai", ai]]);
  canvas.connectors = new Map();
  canvas.selectedNodes = new Set();
  canvas.selectedNode = null;
  canvas.selectedConn = null;
  canvas.el = { classList: { add: vi.fn(), remove: vi.fn() }, style: { cursor: "" }, getBoundingClientRect: () => ({ left: 0, top: 0 }) } as unknown as HTMLCanvasElement;
  canvas.zoom = 1; canvas.panX = 0; canvas.panY = 0;
  canvas.onCanvasChanged = vi.fn();
  canvas.onWarn = vi.fn();
  canvas.onInputWireDropRequest = vi.fn();
  canvas.undoMgr = new UndoManager(canvas);
  canvas.input = new InputHandler(canvas);
  return { canvas, hook, mem, ai };
}

const portOf = (n: CanvasNode, id: string) => n.ports.find(p => p.id === id)!;
const mouse = (x: number, y: number) => ({ clientX: x, clientY: y, button: 0, altKey: false, shiftKey: false, preventDefault: vi.fn() }) as unknown as MouseEvent;
const undoDepth = (c: Canvas) => (c.undoMgr as unknown as { stack: unknown[] }).stack.length;

describe("wire move gesture", () => {
  it("pressing a port picks nothing up and a click on a wired input only selects its wire", () => {
    const { canvas, hook, ai } = makeCanvas();
    const w1 = wire("w1", "hook", "ai", "input");
    canvas.connectors.set("w1", w1);

    const out = portOf(hook, "output");
    canvas.input.onDown(mouse(out.x, out.y));
    expect(canvas.input.reconnEdge).toBeNull();
    expect(canvas.input.pendingConn).toBeNull();
    canvas.input.onWinUp(mouse(out.x, out.y));

    const inp = portOf(ai, "input");
    canvas.input.onDown(mouse(inp.x, inp.y));
    expect(canvas.input.reconnEdge).toBeNull();
    expect(canvas.input.pendingConn).toBeNull();
    canvas.input.onWinUp(mouse(inp.x, inp.y));

    expect(canvas.connectors.get("w1")).toBe(w1);
    expect(canvas.selectedConn).toBe(w1);
    expect(canvas.onInputWireDropRequest).not.toHaveBeenCalled();
    expect(undoDepth(canvas)).toBe(0);
  });

  it("dragging the handle of a selected wire keeps the original in place until a valid drop", () => {
    const { canvas, ai } = makeCanvas();
    const w1 = wire("w1", "hook", "ai", "input");
    canvas.connectors.set("w1", w1);
    canvas.selectedConn = w1;

    const inp = portOf(ai, "input");
    canvas.input.onDown(mouse(inp.x, inp.y));
    expect(canvas.input.reconnEdge).toBeNull();
    canvas.input.onWinMove(mouse(inp.x - 30, inp.y));
    expect(canvas.input.reconnEdge?.conn).toBe(w1);
    expect(canvas.connectors.get("w1")).toBe(w1);

    canvas.input.onKey(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(canvas.input.reconnEdge).toBeNull();
    expect(canvas.input.pendingConn).toBeNull();
    expect(canvas.connectors.get("w1")).toBe(w1);
    expect(w1.data.to_port).toBe("input");
    expect(undoDepth(canvas)).toBe(0);
  });

  it("dropping a dragged wire on empty canvas leaves it untouched, without an undo entry", () => {
    const { canvas, ai } = makeCanvas();
    const w1 = wire("w1", "hook", "ai", "input");
    canvas.connectors.set("w1", w1);
    canvas.selectedConn = w1;
    const inp = portOf(ai, "input");
    canvas.input.onDown(mouse(inp.x, inp.y));
    canvas.input.onWinMove(mouse(inp.x - 30, inp.y));

    canvas.input.onWinUp(mouse(5000, 5000));

    expect(canvas.connectors.get("w1")).toBe(w1);
    expect(w1.data.to_port).toBe("input");
    expect(undoDepth(canvas)).toBe(0);
    expect(canvas.input.reconnEdge).toBeNull();
  });
});

describe("Canvas.rerouteConn", () => {
  it("moves the wire to the new port, keeps its id and condition, and records exactly one undo step", () => {
    const { canvas, ai } = makeCanvas();
    const w1 = wire("w1", "hook", "ai", "input");
    canvas.connectors.set("w1", w1);

    expect(canvas.rerouteConn(w1, { nodeId: "ai", portId: "attachments" })).toBe(true);

    expect(canvas.connectors.size).toBe(1);
    expect(w1.data).toMatchObject({ id: "w1", to_node: "ai", to_port: "attachments", condition: "ok" });
    expect((ai.data.config as Record<string, unknown>).attachments_expr).toBe("{{hook.output}}");
    expect(undoDepth(canvas)).toBe(1);
  });

  it("replaces the wire on an occupied single-arity target, recorded in the same undo step", () => {
    const { canvas } = makeCanvas();
    const w1 = wire("w1", "hook", "ai", "input");
    const w2 = wire("w2", "mem", "ai", "attachments");
    canvas.connectors.set("w1", w1); canvas.connectors.set("w2", w2);

    expect(canvas.rerouteConn(w1, { nodeId: "ai", portId: "attachments" })).toBe(true);

    expect(canvas.connectors.has("w2")).toBe(false);
    expect(w1.data.to_port).toBe("attachments");
    expect(canvas.onWarn).toHaveBeenCalledWith(expect.stringContaining('"mem"'));
    expect(undoDepth(canvas)).toBe(1);

    canvas.undo();
    expect(w1.data.to_port).toBe("input");
    expect(canvas.connectors.get("w2")).toBe(w2);

    canvas.redo();
    expect(w1.data.to_port).toBe("attachments");
    expect(canvas.connectors.has("w2")).toBe(false);
  });

  it("is a no-op for no target, the current port, or the wire's own source node", () => {
    const { canvas } = makeCanvas();
    const w1 = wire("w1", "mem", "ai", "input");
    canvas.connectors.set("w1", w1);

    expect(canvas.rerouteConn(w1, null)).toBe(false);
    expect(canvas.rerouteConn(w1, { nodeId: "ai", portId: "input" })).toBe(false);
    expect(canvas.rerouteConn(w1, { nodeId: "mem", portId: "input" })).toBe(false);
    expect(undoDepth(canvas)).toBe(0);
  });

  it("one undo restores the original wire and its config exactly; redo reapplies", () => {
    const { canvas, ai } = makeCanvas();
    const cfg = ai.data.config as Record<string, unknown>;
    cfg.attachments_expr = "{{custom.output.files}}";
    cfg.prompt = "keep me";
    const w1 = wire("w1", "hook", "ai", "attachments");
    canvas.connectors.set("w1", w1);

    canvas.rerouteConn(w1, { nodeId: "ai", portId: "input" });
    expect(cfg.attachments_expr).toBe("");

    cfg.prompt = "edited after the move";
    canvas.undo();
    expect(w1.data).toMatchObject({ to_node: "ai", to_port: "attachments" });
    expect(cfg.attachments_expr).toBe("{{custom.output.files}}");
    expect(cfg.prompt).toBe("edited after the move");

    canvas.redo();
    expect(w1.data.to_port).toBe("input");
    expect(cfg.attachments_expr).toBe("");
  });
});
