/**
 * DEVIATION NOTE: `WorkflowManager` (from workflow-manager.ts) requires a
 * live Canvas, DOM, localStorage, and Tauri IPC — not testable in unit
 * isolation. The plan's intent is to verify workflow serialization roundtrip,
 * which lives in `CanvasSerializer.ts`. Tests are written against
 * `serialize` / `deserialize` directly, which is where that logic resides.
 */
import { describe, it, expect, vi } from "vitest";
import { serialize, deserialize, DEFAULT_CHAT_SETTINGS } from "../canvas/CanvasSerializer";
import { CanvasNode } from "../canvas/Node";
import { Connector } from "../canvas/Connector";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level)
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

// ---------------------------------------------------------------------------
// Minimal test object factories
// ---------------------------------------------------------------------------

function makeNode(id: string, x: number, y: number, typeId = "http_request"): CanvasNode {
  return new CanvasNode({
    id,
    node_type_id: typeId,
    node_type: "action",
    name: `Node ${id}`,
    config: { url: "https://example.com" },
    credentials: {},
    position: { x, y },
    ports: {
      inputs:  [{ id: "input",  label: "Input",  position: "left"  }],
      outputs: [{ id: "output", label: "Output", position: "right" }],
    },
    input_schema:  {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  });
}

function makeConnector(id: string, fromNode: string, toNode: string): Connector {
  return new Connector({
    id,
    from_node:  fromNode,
    from_port:  "output",
    to_node:    toNode,
    to_port:    "input",
    condition:  null,
    on_success: null,
    on_failure: null,
  });
}

// ---------------------------------------------------------------------------
// serialize — output contract
// ---------------------------------------------------------------------------

describe("serialize — empty canvas", () => {
  const emptyNodes = new Map<string, CanvasNode>();
  const emptyEdges = new Map<string, Connector>();

  it("produces valid JSON", () => {
    const json = serialize("wf_test", "Test", emptyNodes, emptyEdges);
    expect(() => JSON.parse(json)).not.toThrow();
  });

  it("has empty nodes and edges arrays", () => {
    const doc = JSON.parse(serialize("wf_test", "Test", emptyNodes, emptyEdges));
    expect(doc.nodes).toEqual([]);
    expect(doc.edges).toEqual([]);
  });

  it("contains schema_version 1.0", () => {
    const doc = JSON.parse(serialize("wf_test", "Test", emptyNodes, emptyEdges));
    expect(doc.schema_version).toBe("1.0");
  });

  it("serialises the workflow id and name", () => {
    const doc = JSON.parse(serialize("wf_abc", "My Workflow", emptyNodes, emptyEdges));
    expect(doc.id).toBe("wf_abc");
    expect(doc.name).toBe("My Workflow");
  });
});

describe("serialize — 2-node + 1-connector canvas", () => {
  const n1 = makeNode("n1", 100, 200);
  const n2 = makeNode("n2", 400, 200);
  const c1 = makeConnector("e1", "n1", "n2");

  const nodes = new Map([["n1", n1], ["n2", n2]]);
  const edges = new Map([["e1", c1]]);

  it("serialises both nodes", () => {
    const doc = JSON.parse(serialize("wf_test", "Test", nodes, edges));
    expect(doc.nodes).toHaveLength(2);
  });

  it("serialises the edge", () => {
    const doc = JSON.parse(serialize("wf_test", "Test", nodes, edges));
    expect(doc.edges).toHaveLength(1);
  });

  it("excludes NOTE nodes from serialized output", () => {
    const noteNode = makeNode("note1", 50, 50, "note");
    const allNodes = new Map([["n1", n1], ["n2", n2], ["note1", noteNode]]);
    const doc = JSON.parse(serialize("wf_test", "Test", allNodes, edges));
    // note node must be excluded
    expect(doc.nodes.every((n: { node_type_id: string }) => n.node_type_id !== "note")).toBe(true);
    expect(doc.nodes).toHaveLength(2);
  });
});

// ---------------------------------------------------------------------------
// roundtrip: serialize → JSON → deserialize
// ---------------------------------------------------------------------------

describe("deserialize — roundtrip", () => {
  const n1 = makeNode("n1", 100, 200);
  const n2 = makeNode("n2", 400, 350);
  const c1 = makeConnector("e1", "n1", "n2");
  const nodes = new Map([["n1", n1], ["n2", n2]]);
  const edges = new Map([["e1", c1]]);

  const json = serialize("wf_rt", "Roundtrip", nodes, edges);
  const rt   = deserialize(json);

  it("restores correct node count", () => {
    expect(rt.nodes.size).toBe(2);
  });

  it("restores node positions", () => {
    const rn1 = rt.nodes.get("n1");
    const rn2 = rt.nodes.get("n2");
    expect(rn1?.data.position).toEqual({ x: 100, y: 200 });
    expect(rn2?.data.position).toEqual({ x: 400, y: 350 });
  });

  it("restores the connector", () => {
    expect(rt.connectors.size).toBe(1);
    const conn = rt.connectors.get("e1");
    expect(conn?.data.from_node).toBe("n1");
    expect(conn?.data.to_node).toBe("n2");
    expect(conn?.data.from_port).toBe("output");
    expect(conn?.data.to_port).toBe("input");
  });

  it("restores workflow id and name", () => {
    expect(rt.id).toBe("wf_rt");
    expect(rt.name).toBe("Roundtrip");
  });
});

describe("deserialize — defaults", () => {
  const emptyJson = serialize("wf_d", "Defaults", new Map(), new Map());

  it("defaults parallelExecution to false", () => {
    expect(deserialize(emptyJson).parallelExecution).toBe(false);
  });

  it("defaults maxConcurrentNodes to 8", () => {
    expect(deserialize(emptyJson).maxConcurrentNodes).toBe(8);
  });

  it("defaults chatSettings to DEFAULT_CHAT_SETTINGS", () => {
    expect(deserialize(emptyJson).chatSettings).toEqual(DEFAULT_CHAT_SETTINGS);
  });
});

describe("deserialize — parallel_execution flag", () => {
  it("reads parallel_execution: true from JSON", () => {
    const doc = JSON.parse(serialize("wf_p", "Parallel", new Map(), new Map()));
    doc.parallel_execution = true;
    doc.max_concurrent_nodes = 4;
    const rt = deserialize(JSON.stringify(doc));
    expect(rt.parallelExecution).toBe(true);
    expect(rt.maxConcurrentNodes).toBe(4);
  });
});

describe("deserialize — empty canvas JSON", () => {
  it("empty nodes + edges → empty Maps", () => {
    const rt = deserialize(JSON.stringify({ id: "wf_e", name: "Empty", nodes: [], edges: [] }));
    expect(rt.nodes.size).toBe(0);
    expect(rt.connectors.size).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// disabled flag (T1-3): ContextMenu's Disable/Enable toggle must survive
// save (serialize) and load (deserialize) — previously silently dropped.
// ---------------------------------------------------------------------------

describe("serialize — disabled flag", () => {
  it("defaults a fresh node to disabled: false", () => {
    const n1 = makeNode("n1", 0, 0);
    const doc = JSON.parse(serialize("wf_dis", "Dis", new Map([["n1", n1]]), new Map()));
    expect(doc.nodes[0].disabled).toBe(false);
  });

  it("reflects a runtime Disable toggle (mirrors ContextMenu's node.disabled = !node.disabled)", () => {
    const n1 = makeNode("n1", 0, 0);
    n1.disabled = true;
    const doc = JSON.parse(serialize("wf_dis", "Dis", new Map([["n1", n1]]), new Map()));
    expect(doc.nodes[0].disabled).toBe(true);
  });
});

describe("deserialize — disabled flag roundtrip", () => {
  it("restores disabled: true through a full serialize → deserialize cycle", () => {
    const n1 = makeNode("n1", 0, 0);
    n1.disabled = true;
    const json = serialize("wf_dis_rt", "Dis RT", new Map([["n1", n1]]), new Map());
    const rt = deserialize(json);
    expect(rt.nodes.get("n1")?.disabled).toBe(true);
  });

  it("defaults to disabled: false when the field is absent (pre-existing saved workflows)", () => {
    const legacyDoc = {
      id: "wf_legacy", name: "Legacy",
      nodes: [{
        id: "n1", node_type_id: "http_request", node_type: "action", name: "Node n1",
        config: {}, credentials: {}, position: { x: 0, y: 0 },
        ports: { inputs: [], outputs: [] }, input_schema: {}, output_schema: {},
        retry: { max_attempts: 1, backoff_ms: 500 }, fallback_node: null,
        dynamic_ports: false,
        // no `disabled` key — simulates a workflow saved before this field existed
      }],
      edges: [],
    };
    const rt = deserialize(JSON.stringify(legacyDoc));
    expect(rt.nodes.get("n1")?.disabled).toBe(false);
  });
});
