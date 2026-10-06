/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { InputHandler } from "../canvas/InputHandler";
import { Canvas } from "../canvas/Canvas";
import { Connector } from "../canvas/Connector";
import { CanvasNode } from "../canvas/Node";
import type { UndoAction } from "../canvas/UndoManager";
import { serialize, deserialize, registerNodeDescriptors } from "../canvas/CanvasSerializer";
import { checkDangerousNodes } from "../validation";
import type { NodeDescriptor } from "../ipc/workflow";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level)
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeNode(id: string, typeId = "http_request"): CanvasNode {
  return new CanvasNode({
    id,
    node_type_id: typeId,
    node_type: "action",
    name: `Node ${id}`,
    config: {},
    credentials: {},
    position: { x: 0, y: 0 },
    ports: {
      inputs:  [{ id: "input",  label: "Input",  position: "left"  }],
      outputs: [{ id: "output", label: "Output", position: "right" }],
    },
    input_schema: {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  });
}

function makeConnector(id: string, fromNode: string, toNode: string): Connector {
  return new Connector({
    id, from_node: fromNode, from_port: "output", to_node: toNode, to_port: "input",
    condition: null, on_success: null, on_failure: null,
  });
}

// ---------------------------------------------------------------------------
// deleteSelected must partition connectors per node, so
// undoing one node from a multi-node delete can't resurrect a connector
// whose other endpoint is a still-deleted sibling.
// ---------------------------------------------------------------------------

describe("Canvas.deleteSelected — per-node undo partitioning", () => {
  it("each delete_node action only carries connectors touching that node", () => {
    const nA = { data: { id: "A" } } as unknown as CanvasNode;
    const nB = { data: { id: "B" } } as unknown as CanvasNode;
    const cAB = makeConnector("e_ab", "A", "B");          // touches both deleted nodes
    const cAX = makeConnector("e_ax", "A", "X");          // touches only A (X survives)

    const pushed: UndoAction[] = [];
    const fakeThis = {
      selectedNodes: new Set(["A", "B"]),
      nodes: new Map<string, CanvasNode>([["A", nA], ["B", nB]]),
      connectors: new Map<string, Connector>([["e_ab", cAB], ["e_ax", cAX]]),
      selectedConn: null,
      clearDynamicPortExpr: vi.fn(),
      pushUndo: (a: UndoAction) => pushed.push(a),
      clearSelection: vi.fn(),
      onCanvasChanged: null,
    };

    Canvas.prototype.deleteSelected.call(fakeThis as unknown as Canvas);

    const deleteActions = pushed.filter(a => a.type === "delete_node") as Extract<UndoAction, { type: "delete_node" }>[];
    expect(deleteActions).toHaveLength(2);

    const forA = deleteActions.find(a => a.node.data.id === "A")!;
    const forB = deleteActions.find(a => a.node.data.id === "B")!;

    // A's action must not carry a connector that only touches B (none exist
    // here, but it also must not carry more than what touches A).
    expect(forA.connectors.map(c => c.data.id).sort()).toEqual(["e_ab", "e_ax"]);
    // B's action must NOT carry e_ax — that connector never touched B. Before
    // the fix, both actions shared one array containing every deleted
    // connector regardless of which node it actually touched.
    expect(forB.connectors.map(c => c.data.id).sort()).toEqual(["e_ab"]);
  });
});

// ---------------------------------------------------------------------------
// serialize() must never emit an edge with a missing
// endpoint, regardless of how it became dangling.
// ---------------------------------------------------------------------------

describe("serialize — dangling edge filter", () => {
  it("excludes an edge whose from_node is absent from the node map", () => {
    const n2 = makeNode("n2");
    const nodes = new Map([["n2", n2]]); // "n1" deliberately absent
    const edges = new Map([["e1", makeConnector("e1", "n1", "n2")]]);

    const doc = JSON.parse(serialize("wf_test", "Test", nodes, edges));
    expect(doc.edges).toHaveLength(0);
  });

  it("excludes an edge whose to_node is absent from the node map", () => {
    const n1 = makeNode("n1");
    const nodes = new Map([["n1", n1]]); // "n2" deliberately absent
    const edges = new Map([["e1", makeConnector("e1", "n1", "n2")]]);

    const doc = JSON.parse(serialize("wf_test", "Test", nodes, edges));
    expect(doc.edges).toHaveLength(0);
  });

  it("still includes a normal edge between two present, non-NOTE nodes", () => {
    const n1 = makeNode("n1");
    const n2 = makeNode("n2");
    const nodes = new Map([["n1", n1], ["n2", n2]]);
    const edges = new Map([["e1", makeConnector("e1", "n1", "n2")]]);

    const doc = JSON.parse(serialize("wf_test", "Test", nodes, edges));
    expect(doc.edges).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// deserialize must not trust an imported node/edge id verbatim: a bad-format
// or duplicate id is replaced with the same generated scheme already used
// for an absent id, and a dangling edge is still dropped — both now
// reported via importWarnings instead of failing silently.
// ---------------------------------------------------------------------------

describe("deserialize — imported id validation", () => {
  it("normal case: well-formed, unique ids pass through unchanged with no warnings", () => {
    const doc = { id: "wf1", name: "Test", nodes: [{ id: "n1" }, { id: "n2" }], edges: [{ id: "e1", from_node: "n1", to_node: "n2" }] };
    const rt = deserialize(JSON.stringify(doc));
    expect([...rt.nodes.keys()]).toEqual(["n1", "n2"]);
    expect([...rt.connectors.keys()]).toEqual(["e1"]);
    expect(rt.importWarnings).toEqual([]);
  });

  it("edge case: a node id containing HTML metacharacters is replaced and reported", () => {
    const doc = { id: "wf1", name: "Test", nodes: [{ id: '"><img src=x>' }], edges: [] };
    const rt = deserialize(JSON.stringify(doc));
    const ids = [...rt.nodes.keys()];
    expect(ids).toHaveLength(1);
    expect(ids[0]).not.toBe('"><img src=x>');
    expect(ids[0]).toMatch(/^node_/);
    expect(rt.importWarnings.some(w => w.includes("invalid or duplicate"))).toBe(true);
  });

  it("edge case: two nodes sharing the same id both survive — the second is re-keyed instead of overwriting the first", () => {
    const doc = { id: "wf1", name: "Test", nodes: [{ id: "dup", name: "First" }, { id: "dup", name: "Second" }], edges: [] };
    const rt = deserialize(JSON.stringify(doc));
    expect(rt.nodes.size).toBe(2);
    expect(rt.nodes.get("dup")?.data.name).toBe("First");
    expect([...rt.nodes.values()].map(n => n.data.name)).toEqual(["First", "Second"]);
    expect(rt.importWarnings.some(w => w.includes("invalid or duplicate"))).toBe(true);
  });

  it("a genuinely absent id is still generated silently, same as before — not counted as a warning", () => {
    const doc = { id: "wf1", name: "Test", nodes: [{ name: "No id" }], edges: [] };
    const rt = deserialize(JSON.stringify(doc));
    expect(rt.nodes.size).toBe(1);
    expect(rt.importWarnings).toEqual([]);
  });

  it("an edge referencing a missing node is still dropped — now reported instead of silent", () => {
    const doc = { id: "wf1", name: "Test", nodes: [{ id: "n1" }], edges: [{ id: "e1", from_node: "n1", to_node: "ghost" }] };
    const rt = deserialize(JSON.stringify(doc));
    expect(rt.connectors.size).toBe(0);
    expect(rt.importWarnings.some(w => w.includes("dropped"))).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// checkDangerousNodes must be scoped to the exact node set passed
// in, not any wider notion of "the whole workflow".
// ---------------------------------------------------------------------------

describe("checkDangerousNodes — scoped to the given node set", () => {
  function fakeNode(id: string, typeId: string): CanvasNode {
    return { data: { id, node_type_id: typeId, name: id } } as unknown as CanvasNode;
  }

  it("prompts when a dangerous node is present in the given set", async () => {
    const confirm = vi.fn().mockResolvedValue(true);
    const nodes = [fakeNode("n1", "shell_exec")];
    const ok = await checkDangerousNodes("wf1", nodes, new Set(), confirm);
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(ok).toBe(true);
  });

  it("does not prompt for a dangerous node that is outside the given set (ancestor-subgraph scoping)", async () => {
    const confirm = vi.fn().mockResolvedValue(true);
    // Simulates a single-node test run whose ancestor subgraph contains no
    // dangerous node, even though the full canvas (not passed here) might.
    const subgraphNodes = [fakeNode("n1", "http_request")];
    const ok = await checkDangerousNodes("wf1", subgraphNodes, new Set(), confirm);
    expect(confirm).not.toHaveBeenCalled();
    expect(ok).toBe(true);
  });

  it("does not re-prompt once approved for the same workflow+node-set", async () => {
    const confirm = vi.fn().mockResolvedValue(true);
    const approved = new Set<string>();
    const nodes = [fakeNode("n1", "code")];
    await checkDangerousNodes("wf1", nodes, approved, confirm);
    await checkDangerousNodes("wf1", nodes, approved, confirm);
    expect(confirm).toHaveBeenCalledTimes(1);
  });

  it("returns false and does not remember approval when the user cancels", async () => {
    const confirm = vi.fn().mockResolvedValue(false);
    const approved = new Set<string>();
    const nodes = [fakeNode("n1", "shell_exec")];
    const ok = await checkDangerousNodes("wf1", nodes, approved, confirm);
    expect(ok).toBe(false);
    expect(approved.size).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// checkDangerousNodes must also prompt for plugin nodes: they get resolved
// credential values merged into their input and have outbound HTTP access by
// design, but sit outside DANGEROUS_NODE_IDS entirely.
// ---------------------------------------------------------------------------

describe("checkDangerousNodes — plugin nodes", () => {
  function fakeNode(id: string, typeId: string): CanvasNode {
    return { data: { id, node_type_id: typeId, name: id } } as unknown as CanvasNode;
  }

  const PLUGIN_DESCRIPTOR: NodeDescriptor = {
    type_id: "my_plugin_node", display_name: "My Plugin", node_type: "action", version: "1",
    input_schema: {}, output_schema: {},
    ports: { inputs: [{ id: "input", label: "In", position: "left" }], outputs: [{ id: "output", label: "Out", position: "right" }] },
    is_plugin: true,
  };

  it("prompts for a node whose descriptor is registered as is_plugin, even though it's not in DANGEROUS_NODE_IDS", async () => {
    registerNodeDescriptors([PLUGIN_DESCRIPTOR]);
    const confirm = vi.fn().mockResolvedValue(true);
    const nodes = [fakeNode("n1", "my_plugin_node")];
    const ok = await checkDangerousNodes("wf1", nodes, new Set(), confirm);
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(ok).toBe(true);
  });

  it("does not prompt for a registered built-in (non-plugin, non-dangerous) node type", async () => {
    registerNodeDescriptors([{ ...PLUGIN_DESCRIPTOR, type_id: "http_request", is_plugin: false }]);
    const confirm = vi.fn().mockResolvedValue(true);
    const nodes = [fakeNode("n1", "http_request")];
    const ok = await checkDangerousNodes("wf1", nodes, new Set(), confirm);
    expect(confirm).not.toHaveBeenCalled();
    expect(ok).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Bug B — Space also toggled #output-drawer, not just the palette, whenever
// a role="button" control (e.g. #drawer-header) still had keyboard focus.
// onKey is bound at `window` (Canvas.ts), so every keydown app-wide reaches
// it; these exercise that global reach directly against real focused DOM
// elements, same real-world path as the reported bug.
// ---------------------------------------------------------------------------

describe("InputHandler — Space palette shortcut vs. focused role=\"button\" controls", () => {
  function makeFakeCanvas(el: HTMLElement) {
    return {
      el,
      onPaletteRequest: vi.fn(),
    };
  }

  it("normal case: Space opens the palette when focus is on the canvas itself", () => {
    document.body.innerHTML = `<div id="fake-canvas" tabindex="0"></div>`;
    const el = document.getElementById("fake-canvas")!;
    el.focus();
    const canvas = makeFakeCanvas(el);
    const input = new InputHandler(canvas as unknown as Canvas);

    input.onKey(new KeyboardEvent("keydown", { code: "Space", key: " " }));
    expect(canvas.onPaletteRequest).toHaveBeenCalledTimes(1);
  });

  it("normal case: Space opens the palette when nothing in particular has focus (document.body)", () => {
    document.body.innerHTML = `<div id="fake-canvas" tabindex="0"></div>`;
    const el = document.getElementById("fake-canvas")!;
    const canvas = makeFakeCanvas(el);
    const input = new InputHandler(canvas as unknown as Canvas);

    expect(document.activeElement).toBe(document.body);
    input.onKey(new KeyboardEvent("keydown", { code: "Space", key: " " }));
    expect(canvas.onPaletteRequest).toHaveBeenCalledTimes(1);
  });

  it("the bug this guards against: Space does NOT also open the palette while a role=\"button\" control (e.g. #drawer-header) has focus", () => {
    document.body.innerHTML = `
      <div id="fake-canvas" tabindex="0"></div>
      <div class="drawer-header" id="drawer-header" role="button" tabindex="0"></div>
    `;
    const el = document.getElementById("fake-canvas")!;
    const header = document.getElementById("drawer-header")!;
    header.focus();
    expect(document.activeElement).toBe(header);

    const canvas = makeFakeCanvas(el);
    const input = new InputHandler(canvas as unknown as Canvas);
    input.onKey(new KeyboardEvent("keydown", { code: "Space", key: " " }));
    expect(canvas.onPaletteRequest).not.toHaveBeenCalled();
  });

  it("related fix, same root cause: Space does not fire the palette while a native <button> has focus either", () => {
    document.body.innerHTML = `
      <div id="fake-canvas" tabindex="0"></div>
      <button id="some-btn">Run</button>
    `;
    const el = document.getElementById("fake-canvas")!;
    document.getElementById("some-btn")!.focus();

    const canvas = makeFakeCanvas(el);
    const input = new InputHandler(canvas as unknown as Canvas);
    input.onKey(new KeyboardEvent("keydown", { code: "Space", key: " " }));
    expect(canvas.onPaletteRequest).not.toHaveBeenCalled();
  });

  it("edge case: Ctrl/Cmd+K still opens the palette regardless of focus (no conflicting binding on any control)", () => {
    document.body.innerHTML = `
      <div id="fake-canvas" tabindex="0"></div>
      <div class="drawer-header" id="drawer-header" role="button" tabindex="0"></div>
    `;
    const el = document.getElementById("fake-canvas")!;
    document.getElementById("drawer-header")!.focus();

    const canvas = makeFakeCanvas(el);
    const input = new InputHandler(canvas as unknown as Canvas);
    input.onKey(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }));
    expect(canvas.onPaletteRequest).toHaveBeenCalledTimes(1);
  });
});
