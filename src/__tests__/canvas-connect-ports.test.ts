/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { UndoManager } from "../canvas/UndoManager";
import { Connector } from "../canvas/Connector";
import { CanvasNode } from "../canvas/Node";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function node(id: string, inputs: string[], outputs: string[]): CanvasNode {
  return new CanvasNode({
    id, node_type_id: id, node_type: "action", name: id, config: {}, credentials: {},
    position: { x: 0, y: 0 },
    ports: {
      inputs:  inputs.map(i => ({ id: i, label: i, position: "left" as const })),
      outputs: outputs.map(o => ({ id: o, label: o, position: "right" as const })),
    },
    input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: false,
  });
}

function makeCanvas() {
  const canvas = Object.create(Canvas.prototype) as Canvas & Record<string, unknown>;
  canvas.nodes = new Map([
    ["hook", node("hook", [], ["output"])],
    ["other", node("other", [], ["output"])],
    ["ai", node("ai", ["input", "attachments"], ["output"])],
  ]);
  canvas.connectors = new Map();
  canvas.selectedNodes = new Set();
  canvas.selectedNode = null;
  canvas.selectedConn = null;
  canvas.onCanvasChanged = vi.fn();
  canvas.onWarn = vi.fn();
  canvas.undoMgr = new UndoManager(canvas);
  return canvas;
}

const wire = (id: string, from: string, to: string, toPort: string) =>
  new Connector({ id, from_node: from, from_port: "output", to_node: to, to_port: toPort, condition: null, on_success: null, on_failure: null });
const undoDepth = (c: Canvas) => (c.undoMgr as unknown as { stack: unknown[] }).stack.length;

describe("Canvas.connectPorts", () => {
  it("replaces a wire already on the target port as a single undo step", () => {
    const canvas = makeCanvas();
    const old = wire("e1", "other", "ai", "attachments");
    canvas.connectors.set("e1", old);

    expect(canvas.connectPorts("hook", "output", "ai", "attachments")).toBe(true);

    expect(canvas.connectors.has("e1")).toBe(false);
    expect([...canvas.connectors.values()].map(c => c.data.from_node)).toEqual(["hook"]);
    expect(undoDepth(canvas)).toBe(1);

    canvas.undo();
    expect(canvas.connectors.get("e1")).toBe(old);
    expect(canvas.connectors.size).toBe(1);
  });

  it("changes nothing and returns false when either port does not exist", () => {
    const canvas = makeCanvas();
    canvas.connectors.set("e1", wire("e1", "other", "ai", "input"));

    expect(canvas.connectPorts("hook", "output", "ai", "nope")).toBe(false);
    expect(canvas.connectPorts("hook", "nope", "ai", "input")).toBe(false);
    expect(canvas.connectPorts("gone", "output", "ai", "input")).toBe(false);

    expect(canvas.connectors.has("e1")).toBe(true);
    expect(canvas.connectors.size).toBe(1);
    expect(undoDepth(canvas)).toBe(0);
  });

  it("is a no-op when the identical wire already exists", () => {
    const canvas = makeCanvas();
    canvas.connectors.set("e1", wire("e1", "hook", "ai", "input"));

    expect(canvas.connectPorts("hook", "output", "ai", "input")).toBe(true);

    expect(canvas.connectors.size).toBe(1);
    expect(undoDepth(canvas)).toBe(0);
  });
});
