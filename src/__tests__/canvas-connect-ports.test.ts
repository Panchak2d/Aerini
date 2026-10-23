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

function node(id: string, inputs: string[], outputs: string[], typeId = id, config: Record<string, unknown> = {}, name = id): CanvasNode {
  return new CanvasNode({
    id, node_type_id: typeId, node_type: "action", name, config, credentials: {},
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

describe("Canvas.connectPorts chat defaults", () => {
  const chatCanvas = (aiConfig: Record<string, unknown> = {}, outConfig: Record<string, unknown> = {}) => {
    const canvas = makeCanvas();
    canvas.nodes.set("wh", node("wh", [], ["output"], "webhook", {}, "Chat In"));
    canvas.nodes.set("llm", node("llm", ["input"], ["output"], "ai_prompt", aiConfig, "AI Prompt"));
    canvas.nodes.set("show", node("show", ["input"], ["output"], "output", outConfig, "Output"));
    return canvas;
  };
  const cfg = (c: Canvas, id: string) => c.nodes.get(id)!.data.config as Record<string, unknown>;

  it("fills a blank AI prompt with the chat message when a Webhook is wired in, as one undo step", () => {
    const canvas = chatCanvas();

    expect(canvas.connectPorts("wh", "output", "llm", "input")).toBe(true);

    expect(cfg(canvas, "llm").prompt).toBe("{{Chat In.output.body.message}}");
    expect(canvas.onWarn).toHaveBeenCalledTimes(1);
    expect(undoDepth(canvas)).toBe(1);
    canvas.undo();
    expect(cfg(canvas, "llm").prompt).toBeUndefined();
  });

  it("leaves a prompt the user already wrote, and prompts fed by other nodes, alone", () => {
    const written = chatCanvas({ prompt: "Summarize this" });
    written.connectPorts("wh", "output", "llm", "input");
    expect(cfg(written, "llm").prompt).toBe("Summarize this");

    const other = chatCanvas();
    other.connectPorts("other", "output", "llm", "input");
    expect(cfg(other, "llm").prompt).toBeUndefined();
  });

  it("sets an Output's Field to the AI reply only when it was blank", () => {
    const blank = chatCanvas();
    blank.connectPorts("llm", "output", "show", "input");
    expect(cfg(blank, "show").field).toBe("content");

    const chosen = chatCanvas({}, { field: "body.text" });
    chosen.connectPorts("llm", "output", "show", "input");
    expect(cfg(chosen, "show").field).toBe("body.text");

    const fromOther = chatCanvas();
    fromOther.connectPorts("other", "output", "show", "input");
    expect(cfg(fromOther, "show").field).toBeUndefined();
  });
});

describe("Canvas.connectPorts error port", () => {
  it("refuses to wire an Error port into a Files port and says why, but allows it into a normal input", () => {
    const canvas = makeCanvas();
    const hook = node("hook", [], ["output", "on_error"]);
    canvas.nodes.set("hook", hook);
    canvas.nodes.get("ai")!.data.ports.inputs.find(p => p.id === "attachments")!.port_type = "files";

    expect(canvas.connectPorts("hook", "on_error", "ai", "attachments")).toBe(false);
    expect(canvas.connectors.size).toBe(0);
    expect(undoDepth(canvas)).toBe(0);
    expect(canvas.onWarn).toHaveBeenCalledWith(expect.stringContaining("Error port"));

    expect(canvas.connectPorts("hook", "on_error", "ai", "input")).toBe(true);
  });
});
