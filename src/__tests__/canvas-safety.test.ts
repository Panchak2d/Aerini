/**
 * @vitest-environment jsdom
 *
 * DEVIATION NOTE: a real `Canvas` cannot be constructed in this test
 * environment — its constructor calls `new ResizeObserver(...)` (not
 * implemented by jsdom) and `canvasEl.getContext("2d")` (requires the
 * optional `canvas` npm package, not a project dependency). Matching the
 * precedent already established in workflow-manager.test.ts, these tests
 * exercise the real fixed methods directly instead of going through a full
 * Canvas instance:
 *   - `InputHandler` has no DOM dependency in its constructor, so it is
 *     constructed for real; the `Canvas` it depends on is a minimal
 *     duck-typed stub covering only what InputHandler.onKey's Escape
 *     branch touches.
 *   - `Canvas.prototype.deleteSelected` is called with `.call(fakeThis)`
 *     against a duck-typed `this` — the real prototype method, not a
 *     reimplementation, exercised without needing the rest of Canvas's
 *     constructor to run.
 *   - `serialize()` is a plain function and needs no DOM at all.
 */
import { describe, it, expect, vi } from "vitest";
import { InputHandler } from "../canvas/InputHandler";
import { Canvas } from "../canvas/Canvas";
import { Connector, PendingConnector } from "../canvas/Connector";
import { CanvasNode } from "../canvas/Node";
import type { UndoAction } from "../canvas/UndoManager";
import { serialize } from "../canvas/CanvasSerializer";
import { checkDangerousNodes } from "../validation";

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
// T1-4 — Escape while repositioning/detaching a wire must restore it, not
// permanently delete it.
// ---------------------------------------------------------------------------

describe("InputHandler — Escape during wire reconnect (T1-4)", () => {
  function makeFakeCanvas() {
    const connectors = new Map<string, Connector>();
    const injectCalls: string[] = [];
    const canvas = {
      el: { classList: { add: vi.fn(), remove: vi.fn() }, style: { cursor: "" } },
      connectors,
      pendingInsert: null,
      insertGhost: null,
      _pendingInputWireDrop: null,
      clearSelection: vi.fn(),
      injectDynamicPortExpr: vi.fn((c: Connector) => injectCalls.push(c.data.id)),
    };
    return { canvas, injectCalls };
  }

  it("restores the grabbed connector and re-injects its dynamic-port expression", () => {
    const { canvas, injectCalls } = makeFakeCanvas();
    const input = new InputHandler(canvas as unknown as Canvas);
    const conn = makeConnector("e1", "n1", "n2");

    // Simulate onDown having grabbed an existing wire's endpoint: removed
    // from connectors, tracked as reconnEdge, mid-drag.
    input.reconnEdge = { conn };
    input.pendingConn = new PendingConnector("n1", "output", 0, 0);

    input.onKey(new KeyboardEvent("keydown", { key: "Escape" }));

    expect(canvas.connectors.get("e1")).toBe(conn);
    expect(injectCalls).toEqual(["e1"]);
    expect(input.reconnEdge).toBeNull();
    expect(input.pendingConn).toBeNull();
  });

  it("is a no-op when no reconnect was in progress (plain Escape)", () => {
    const { canvas, injectCalls } = makeFakeCanvas();
    const input = new InputHandler(canvas as unknown as Canvas);

    input.onKey(new KeyboardEvent("keydown", { key: "Escape" }));

    expect(injectCalls).toEqual([]);
    expect(input.reconnEdge).toBeNull();
    expect(canvas.clearSelection).toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// T1-14 (part 1) — deleteSelected must partition connectors per node, so
// undoing one node from a multi-node delete can't resurrect a connector
// whose other endpoint is a still-deleted sibling.
// ---------------------------------------------------------------------------

describe("Canvas.deleteSelected — per-node undo partitioning (T1-14)", () => {
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
// T1-14 (part 2) — serialize() must never emit an edge with a missing
// endpoint, regardless of how it became dangling.
// ---------------------------------------------------------------------------

describe("serialize — dangling edge filter (T1-14)", () => {
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
// T1-15 — checkDangerousNodes must be scoped to the exact node set passed
// in, not any wider notion of "the whole workflow".
// ---------------------------------------------------------------------------

describe("checkDangerousNodes — scoped to the given node set (T1-15)", () => {
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
