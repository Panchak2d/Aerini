/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { InputHandler } from "../canvas/InputHandler";
import { UndoManager } from "../canvas/UndoManager";
import { Connector, WIRE_DELETE_HIT_RADIUS } from "../canvas/Connector";
import { CanvasNode, NODE_WIDTH } from "../canvas/Node";

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

function wire(id: string, from: string, to: string, toPort: string, onSuccess: string | null = null): Connector {
  return new Connector({ id, from_node: from, from_port: "output", to_node: to, to_port: toPort, condition: null, on_success: onSuccess, on_failure: null });
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
  canvas.hoveredConn = null;
  canvas.deleteHandleHot = false;
  canvas.el = { classList: { add: vi.fn(), remove: vi.fn() }, style: { cursor: "" }, getBoundingClientRect: () => ({ left: 0, top: 0 }) } as unknown as HTMLCanvasElement;
  canvas.zoom = 1; canvas.panX = 0; canvas.panY = 0;
  canvas.onCanvasChanged = vi.fn();
  canvas.undoMgr = new UndoManager(canvas);
  canvas.input = new InputHandler(canvas);
  return { canvas, hook, mem, ai };
}

const mouse = (x: number, y: number) => ({ clientX: x, clientY: y, button: 0, altKey: false, shiftKey: false, preventDefault: vi.fn() }) as unknown as MouseEvent;
const undoDepth = (c: Canvas) => (c.undoMgr as unknown as { stack: unknown[] }).stack.length;
const anchorOf = (c: Canvas, w: Connector) => w.deleteButtonAnchor(c.nodes)!;

describe("wire hover", () => {
  it("hovering a wire marks it hovered and moving to empty canvas clears it", () => {
    const { canvas } = makeCanvas();
    const w1 = wire("w1", "hook", "mem", "input");
    canvas.connectors.set("w1", w1);
    const at = anchorOf(canvas, w1);

    canvas.input.onMove(mouse(at.x, at.y));
    expect(canvas.hoveredConn).toBe(w1);
    expect(w1.hovered).toBe(true);

    canvas.input.onMove(mouse(5000, 5000));
    expect(canvas.hoveredConn).toBeNull();
    expect(w1.hovered).toBe(false);
  });

  it("does not hover a wire that runs beneath a node", () => {
    const { canvas, mem } = makeCanvas();
    const w1 = wire("w1", "hook", "ai", "input");
    canvas.connectors.set("w1", w1);
    const at = anchorOf(canvas, w1);
    mem.data.position = { x: at.x - 20, y: at.y - 10 };
    mem.updatePortPositions();
    expect(mem.containsPoint(at.x, at.y)).toBe(true);

    canvas.input.onMove(mouse(at.x, at.y));

    expect(canvas.hoveredConn).toBeNull();
  });

  it("keeps the hover while the pointer is on the delete button but off the wire itself", () => {
    const { canvas } = makeCanvas();
    const w1 = wire("w1", "hook", "mem", "input");
    canvas.connectors.set("w1", w1);
    const at = anchorOf(canvas, w1);
    canvas.input.onMove(mouse(at.x, at.y));

    const offY = at.y + WIRE_DELETE_HIT_RADIUS - 2;
    expect(w1.containsPoint(at.x, offY, canvas.nodes, 8)).toBe(false);
    canvas.input.onMove(mouse(at.x, offY));

    expect(canvas.hoveredConn).toBe(w1);
    expect(canvas.deleteHandleHot).toBe(true);
    expect(canvas.el.style.cursor).toBe("pointer");
  });

  it("clears the hover when the pointer leaves the canvas", () => {
    const { canvas } = makeCanvas();
    const w1 = wire("w1", "hook", "mem", "input");
    canvas.connectors.set("w1", w1);
    const at = anchorOf(canvas, w1);
    canvas.input.onMove(mouse(at.x, at.y));

    canvas.input.onLeave();

    expect(canvas.hoveredConn).toBeNull();
    expect(w1.hovered).toBe(false);
  });
});

describe("wire delete button visibility", () => {
  it("shows for a selected wire without hover and not for an unselected, unhovered one", () => {
    const { canvas } = makeCanvas();
    const w1 = wire("w1", "hook", "mem", "input");
    canvas.connectors.set("w1", w1);
    expect(canvas.deleteHandleTarget()).toBeNull();

    canvas.selectConn(w1);

    expect(canvas.deleteHandleTarget()).toMatchObject({ conn: w1, ...anchorOf(canvas, w1) });
  });

  it("is hidden while a gesture is in progress", () => {
    const { canvas } = makeCanvas();
    const w1 = wire("w1", "hook", "mem", "input");
    canvas.connectors.set("w1", w1);
    canvas.selectConn(w1);

    canvas.input.isPanning = true;

    expect(canvas.deleteHandleTarget()).toBeNull();
  });

  it("moves off the midpoint on a wire that carries a success route", () => {
    const { canvas } = makeCanvas();
    const plain = wire("w1", "hook", "mem", "input");
    const branched = wire("w2", "hook", "mem", "input", "ok");

    expect(anchorOf(canvas, branched).x).toBeLessThan(anchorOf(canvas, plain).x);
  });

  it("is hidden on a wire too short to clear both end handles", () => {
    const { canvas, mem } = makeCanvas();
    const w1 = wire("w1", "hook", "mem", "input");
    canvas.connectors.set("w1", w1);
    mem.data.position = { x: NODE_WIDTH + 20, y: 0 };
    mem.updatePortPositions();
    canvas.selectConn(w1);

    expect(canvas.deleteHandleTarget()).toBeNull();
  });
});

describe("wire delete button press", () => {
  it("deletes exactly that wire as one undo step, over a node, and undo restores it", () => {
    const { canvas, mem } = makeCanvas();
    const w1 = wire("w1", "hook", "ai", "input");
    const w2 = wire("w2", "mem", "ai", "attachments");
    canvas.connectors.set("w1", w1); canvas.connectors.set("w2", w2);
    const at = anchorOf(canvas, w1);
    mem.data.position = { x: at.x - 20, y: at.y - 10 };
    mem.updatePortPositions();
    canvas.selectConn(w1);

    canvas.input.onDown(mouse(at.x, at.y));
    expect(canvas.input.wireDeletePress).toBe(w1);
    canvas.input.onWinUp(mouse(at.x, at.y));

    expect(canvas.connectors.has("w1")).toBe(false);
    expect(canvas.connectors.get("w2")).toBe(w2);
    expect(undoDepth(canvas)).toBe(1);
    expect(canvas.input.wireDeletePress).toBeNull();

    canvas.undo();
    expect(canvas.connectors.get("w1")).toBe(w1);
    expect(canvas.connectors.get("w2")).toBe(w2);
  });

  it("deletes nothing when released outside the button, even outside the canvas", () => {
    const { canvas } = makeCanvas();
    const w1 = wire("w1", "hook", "mem", "input");
    canvas.connectors.set("w1", w1);
    const at = anchorOf(canvas, w1);
    canvas.selectConn(w1);

    canvas.input.onDown(mouse(at.x, at.y));
    canvas.input.onWinMove(mouse(at.x + 300, at.y + 300));
    canvas.input.onWinUp(mouse(at.x + 300, at.y + 300));

    expect(canvas.connectors.get("w1")).toBe(w1);
    expect(undoDepth(canvas)).toBe(0);
    expect(canvas.input.wireDeletePress).toBeNull();
  });

  it("cancels a pressed button on Escape", () => {
    const { canvas } = makeCanvas();
    const w1 = wire("w1", "hook", "mem", "input");
    canvas.connectors.set("w1", w1);
    const at = anchorOf(canvas, w1);
    canvas.selectConn(w1);
    canvas.input.onDown(mouse(at.x, at.y));

    canvas.input.onKey(new KeyboardEvent("keydown", { key: "Escape" }));
    canvas.input.onWinUp(mouse(at.x, at.y));

    expect(canvas.input.wireDeletePress).toBeNull();
    expect(canvas.connectors.get("w1")).toBe(w1);
    expect(undoDepth(canvas)).toBe(0);
  });
});
