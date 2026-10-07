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

function node(id: string, typeId: string, x: number, inputs: string[], outputs: string[], y = 0, multi: string[] = []): CanvasNode {
  return new CanvasNode({
    id, node_type_id: typeId, node_type: "action", name: id, config: {}, credentials: {},
    position: { x, y },
    ports: {
      inputs:  inputs.map(i => ({ id: i, label: i, position: "left" as const, ...(multi.includes(i) ? { arity: "multi" as const } : {}) })),
      outputs: outputs.map(o => ({ id: o, label: o, position: "right" as const })),
    },
    input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: false,
  });
}

function wire(id: string, from: string, fromPort: string, to: string, toPort: string): Connector {
  return new Connector({ id, from_node: from, from_port: fromPort, to_node: to, to_port: toPort, condition: null, on_success: null, on_failure: null });
}

function makeCanvas() {
  const canvas = Object.create(Canvas.prototype) as Canvas & Record<string, unknown>;
  const hook = node("hook", "webhook", 0, [], ["output"]);
  const memRead = node("mem_read", "ai_memory", 400, ["input"], ["output"]);
  const ai = node("ai", "ai_prompt", 800, ["input", "attachments"], ["output"]);
  canvas.nodes = new Map([["hook", hook], ["mem_read", memRead], ["ai", ai]]);
  canvas.connectors = new Map();
  canvas.selectedNodes = new Set();
  canvas.selectedNode = null;
  canvas.selectedConn = null;
  canvas.el = { classList: { add: vi.fn(), remove: vi.fn() }, style: { cursor: "" }, getBoundingClientRect: () => ({ left: 0, top: 0 }) } as unknown as HTMLCanvasElement;
  canvas.zoom = 1; canvas.panX = 0; canvas.panY = 0;
  canvas.onCanvasChanged = vi.fn();
  canvas.onWarn = vi.fn();
  canvas.onInputWireDropRequest = vi.fn();
  canvas.onWireDropRequest = vi.fn();
  canvas.undoMgr = new UndoManager(canvas);
  canvas.input = new InputHandler(canvas);
  return { canvas, hook, memRead, ai };
}

const portOf = (n: CanvasNode, id: string) => n.ports.find(p => p.id === id)!;
const mouse = (x: number, y: number, mods: { ctrlKey?: boolean } = {}) => ({ clientX: x, clientY: y, button: 0, altKey: false, shiftKey: false, ctrlKey: mods.ctrlKey ?? false, metaKey: false, preventDefault: vi.fn() }) as unknown as MouseEvent;
const undoDepth = (c: Canvas) => (c.undoMgr as unknown as { stack: unknown[] }).stack.length;

type Pt = { x: number; y: number };
function drag(canvas: Canvas & Record<string, unknown>, from: Pt, to: Pt, mods: { ctrlKey?: boolean } = {}) {
  const input = canvas.input as InputHandler;
  input.onDown(mouse(from.x, from.y, mods));
  input.onWinMove(mouse(from.x + 10, from.y + 10, mods));
  input.onWinMove(mouse(to.x, to.y, mods));
  input.onWinUp(mouse(to.x, to.y, mods));
}
const edges = (c: Canvas) => [...c.connectors.values()].map(w => `${w.data.from_node}.${w.data.from_port}->${w.data.to_node}.${w.data.to_port}`).sort();

describe("output fan-out", () => {
  it("dragging a second wire from a wired output keeps the first wire", () => {
    const { canvas, hook, memRead, ai } = makeCanvas();
    const first = wire("w1", "hook", "output", "ai", "attachments");
    canvas.connectors.set("w1", first);

    const out = portOf(hook, "output");
    const target = portOf(memRead, "input");
    canvas.input.onDown(mouse(out.x, out.y));
    canvas.input.onWinMove(mouse(out.x + 40, out.y + 10));
    canvas.input.onWinMove(mouse(target.x, target.y));
    canvas.input.onWinUp(mouse(target.x, target.y));

    expect(canvas.connectors.size).toBe(2);
    expect(canvas.connectors.get("w1")).toBe(first);
    expect(first.data).toMatchObject({ from_node: "hook", to_node: "ai", to_port: "attachments" });
    const added = [...canvas.connectors.values()].find(c => c !== first)!;
    expect(added.data).toMatchObject({ from_node: "hook", from_port: "output", to_node: "mem_read", to_port: "input" });
    expect(ai.data.id).toBe("ai");
  });
});

describe("port press threshold", () => {
  it("does nothing until the pointer travels past the threshold", () => {
    const { canvas, hook, memRead } = makeCanvas();
    const out = portOf(hook, "output");
    const inp = portOf(memRead, "input");

    canvas.input.onDown(mouse(out.x, out.y));
    canvas.input.onWinMove(mouse(out.x + 3, out.y));
    expect(canvas.input.pendingConn).toBeNull();
    canvas.input.onWinUp(mouse(out.x + 3, out.y));
    expect(canvas.onWireDropRequest).not.toHaveBeenCalled();

    canvas.input.onDown(mouse(inp.x, inp.y));
    canvas.input.onWinUp(mouse(inp.x, inp.y));
    expect(canvas.onInputWireDropRequest).not.toHaveBeenCalled();
    expect(canvas.connectors.size).toBe(0);
    expect(undoDepth(canvas)).toBe(0);

    canvas.input.onDown(mouse(out.x, out.y));
    canvas.input.onWinMove(mouse(out.x + 5, out.y));
    expect(canvas.input.pendingConn).not.toBeNull();
  });

  it("selects the wire when a wired input is clicked without moving", () => {
    const { canvas, hook, ai } = makeCanvas();
    const w1 = wire("w1", "hook", "output", "ai", "input");
    canvas.connectors.set("w1", w1);
    const inp = portOf(ai, "input");

    canvas.input.onDown(mouse(inp.x, inp.y));
    expect(canvas.input.reconnEdge).toBeNull();
    canvas.input.onWinUp(mouse(inp.x, inp.y));

    expect(canvas.selectedConn).toBe(w1);
    expect(canvas.connectors.get("w1")).toBe(w1);
    expect(hook.data.id).toBe("hook");
  });
});

describe("reverse wire from an input", () => {
  it("connects to an output it is dropped on, as one undo step", () => {
    const { canvas, hook, ai } = makeCanvas();
    drag(canvas, portOf(ai, "attachments"), portOf(hook, "output"));

    expect(edges(canvas)).toEqual(["hook.output->ai.attachments"]);
    expect(undoDepth(canvas)).toBe(1);
  });

  it("opens the wire-in palette when dropped on empty canvas", () => {
    const { canvas, ai } = makeCanvas();
    const inp = portOf(ai, "input");
    drag(canvas, inp, { x: 5000, y: 5000 });

    expect(canvas.onInputWireDropRequest).toHaveBeenCalledWith("ai", "input", 5000, 5000);
    expect(canvas.connectors.size).toBe(0);
  });

  it("starts a new reverse wire from a multi-arity input instead of picking a wire up", () => {
    const { canvas, ai } = makeCanvas();
    ai.data.ports.inputs[0].arity = "multi";
    canvas.connectors.set("w1", wire("w1", "hook", "output", "ai", "input"));

    const inp = portOf(ai, "input");
    canvas.input.onDown(mouse(inp.x, inp.y));
    canvas.input.onWinMove(mouse(inp.x - 30, inp.y));

    expect(canvas.input.reconnEdge).toBeNull();
    expect(canvas.input.pendingConn?.direction).toBe("reverse");
  });
});

describe("picking up a wire from a wired single-arity input", () => {
  it("moves the wire to another input without pre-selecting it", () => {
    const { canvas, ai } = makeCanvas();
    const w1 = wire("w1", "hook", "output", "ai", "input");
    canvas.connectors.set("w1", w1);

    drag(canvas, portOf(ai, "input"), portOf(ai, "attachments"));

    expect(canvas.connectors.size).toBe(1);
    expect(w1.data).toMatchObject({ id: "w1", to_node: "ai", to_port: "attachments" });
    expect(undoDepth(canvas)).toBe(1);
  });
});

describe("occupied inputs", () => {
  it("replaces the wire on a single-arity input in one undo step and names the replaced source", () => {
    const { canvas, hook, memRead, ai } = makeCanvas();
    const old = wire("w1", "mem_read", "output", "ai", "input");
    canvas.connectors.set("w1", old);

    drag(canvas, portOf(hook, "output"), portOf(ai, "input"));

    expect(edges(canvas)).toEqual(["hook.output->ai.input"]);
    expect(undoDepth(canvas)).toBe(1);
    expect(canvas.onWarn).toHaveBeenCalledWith(expect.stringContaining('"mem_read"'));

    canvas.undo();
    expect(edges(canvas)).toEqual(["mem_read.output->ai.input"]);
    expect(canvas.connectors.get("w1")).toBe(old);

    canvas.redo();
    expect(edges(canvas)).toEqual(["hook.output->ai.input"]);
    expect(memRead.data.id).toBe("mem_read");
  });

  it("appends to a multi-arity input", () => {
    const { canvas, hook, ai } = makeCanvas();
    ai.data.ports.inputs[0].arity = "multi";
    canvas.connectors.set("w1", wire("w1", "mem_read", "output", "ai", "input"));

    drag(canvas, portOf(hook, "output"), portOf(ai, "input"));

    expect(edges(canvas)).toEqual(["hook.output->ai.input", "mem_read.output->ai.input"]);
    expect(canvas.onWarn).not.toHaveBeenCalled();
  });
});

describe("Ctrl-drag from an output", () => {
  it("moves the source of every wire on that output to the drop output as one undo step", () => {
    const { canvas, hook, memRead, ai } = makeCanvas();
    canvas.connectors.set("w1", wire("w1", "hook", "output", "ai", "input"));
    canvas.connectors.set("w2", wire("w2", "hook", "output", "ai", "attachments"));

    drag(canvas, portOf(hook, "output"), portOf(memRead, "output"), { ctrlKey: true });

    expect(edges(canvas)).toEqual(["mem_read.output->ai.attachments", "mem_read.output->ai.input"]);
    expect((ai.data.config as Record<string, unknown>).attachments_expr).toBe("{{mem_read.output}}");
    expect(undoDepth(canvas)).toBe(1);

    canvas.undo();
    expect(edges(canvas)).toEqual(["hook.output->ai.attachments", "hook.output->ai.input"]);
    expect((ai.data.config as Record<string, unknown>).attachments_expr).not.toBe("{{mem_read.output}}");

    canvas.redo();
    expect(edges(canvas)).toEqual(["mem_read.output->ai.attachments", "mem_read.output->ai.input"]);
  });

  it("refuses with a warning when a wire would loop into its own node", () => {
    const { canvas, hook, memRead } = makeCanvas();
    canvas.connectors.set("w1", wire("w1", "hook", "output", "mem_read", "input"));

    drag(canvas, portOf(hook, "output"), portOf(memRead, "output"), { ctrlKey: true });

    expect(edges(canvas)).toEqual(["hook.output->mem_read.input"]);
    expect(canvas.onWarn).toHaveBeenCalledOnce();
    expect(undoDepth(canvas)).toBe(0);
  });

  it("refuses when the drop output already feeds the same input", () => {
    const { canvas, hook, memRead } = makeCanvas();
    canvas.connectors.set("w1", wire("w1", "hook", "output", "ai", "attachments"));
    canvas.connectors.set("w2", wire("w2", "mem_read", "output", "ai", "attachments"));

    drag(canvas, portOf(hook, "output"), portOf(memRead, "output"), { ctrlKey: true });

    expect(edges(canvas)).toEqual(["hook.output->ai.attachments", "mem_read.output->ai.attachments"]);
    expect(canvas.onWarn).toHaveBeenCalledOnce();
    expect(undoDepth(canvas)).toBe(0);
  });

  it("makes a normal new wire when the output has no wires", () => {
    const { canvas, hook, ai } = makeCanvas();
    drag(canvas, portOf(hook, "output"), portOf(ai, "input"), { ctrlKey: true });

    expect(edges(canvas)).toEqual(["hook.output->ai.input"]);
    expect(undoDepth(canvas)).toBe(1);
  });
});

describe("Escape during a port drag", () => {
  it("cancels an armed press, a new wire, and a wire pickup without changing anything", () => {
    const { canvas, hook, ai } = makeCanvas();
    const w1 = wire("w1", "hook", "output", "ai", "input");
    canvas.connectors.set("w1", w1);
    const esc = () => canvas.input.onKey(new KeyboardEvent("keydown", { key: "Escape" }));
    const out = portOf(hook, "output");
    const inp = portOf(ai, "input");

    canvas.input.onDown(mouse(out.x, out.y));
    esc();
    expect(canvas.input.portPress).toBeNull();

    canvas.input.onDown(mouse(out.x, out.y));
    canvas.input.onWinMove(mouse(out.x + 30, out.y));
    expect(canvas.input.pendingConn).not.toBeNull();
    esc();
    canvas.input.onWinUp(mouse(out.x + 30, out.y));
    expect(canvas.input.pendingConn).toBeNull();
    expect(canvas.onWireDropRequest).not.toHaveBeenCalled();

    canvas.input.onDown(mouse(inp.x, inp.y));
    canvas.input.onWinMove(mouse(inp.x - 30, inp.y));
    expect(canvas.input.reconnEdge).not.toBeNull();
    esc();
    expect(canvas.input.reconnEdge).toBeNull();
    expect(canvas.connectors.get("w1")).toBe(w1);
    expect(undoDepth(canvas)).toBe(0);
  });
});

describe("loading a workflow with an overfed input", () => {
  it("keeps every wire and raises one warning", () => {
    const { canvas } = makeCanvas();
    canvas.connectors.set("w1", wire("w1", "hook", "output", "ai", "input"));
    canvas.connectors.set("w2", wire("w2", "mem_read", "output", "ai", "input"));
    canvas.connectors.set("w3", wire("w3", "hook", "output", "ai", "attachments"));

    canvas.warnArityViolations();

    expect(canvas.connectors.size).toBe(3);
    expect(canvas.onWarn).toHaveBeenCalledOnce();
    expect(canvas.onWarn).toHaveBeenCalledWith(expect.stringContaining("One input"));
  });

  it("stays quiet for a multi-arity input and for fan-out", () => {
    const { canvas, ai } = makeCanvas();
    ai.data.ports.inputs[0].arity = "multi";
    canvas.connectors.set("w1", wire("w1", "hook", "output", "ai", "input"));
    canvas.connectors.set("w2", wire("w2", "mem_read", "output", "ai", "input"));
    canvas.connectors.set("w3", wire("w3", "hook", "output", "ai", "attachments"));
    canvas.connectors.set("w4", wire("w4", "hook", "output", "mem_read", "input"));

    canvas.warnArityViolations();

    expect(canvas.onWarn).not.toHaveBeenCalled();
  });
});

describe("building the chat graph by gestures alone", () => {
  it("wires hook, memory, AI and output nodes into the full graph", () => {
    const { canvas, hook, memRead, ai } = makeCanvas();
    const out = node("out", "output", 1200, ["input"], [], 0);
    const memUser = node("mem_user", "ai_memory", 1200, ["input"], ["output"], 300);
    const memBot = node("mem_bot", "ai_memory", 1600, ["input"], ["output"], 300);
    for (const n of [out, memUser, memBot]) (canvas.nodes as Map<string, CanvasNode>).set(n.data.id, n);

    drag(canvas, portOf(hook, "output"), portOf(ai, "attachments"));
    drag(canvas, portOf(memRead, "output"), portOf(ai, "input"));
    drag(canvas, portOf(ai, "output"), portOf(out, "input"));
    drag(canvas, portOf(ai, "output"), portOf(memUser, "input"));
    drag(canvas, portOf(memUser, "output"), portOf(memBot, "input"));
    drag(canvas, portOf(memRead, "input"), portOf(hook, "output"));

    expect(edges(canvas)).toEqual([
      "ai.output->mem_user.input",
      "ai.output->out.input",
      "hook.output->ai.attachments",
      "hook.output->mem_read.input",
      "mem_read.output->ai.input",
      "mem_user.output->mem_bot.input",
    ]);
    const warnings = (canvas.onWarn as ReturnType<typeof vi.fn>).mock.calls.map(c => String(c[0]));
    expect(warnings.filter(w => /Replaced|more than one wire/.test(w))).toEqual([]);
    expect(undoDepth(canvas)).toBe(6);
  });
});
