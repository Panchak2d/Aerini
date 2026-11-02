/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { CanvasNode } from "../canvas/Node";
import { UndoManager } from "../canvas/UndoManager";
import { validateNodeName, uniqueNodeName, renameReferences } from "../node-names";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

describe("validateNodeName", () => {
  it("refuses characters the expression parser splits on and suggests a replacement", () => {
    expect(validateNodeName("Fetch v1.2")).toBe(`Names can't contain '.'. Try "Fetch v1-2".`);
    for (const bad of ["a{b", "a}b", "a,b", 'a"b', "a'b", "$x"]) expect(validateNodeName(bad)).not.toBeNull();
  });

  it("allows built-in style names with a spaced '(' but refuses ones read as a function call or unbalanced", () => {
    expect(validateNodeName("Code (JS)")).toBeNull();
    expect(validateNodeName("Claude (Anthropic)")).toBeNull();
    expect(validateNodeName("Code(JS)")).not.toBeNull();
    expect(validateNodeName("Code (JS")).not.toBeNull();
    expect(validateNodeName("   ")).not.toBeNull();
  });
});

describe("uniqueNodeName", () => {
  it("returns a free name unchanged and numbers a taken one", () => {
    expect(uniqueNodeName("Notify Me", ["Other"])).toBe("Notify Me");
    expect(uniqueNodeName("Notify Me", ["Notify Me"])).toBe("Notify Me 2");
  });

  it("continues an existing numbering only when the unnumbered name exists", () => {
    expect(uniqueNodeName("Notify Me 2", ["Notify Me", "Notify Me 2"])).toBe("Notify Me 3");
    expect(uniqueNodeName("Imagen 4", ["Imagen 4"])).toBe("Imagen 4 2");
  });
});

describe("renameReferences", () => {
  it("rewrites references inside {{}} including function arguments, and nothing outside them", () => {
    const text = "Hi {{Old.output.x}} and {{ upper(Old.output.y) }} {{Other.output}} Old.output";
    expect(renameReferences(text, "Old", "New"))
      .toBe("Hi {{New.output.x}} and {{ upper(New.output.y) }} {{Other.output}} Old.output");
  });

  it("leaves quoted literals and longer names that share a prefix alone", () => {
    expect(renameReferences('{{ replace("Old.output.x", Old.output.y) }}', "Old", "New"))
      .toBe('{{ replace("Old.output.x", New.output.y) }}');
    expect(renameReferences("{{Fetch 2.output.x}}", "Fetch", "Get")).toBe("{{Fetch 2.output.x}}");
    expect(renameReferences("{{Code (JS).output.x}}", "Code (JS)", "Run")).toBe("{{Run.output.x}}");
  });

  it("walks arrays and objects, and returns the same reference when nothing changed", () => {
    const cfg = { tools: [{ body: "{{Old.output}}" }], n: 5 };
    expect(renameReferences(cfg, "Old", "New")).toEqual({ tools: [{ body: "{{New.output}}" }], n: 5 });
    const same = { a: "plain", b: ["x"] };
    expect(renameReferences(same, "Old", "New")).toBe(same);
  });
});

function node(id: string, config: Record<string, unknown> = {}): CanvasNode {
  return new CanvasNode({
    id, node_type_id: "http_request", node_type: "action", name: `Node ${id}`,
    config, credentials: {}, position: { x: 0, y: 0 },
    ports: { inputs: [{ id: "input", label: "Input", position: "left" }], outputs: [{ id: "output", label: "Output", position: "right" }] },
    input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: false,
  });
}

function makeCanvas() {
  const canvas = Object.create(Canvas.prototype) as Canvas & Record<string, unknown>;
  const nodes = [
    node("a"),
    node("b", { body: "{{Node a.output.x}}", keep: 3 }),
    node("c", { tools: [{ text: "{{Node a.output}}" }] }),
  ];
  canvas.nodes = new Map(nodes.map(n => [n.data.id, n]));
  canvas.connectors = new Map();
  canvas.selectedNodes = new Set();
  canvas.selectedNode = null;
  canvas.selectedConn = null;
  canvas.onCanvasChanged = vi.fn();
  canvas.onWarn = vi.fn();
  canvas.undoMgr = new UndoManager(canvas);
  return canvas;
}

const depth = (c: Canvas) => (c.undoMgr as unknown as { stack: unknown[] }).stack.length;

describe("Canvas.renameNode", () => {
  it("renames and rewrites every reference as one undo step", () => {
    const canvas = makeCanvas();
    const res = canvas.renameNode(canvas.nodes.get("a")!, "Summarize");
    expect(res).toEqual({ name: "Summarize", message: "Updated references in 2 other nodes." });
    expect(canvas.nodes.get("b")!.data.config).toEqual({ body: "{{Summarize.output.x}}", keep: 3 });
    expect(canvas.nodes.get("c")!.data.config).toEqual({ tools: [{ text: "{{Summarize.output}}" }] });
    expect(depth(canvas)).toBe(1);

    canvas.undo();
    expect(canvas.nodes.get("a")!.data.name).toBe("Node a");
    expect(canvas.nodes.get("b")!.data.config).toEqual({ body: "{{Node a.output.x}}", keep: 3 });
    expect(canvas.nodes.get("c")!.data.config).toEqual({ tools: [{ text: "{{Node a.output}}" }] });
  });

  it("refuses an unusable name and suffixes a taken one without touching references", () => {
    const canvas = makeCanvas();
    const a = canvas.nodes.get("a")!;
    expect(canvas.renameNode(a, "Fetch v1.2").name).toBe("Node a");
    expect(depth(canvas)).toBe(0);

    const res = canvas.renameNode(canvas.nodes.get("b")!, "Node c");
    expect(res.name).toBe("Node c 2");
    expect(res.message).toContain('"Node c" is already used');
  });
});
