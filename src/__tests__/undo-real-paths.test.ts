/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { InputHandler } from "../canvas/InputHandler";
import { UndoManager } from "../canvas/UndoManager";
import { Connector } from "../canvas/Connector";
import { CanvasNode, NODE_WIDTH } from "../canvas/Node";
import type { NodeDescriptor } from "../ipc/workflow";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

type NodeOpts = { x?: number; y?: number; inputs?: string[]; dynamic?: boolean };

function node(id: string, typeId: string, o: NodeOpts = {}): CanvasNode {
  const inputs = o.inputs ?? ["input"];
  return new CanvasNode({
    id, node_type_id: typeId, node_type: "action", name: id, config: {}, credentials: {},
    position: { x: o.x ?? 0, y: o.y ?? 0 },
    ports: {
      inputs:  inputs.map(i => ({ id: i, label: i, position: "left" as const })),
      outputs: [{ id: "output", label: "output", position: "right" as const }],
    },
    input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: o.dynamic ?? false,
  });
}

function wire(id: string, from: string, to: string, toPort = "input"): Connector {
  return new Connector({ id, from_node: from, from_port: "output", to_node: to, to_port: toPort, condition: null, on_success: null, on_failure: null });
}

function descriptor(name: string, inputs: string[] = ["input"], outputs: string[] = ["output"]): NodeDescriptor {
  return {
    type_id: "http_request", display_name: name, node_type: "action", version: "1",
    input_schema: {}, output_schema: {},
    ports: {
      inputs:  inputs.map(i => ({ id: i, label: i, position: "left" as const })),
      outputs: outputs.map(o => ({ id: o, label: o, position: "right" as const })),
    },
  };
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

const add = (c: Canvas, ...ns: CanvasNode[]) => ns.forEach(n => c.nodes.set(n.data.id, n));
const depth = (c: Canvas) => (c.undoMgr as unknown as { stack: unknown[] }).stack.length;
const label = (c: Canvas) => c.undoMgr.state().undoLabel;
const edges = (c: Canvas) => [...c.connectors.values()].map(w => `${w.data.from_node}->${w.data.to_node}.${w.data.to_port}`).sort();
const portOf = (n: CanvasNode, id: string) => n.ports.find(p => p.id === id)!;
const mouse = (x: number, y: number) =>
  ({ clientX: x, clientY: y, button: 0, altKey: false, shiftKey: false, ctrlKey: false, metaKey: false, preventDefault: vi.fn() }) as unknown as MouseEvent;

/** World point on the middle of the wire from `from.output` to `to.<toPort>`. */
function wireMidpoint(from: CanvasNode, to: CanvasNode, toPort = "input") {
  const fp = portOf(from, "output"), tp = portOf(to, toPort);
  return { x: (fp.x + tp.x) / 2, y: (fp.y + tp.y) / 2 };
}

/** Moves `n` so its centre sits at `p`, as a drop onto a wire would. */
function centreOn(n: CanvasNode, p: { x: number; y: number }) {
  n.data.position = { x: p.x - NODE_WIDTH / 2, y: p.y - n.height / 2 };
  n.updatePortPositions();
}

beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(1_000_000); localStorage.clear(); });
afterEach(() => { vi.useRealTimers(); });

describe("tryWireInsert (the real split path)", () => {
  it("splits the wire; one undo restores it exactly and redo re-splits it", () => {
    const canvas = makeCanvas();
    const a = node("a", "http_request", { x: 0, y: 200 }), b = node("b", "http_request", { x: 900, y: 200 });
    const mid = node("mid", "http_request", { x: 0, y: 600 });
    add(canvas, a, b, mid);
    const original = wire("w_ab", "a", "b");
    canvas.connectors.set("w_ab", original);
    centreOn(mid, wireMidpoint(a, b));

    expect(canvas.tryWireInsert(mid)).toBe(true);
    expect(edges(canvas)).toEqual(["a->mid.input", "mid->b.input"]);
    expect(depth(canvas)).toBe(1);

    canvas.undo();
    expect(canvas.connectors.get("w_ab")).toBe(original);
    expect(edges(canvas)).toEqual(["a->b.input"]);
    expect(canvas.nodes.get("mid")).toBe(mid);

    canvas.redo();
    expect(edges(canvas)).toEqual(["a->mid.input", "mid->b.input"]);
  });

  it("edge case: a node away from every wire changes nothing and records nothing", () => {
    const canvas = makeCanvas();
    const a = node("a", "http_request", { x: 0, y: 200 }), b = node("b", "http_request", { x: 900, y: 200 });
    const far = node("far", "http_request", { x: 400, y: 700 });
    add(canvas, a, b, far);
    canvas.connectors.set("w_ab", wire("w_ab", "a", "b"));

    expect(canvas.tryWireInsert(far)).toBe(false);
    expect(edges(canvas)).toEqual(["a->b.input"]);
    expect(depth(canvas)).toBe(0);
  });

  it("splitting a wire into AI Prompt's Files port re-points its expression at the inserted node, and undo/redo round-trip it", () => {
    const canvas = makeCanvas();
    const src = node("src", "webhook", { x: 0, y: 200 });
    const ai = node("ai", "ai_prompt", { x: 900, y: 200, inputs: ["input", "attachments"] });
    const mid = node("mid", "http_request", { x: 0, y: 600 });
    add(canvas, src, ai, mid);
    canvas.connectors.set("w1", wire("w1", "src", "ai", "attachments"));
    ai.data.config["attachments_expr"] = "{{src.output}}";
    centreOn(mid, wireMidpoint(src, ai, "attachments"));

    expect(canvas.tryWireInsert(mid)).toBe(true);
    expect(ai.data.config["attachments_expr"]).toBe("{{mid.output}}");

    canvas.undo();
    expect(ai.data.config["attachments_expr"]).toBe("{{src.output}}");
    canvas.redo();
    expect(ai.data.config["attachments_expr"]).toBe("{{mid.output}}");
  });

  it("splitting a wire into a dynamic-port node's flat input re-points its files expression, and undo restores it", () => {
    const canvas = makeCanvas();
    const src = node("src", "text_to_file", { x: 0, y: 200 });
    const save = node("save", "save_to_folder", { x: 900, y: 200, dynamic: true });
    const mid = node("mid", "http_request", { x: 0, y: 600 });
    add(canvas, src, save, mid);
    canvas.connectors.set("w1", wire("w1", "src", "save", "input"));
    save.data.config["files"] = "{{src.output}}";
    centreOn(mid, wireMidpoint(src, save));

    expect(canvas.tryWireInsert(mid)).toBe(true);
    expect(save.data.config["files"]).toBe("{{mid.output}}");

    canvas.undo();
    expect(save.data.config["files"]).toBe("{{src.output}}");
    expect(edges(canvas)).toEqual(["src->save.input"]);
  });
});

describe("Canvas.placeNode (the real placement path)", () => {
  it("placing a node onto a wire is one step; undo removes the node and restores the wire", () => {
    const canvas = makeCanvas();
    const a = node("a", "http_request", { x: 0, y: 200 }), b = node("b", "http_request", { x: 900, y: 200 });
    add(canvas, a, b);
    const original = wire("w_ab", "a", "b");
    canvas.connectors.set("w_ab", original);
    const at = wireMidpoint(a, b);

    const placed = canvas.placeNode(descriptor("Mid"), at.x, at.y);

    expect(canvas.nodes.get(placed.data.id)).toBe(placed);
    expect(edges(canvas)).toEqual([`a->${placed.data.id}.input`, `${placed.data.id}->b.input`]);
    expect(depth(canvas)).toBe(1);
    expect(label(canvas)).toBe('Add "Mid"');

    canvas.undo();
    expect(canvas.nodes.has(placed.data.id)).toBe(false);
    expect(canvas.connectors.get("w_ab")).toBe(original);
    expect(canvas.connectors.size).toBe(1);

    canvas.redo();
    expect(canvas.nodes.get(placed.data.id)).toBe(placed);
    expect(canvas.connectors.size).toBe(2);
  });

  it("edge case: placing a node away from every wire is a single add step", () => {
    const canvas = makeCanvas();
    const a = node("a", "http_request", { x: 0, y: 200 }), b = node("b", "http_request", { x: 900, y: 200 });
    add(canvas, a, b);
    canvas.connectors.set("w_ab", wire("w_ab", "a", "b"));

    const placed = canvas.placeNode(descriptor("Lone"), 300, 800);

    expect(edges(canvas)).toEqual(["a->b.input"]);
    expect(depth(canvas)).toBe(1);
    canvas.undo();
    expect(canvas.nodes.has(placed.data.id)).toBe(false);
  });
});

describe("wire-drop completion (the real drop paths)", () => {
  it("dropping a wire on empty canvas adds the picked node and its wire as one step", () => {
    const canvas = makeCanvas();
    const a = node("a", "http_request", { x: 0, y: 200 });
    add(canvas, a);
    canvas._pendingWireDrop = { fromNode: "a", fromPort: "output", wx: 700, wy: 300 };

    canvas.completeWireDrop(descriptor("Next"));

    expect(canvas.nodes.size).toBe(2);
    expect(canvas.connectors.size).toBe(1);
    expect(depth(canvas)).toBe(1);
    expect(label(canvas)).toBe('Add "Next"');

    canvas.undo();
    expect(canvas.nodes.size).toBe(1);
    expect(canvas.connectors.size).toBe(0);
    canvas.redo();
    expect(canvas.nodes.size).toBe(2);
    expect(canvas.connectors.size).toBe(1);
  });

  it("dropping an input wire onto an occupied single input replaces the old wire in one step; undo brings it back", () => {
    const canvas = makeCanvas();
    const a = node("a", "http_request", { x: 0, y: 200 }), b = node("b", "http_request", { x: 900, y: 200 });
    add(canvas, a, b);
    const original = wire("w_ab", "a", "b");
    canvas.connectors.set("w_ab", original);
    canvas._pendingInputWireDrop = { toNode: "b", toPort: "input", wx: 700, wy: 500 };

    canvas.completeInputWireDrop(descriptor("Feeder"));

    const feeder = [...canvas.nodes.values()].find(n => n.data.name === "Feeder")!;
    expect(edges(canvas)).toEqual([`${feeder.data.id}->b.input`]);
    expect(depth(canvas)).toBe(1);

    canvas.undo();
    expect(canvas.nodes.has(feeder.data.id)).toBe(false);
    expect(canvas.connectors.get("w_ab")).toBe(original);
    expect(canvas.connectors.size).toBe(1);

    canvas.redo();
    expect(edges(canvas)).toEqual([`${feeder.data.id}->b.input`]);
  });

  it("edge case: a descriptor with no ports to connect adds nothing and records nothing", () => {
    const canvas = makeCanvas();
    add(canvas, node("a", "http_request"));
    canvas._pendingWireDrop = { fromNode: "a", fromPort: "output", wx: 700, wy: 300 };

    canvas.completeWireDrop(descriptor("Sink", []));

    expect(canvas.nodes.size).toBe(1);
    expect(depth(canvas)).toBe(0);
  });
});

describe("InputHandler node drag (the real mouse path)", () => {
  function setup() {
    const canvas = makeCanvas();
    const a = node("a", "http_request", { x: 0, y: 200 }), b = node("b", "http_request", { x: 900, y: 200 });
    const mid = node("mid", "http_request", { x: 300, y: 600 });
    add(canvas, a, b, mid);
    const original = wire("w_ab", "a", "b");
    canvas.connectors.set("w_ab", original);
    return { canvas, a, b, mid, original };
  }

  function dragNode(canvas: Canvas, n: CanvasNode, to: { x: number; y: number }) {
    const grab = { x: n.data.position.x + 20, y: n.data.position.y + 10 };
    canvas.input.onDown(mouse(grab.x, grab.y));
    canvas.input.onWinMove(mouse(grab.x + 5, grab.y + 5));
    canvas.input.onWinMove(mouse(to.x + 20, to.y + 10));
    canvas.input.onWinUp(mouse(to.x + 20, to.y + 10));
  }

  it("dropping a dragged node onto a wire is one step; undo restores the wire and the old position and keeps the node", () => {
    const { canvas, a, b, mid, original } = setup();
    const target = wireMidpoint(a, b);

    dragNode(canvas, mid, { x: target.x - NODE_WIDTH / 2, y: target.y - mid.height / 2 });

    expect(edges(canvas)).toEqual(["a->mid.input", "mid->b.input"]);
    expect(depth(canvas)).toBe(1);
    expect(label(canvas)).toBe('Move "mid"');

    canvas.undo();
    expect(canvas.nodes.get("mid")).toBe(mid);
    expect(mid.data.position).toEqual({ x: 300, y: 600 });
    expect(canvas.connectors.get("w_ab")).toBe(original);
    expect(canvas.connectors.size).toBe(1);

    canvas.redo();
    expect(edges(canvas)).toEqual(["a->mid.input", "mid->b.input"]);
    expect(mid.data.position.x).toBeCloseTo(target.x - NODE_WIDTH / 2);
  });

  it("edge case: dragging a node somewhere clear of every wire is a plain move step", () => {
    const { canvas, mid } = setup();

    dragNode(canvas, mid, { x: 500, y: 750 });

    expect(edges(canvas)).toEqual(["a->b.input"]);
    expect(depth(canvas)).toBe(1);
    canvas.undo();
    expect(mid.data.position).toEqual({ x: 300, y: 600 });
  });
});
