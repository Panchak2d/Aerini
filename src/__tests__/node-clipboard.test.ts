// @vitest-environment jsdom

import { describe, it, expect, beforeEach } from "vitest";
import type { CanvasNode, CanvasNodeData } from "../canvas/Node";
import type { ConnectorData } from "../canvas/Connector";
import { NODE_IDS } from "../node-ids";
import { PASTE_OFFSET, hasClipboard, holdClipboard, pasteOrigin, planPaste, snapshotSelection } from "../canvas/node-clipboard";

function data(id: string, name: string, over: Partial<CanvasNodeData> = {}): CanvasNodeData {
  return {
    id, name, node_type_id: "http_request", node_type: "action", config: {}, credentials: { token: "secret-ref" },
    position: { x: 100, y: 200 }, ports: { inputs: [], outputs: [] }, input_schema: {}, output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 }, fallback_node: null, dynamic_ports: false, ...over,
  };
}

const node = (d: CanvasNodeData, width = 220, height = 80): CanvasNode => ({ data: d, width, height }) as unknown as CanvasNode;

const edge = (id: string, from: string, to: string): { data: ConnectorData } => ({
  data: { id, from_node: from, from_port: "out", to_node: to, to_port: "in", condition: null, on_success: null, on_failure: null },
});

let nextId = 0;
const newNodeId = (isNote: boolean): string => `${isNote ? "note" : "node"}_new${nextId++}`;
const newEdgeId = (): string => `e_new${nextId++}`;

beforeEach(() => { holdClipboard(null); nextId = 0; });

describe("snapshotSelection", () => {
  it("keeps wires inside the selection, drops wires to outside nodes, strips credentials, and measures bounds", () => {
    const a = node(data("a", "A", { position: { x: 0, y: 0 } }));
    const b = node(data("b", "B", { position: { x: 300, y: 100 } }));
    const snap = snapshotSelection([a, b], [edge("e1", "a", "b"), edge("e2", "b", "outside"), edge("e3", "outside", "a")])!;
    expect(snap.edges.map(e => e.id)).toEqual(["e1"]);
    expect(snap.nodes.every(n => Object.keys(n.credentials).length === 0)).toBe(true);
    expect(a.data.credentials).toEqual({ token: "secret-ref" });
    expect(snap.bounds).toEqual({ x: 0, y: 0, width: 520, height: 180 });
  });

  it("returns null for an empty selection", () => {
    expect(snapshotSelection([], [])).toBeNull();
  });
});

describe("planPaste", () => {
  it("gives copies fresh ids, unique names and shifted positions, and rewires only between copies", () => {
    const a = node(data("a", "A", { fallback_node: "b", config: { url: "x" } }));
    const b = node(data("b", "B", { fallback_node: "outside", config: { url: "{{A.body}}" } }));
    const snap = snapshotSelection([a, b], [edge("e1", "a", "b")])!;
    const plan = planPaste(snap, { x: 150, y: 250 }, ["A", "B"], newNodeId, newEdgeId);
    expect(plan.nodes.map(n => n.name)).toEqual(["A 2", "B 2"]);
    expect(plan.nodes.map(n => n.id)).toEqual(["node_new0", "node_new1"]);
    expect(plan.nodes[0].position).toEqual({ x: 150, y: 250 });
    expect(plan.nodes[0].fallback_node).toBe("node_new1");
    expect(plan.nodes[1].fallback_node).toBeNull();
    expect(plan.nodes[1].config["url"]).toBe("{{A 2.body}}");
    expect(plan.edges).toHaveLength(1);
    expect(plan.edges[0]).toMatchObject({ from_node: "node_new0", to_node: "node_new1" });
    expect(snap.nodes[1].config["url"]).toBe("{{A.body}}");
  });

  it("does not rewrite a reference twice when a new name equals another copied node's old name", () => {
    const a = node(data("a", "A"));
    const a2 = node(data("a2", "A 2"));
    const user = node(data("u", "User", { config: { v: "{{A.x}} {{A 2.x}}" } }));
    const snap = snapshotSelection([a, a2, user], [])!;
    const plan = planPaste(snap, { x: 0, y: 0 }, ["A"], newNodeId, newEdgeId);
    expect(plan.nodes.map(n => n.name)).toEqual(["A 2", "A 3", "User"]);
    expect(plan.nodes[2].config["v"]).toBe("{{A 2.x}} {{A 3.x}}");
  });

  it("keeps names unchanged when they are free and issues note ids for notes", () => {
    const frame = node(data("f", "Group", { node_type_id: NODE_IDS.NOTE }));
    const plan = planPaste(snapshotSelection([frame], [])!, { x: 0, y: 0 }, [], newNodeId, newEdgeId);
    expect(plan.nodes[0].name).toBe("Group");
    expect(plan.nodes[0].id).toBe("note_new0");
  });
});

describe("pasteOrigin", () => {
  const bounds = { x: 100, y: 100, width: 200, height: 80 };

  it("centres the pasted block on the pointer", () => {
    expect(pasteOrigin(bounds, { x: 500, y: 400 })).toEqual({ x: 400, y: 360 });
  });

  it("offsets further on each paste without a pointer", () => {
    const snap = snapshotSelection([node(data("a", "A"))], [])!;
    holdClipboard(snap);
    expect(pasteOrigin(bounds, null)).toEqual({ x: 100 + PASTE_OFFSET, y: 100 + PASTE_OFFSET });
    planPaste(snap, { x: 0, y: 0 }, [], newNodeId, newEdgeId);
    expect(pasteOrigin(bounds, null)).toEqual({ x: 100 + PASTE_OFFSET * 2, y: 100 + PASTE_OFFSET * 2 });
  });
});

describe("clipboard state", () => {
  it("reports whether something is held", () => {
    expect(hasClipboard()).toBe(false);
    holdClipboard(snapshotSelection([node(data("a", "A"))], []));
    expect(hasClipboard()).toBe(true);
  });
});
