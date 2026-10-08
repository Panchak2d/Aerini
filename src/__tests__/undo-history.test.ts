/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { Connector } from "../canvas/Connector";
import { CanvasNode } from "../canvas/Node";
import { UndoManager, historyShortcut, MAX_HISTORY, COALESCE_MS } from "../canvas/UndoManager";
import type { HistoryState } from "../canvas/UndoManager";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function node(id: string, typeId = "http_request", x = 0): CanvasNode {
  return new CanvasNode({
    id, node_type_id: typeId, node_type: "action", name: `Node ${id}`,
    config: {}, credentials: {}, position: { x, y: 0 },
    ports: {
      inputs:  [{ id: "input", label: "Input", position: "left" }, { id: "attachments", label: "Files", position: "left" }],
      outputs: [{ id: "output", label: "Output", position: "right" }],
    },
    input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: false,
  });
}

function wire(id: string, from: string, to: string, toPort = "input"): Connector {
  return new Connector({ id, from_node: from, from_port: "output", to_node: to, to_port: toPort, condition: null, on_success: null, on_failure: null });
}

function makeCanvas(ids: string[] = ["a", "b", "c"]) {
  const canvas = Object.create(Canvas.prototype) as Canvas & Record<string, unknown>;
  canvas.nodes = new Map(ids.map((id, i) => [id, node(id, "http_request", i * 300)]));
  canvas.connectors = new Map();
  canvas.selectedNodes = new Set();
  canvas.selectedNode = null;
  canvas.selectedConn = null;
  canvas.onCanvasChanged = vi.fn();
  canvas.onWarn = vi.fn();
  const events: HistoryState[] = [];
  canvas.onHistoryChange = (s: HistoryState) => events.push(s);
  canvas.undoMgr = new UndoManager(canvas);
  return { canvas, events };
}

const last = <T,>(a: T[]): T | undefined => a[a.length - 1];
const depth = (c: Canvas) => (c.undoMgr as unknown as { stack: unknown[] }).stack.length;

beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(1_000_000); });
afterEach(() => { vi.useRealTimers(); });

describe("historyShortcut", () => {
  const key = (o: Partial<KeyboardEvent>) =>
    ({ key: "", code: "", shiftKey: false, altKey: false, ctrlKey: true, metaKey: false, ...o }) as KeyboardEvent;

  it("recognises undo and both redo chords, whatever the key's case", () => {
    expect(historyShortcut(key({ key: "z", code: "KeyZ" }))).toBe("undo");
    expect(historyShortcut(key({ key: "Z", code: "KeyZ", shiftKey: true }))).toBe("redo");
    expect(historyShortcut(key({ key: "y", code: "KeyY" }))).toBe("redo");
    expect(historyShortcut(key({ key: "Z", code: "KeyZ" }))).toBe("undo");
    expect(historyShortcut(key({ key: "z", metaKey: true, ctrlKey: false, code: "KeyZ" }))).toBe("undo");
  });

  it("edge cases: non-Latin layouts use the physical key, AltGr and bare keys are ignored", () => {
    expect(historyShortcut(key({ key: "я", code: "KeyZ" }))).toBe("undo");
    expect(historyShortcut(key({ key: "z", code: "KeyW" }))).toBe("undo");
    expect(historyShortcut(key({ key: "z", code: "KeyZ", altKey: true }))).toBeNull();
    expect(historyShortcut(key({ key: "z", code: "KeyZ", ctrlKey: false }))).toBeNull();
    expect(historyShortcut(key({ key: "Y", code: "KeyY", shiftKey: true }))).toBeNull();
  });
});

describe("transactions", () => {
  it("everything pushed inside transact is one step, undone and redone as a unit", () => {
    const { canvas } = makeCanvas();
    const c1 = wire("w1", "a", "b"), c2 = wire("w2", "b", "c");
    canvas.transact("Wire both", () => {
      canvas.connectors.set("w1", c1); canvas.pushUndo({ type: "add_edge", connector: c1 });
      canvas.connectors.set("w2", c2); canvas.pushUndo({ type: "add_edge", connector: c2 });
    });
    expect(depth(canvas)).toBe(1);
    expect(canvas.undoMgr.state().undoLabel).toBe("Wire both");
    canvas.undo();
    expect(canvas.connectors.size).toBe(0);
    canvas.redo();
    expect(canvas.connectors.size).toBe(2);
  });

  it("edge case: a throwing body still records what it already changed; nested calls join the outer step", () => {
    const { canvas } = makeCanvas();
    const c1 = wire("w1", "a", "b");
    expect(() => canvas.transact("Outer", () => {
      canvas.transact("Inner", () => { canvas.connectors.set("w1", c1); canvas.pushUndo({ type: "add_edge", connector: c1 }); });
      throw new Error("boom");
    })).toThrow("boom");
    expect(depth(canvas)).toBe(1);
    canvas.undo();
    expect(canvas.connectors.size).toBe(0);
  });
});

describe("Canvas.placeNode / drag / duplicate are single steps", () => {
  it("duplicating several nodes undoes in one step", () => {
    const { canvas } = makeCanvas();
    (canvas as unknown as { snap: unknown }).snap = { avoidOverlap: vi.fn() };
    canvas.selectedNodes = new Set(["a", "b"]);
    canvas.dupSelected();
    expect(canvas.nodes.size).toBe(5);
    expect(depth(canvas)).toBe(1);
    canvas.undo();
    expect(canvas.nodes.size).toBe(3);
  });
});

describe("workflow switch", () => {
  it("replacing the canvas graph drops the previous workflow's history", () => {
    const { canvas, events } = makeCanvas();
    const c1 = wire("w1", "a", "b");
    canvas.connectors.set("w1", c1);
    canvas.pushUndo({ type: "add_edge", connector: c1 });
    expect(canvas.canUndo()).toBe(true);

    canvas.nodes = new Map([["z", node("z")]]);
    canvas.connectors = new Map();
    expect(canvas.canUndo()).toBe(false);
    expect(last(events)?.kind).toBe("clear");
    expect(canvas.undo()).toBe(false);
    expect(canvas.nodes.size).toBe(1);
  });

  it("replacing only the connector map also drops history, and an already-empty history stays silent", () => {
    const { canvas, events } = makeCanvas();
    canvas.connectors = new Map();
    expect(events).toHaveLength(0);

    canvas.pushUndo({ type: "move_node", nodeId: "a", from: { x: 0, y: 0 }, to: { x: 5, y: 5 } });
    const before = events.length;
    canvas.connectors = new Map();
    expect(events).toHaveLength(before + 1);
    expect(last(events)?.kind).toBe("clear");
    expect(canvas.canUndo()).toBe(false);
  });
});

describe("undo/redo guards", () => {
  it("does nothing, and says why, while a gesture is in flight", () => {
    const { canvas, events } = makeCanvas();
    const c1 = wire("w1", "a", "b");
    canvas.connectors.set("w1", c1);
    canvas.pushUndo({ type: "add_edge", connector: c1 });
    canvas.input = { isGestureActive: () => true } as never;
    expect(canvas.undo()).toBe(false);
    expect(canvas.connectors.size).toBe(1);
    expect(last(events)?.kind).toBe("busy");
    canvas.input = { isGestureActive: () => false } as never;
    expect(canvas.undo()).toBe(true);
  });

  it("empty stacks report instead of failing silently, and a new edit clears redo", () => {
    const { canvas, events } = makeCanvas();
    expect(canvas.undo()).toBe(false);
    expect(last(events)?.kind).toBe("empty-undo");
    expect(canvas.redo()).toBe(false);
    expect(last(events)?.kind).toBe("empty-redo");

    const c1 = wire("w1", "a", "b");
    canvas.connectors.set("w1", c1);
    canvas.pushUndo({ type: "add_edge", connector: c1 });
    canvas.undo();
    expect(canvas.canRedo()).toBe(true);
    canvas.pushUndo({ type: "move_node", nodeId: "a", from: { x: 0, y: 0 }, to: { x: 5, y: 5 } });
    expect(canvas.canRedo()).toBe(false);
  });

  it("history is capped; the oldest step is the one dropped", () => {
    const { canvas } = makeCanvas();
    for (let i = 0; i < MAX_HISTORY + 5; i++) {
      canvas.pushUndo({ type: "move_node", nodeId: "a", from: { x: i, y: 0 }, to: { x: i + 1, y: 0 } });
    }
    expect(depth(canvas)).toBe(MAX_HISTORY);
  });

  it("a wire whose node vanished is swept instead of left dangling", () => {
    const { canvas } = makeCanvas();
    const c1 = wire("w1", "a", "b");
    canvas.pushUndo({ type: "delete_edge", connector: c1 });
    canvas.nodes.delete("b");
    canvas.undo();
    expect(canvas.connectors.size).toBe(0);
  });

  it("a step that throws is dropped, reported, and leaves the stack usable", () => {
    const { canvas, events } = makeCanvas();
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    canvas.applyConfigPatches = () => { throw new Error("bad patch"); };
    canvas.pushUndo({ type: "config_patch", configs: [{ nodeId: "a", keys: { k: { before: null, after: "1" } } }] });
    expect(canvas.undo()).toBe(false);
    expect(last(events)?.kind).toBe("failed");
    expect(depth(canvas)).toBe(0);
    spy.mockRestore();
  });
});

describe("selection after undo/redo", () => {
  it("selects what the step touched and drops references to nodes that no longer exist", () => {
    const { canvas } = makeCanvas();
    const a = canvas.nodes.get("a")!;
    canvas.selectedNodes = new Set(["a"]);
    canvas.selectedNode = a;
    canvas.deleteSelected();
    expect(canvas.selectedNodes.size).toBe(0);
    canvas.undo();
    expect([...canvas.selectedNodes]).toEqual(["a"]);
    expect(canvas.selectedNode).toBe(a);
    canvas.redo();
    expect(canvas.selectedNodes.size).toBe(0);
    expect(canvas.selectedNode).toBeNull();
  });
});

describe("node property edits", () => {
  it("rapid edits to one node merge into one step; undo restores the original, redo the final", () => {
    const { canvas } = makeCanvas();
    const a = canvas.nodes.get("a")!;
    canvas.beginNodeEdit(a);
    for (const v of ["h", "he", "hel", "hello"]) {
      a.data.config["prompt"] = v;
      canvas.commitNodeEdit(a);
      vi.advanceTimersByTime(100);
    }
    expect(depth(canvas)).toBe(1);
    canvas.undo();
    expect("prompt" in a.data.config).toBe(false);
    canvas.redo();
    expect(a.data.config["prompt"]).toBe("hello");
  });

  it("edge case: a pause starts a new step, and typing back to the original leaves no step at all", () => {
    const { canvas } = makeCanvas();
    const a = canvas.nodes.get("a")!;
    canvas.beginNodeEdit(a);
    a.data.config["k"] = "one"; canvas.commitNodeEdit(a);
    vi.advanceTimersByTime(COALESCE_MS + 1);
    a.data.config["k"] = "two"; canvas.commitNodeEdit(a);
    expect(depth(canvas)).toBe(2);
    canvas.undo();
    expect(a.data.config["k"]).toBe("one");

    const b = canvas.nodes.get("b")!;
    canvas.beginNodeEdit(b);
    b.data.config["x"] = "1"; canvas.commitNodeEdit(b);
    delete b.data.config["x"]; canvas.commitNodeEdit(b);
    expect(depth(canvas)).toBe(1);
  });

  it("rename and enable/disable are one undoable step each, with readable labels", () => {
    const { canvas } = makeCanvas();
    const a = canvas.nodes.get("a")!;
    canvas.editNode(a, () => { a.data.name = "Fetch"; });
    expect(canvas.undoMgr.state().undoLabel).toBe('Rename "Fetch"');
    canvas.editNode(a, () => { a.disabled = true; });
    expect(canvas.undoMgr.state().undoLabel).toBe('Disable "Fetch"');
    canvas.undo();
    expect(a.disabled).toBe(false);
    expect(a.data.disabled).toBe(false);
    canvas.undo();
    expect(a.data.name).toBe("Node a");
    canvas.redo(); canvas.redo();
    expect(a.data.name).toBe("Fetch");
    expect(a.disabled).toBe(true);
  });

  it("an edit that prunes wires restores them on undo", () => {
    const { canvas } = makeCanvas();
    const a = canvas.nodes.get("a")!;
    const w = wire("w1", "b", "a");
    canvas.beginNodeEdit(a);
    a.data.config["mode"] = "x";
    canvas.commitNodeEdit(a, [w]);
    canvas.undo();
    expect(canvas.connectors.get("w1")).toBe(w);
    canvas.redo();
    expect(canvas.connectors.has("w1")).toBe(false);
  });
});

describe("wiring restores config exactly", () => {
  it("undoing a connection puts back the expression the person had typed, not a blank", () => {
    const { canvas } = makeCanvas(["src", "ai"]);
    const ai = canvas.nodes.get("ai")!;
    (ai.data as { node_type_id: string }).node_type_id = "ai_prompt";
    ai.data.config["attachments_expr"] = "{{Typed.output}}";
    (canvas as unknown as { input: unknown }).input = undefined;

    expect(canvas.connectPorts("src", "output", "ai", "attachments")).toBe(true);
    expect(ai.data.config["attachments_expr"]).not.toBe("{{Typed.output}}");

    canvas.undo();
    expect(ai.data.config["attachments_expr"]).toBe("{{Typed.output}}");
    canvas.redo();
    expect(ai.data.config["attachments_expr"]).not.toBe("{{Typed.output}}");
  });
});

describe("credential edits", () => {
  it("picking a credential is its own undoable step with a readable label", () => {
    const { canvas } = makeCanvas();
    const a = canvas.nodes.get("a")!;
    canvas.beginNodeEdit(a);
    a.data.credentials["api_key"] = "cred_1";
    canvas.commitNodeEdit(a);
    expect(depth(canvas)).toBe(1);
    expect(canvas.undoMgr.state().undoLabel).toBe('Change credential on "Node a"');

    canvas.undo();
    expect(a.data.credentials).toEqual({});
    canvas.redo();
    expect(a.data.credentials).toEqual({ api_key: "cred_1" });
  });

  it("a credential changed together with a setting is undone together with it", () => {
    const { canvas } = makeCanvas();
    const a = canvas.nodes.get("a")!;
    a.data.credentials["api_key"] = "old";
    a.data.config["model"] = "m1";
    canvas.beginNodeEdit(a);
    a.data.credentials["api_key"] = "new";
    canvas.commitNodeEdit(a);
    a.data.config["model"] = "m2";
    canvas.commitNodeEdit(a);
    expect(depth(canvas)).toBe(1);

    canvas.undo();
    expect(a.data.credentials).toEqual({ api_key: "old" });
    expect(a.data.config["model"]).toBe("m1");
    canvas.redo();
    expect(a.data.credentials).toEqual({ api_key: "new" });
    expect(a.data.config["model"]).toBe("m2");
  });

  it("edge case: clearing a credential restores it, undo keeps the same object, and re-picking the original leaves no step", () => {
    const { canvas } = makeCanvas();
    const a = canvas.nodes.get("a")!;
    a.data.credentials["api_key"] = "keep";
    const held = a.data.credentials;
    canvas.beginNodeEdit(a);
    delete a.data.credentials["api_key"];
    canvas.commitNodeEdit(a);
    canvas.undo();
    expect(a.data.credentials).toBe(held);
    expect(held).toEqual({ api_key: "keep" });

    canvas.beginNodeEdit(a);
    a.data.credentials["api_key"] = "other";
    canvas.commitNodeEdit(a);
    a.data.credentials["api_key"] = "keep";
    canvas.commitNodeEdit(a);
    expect(depth(canvas)).toBe(0);
  });
});
