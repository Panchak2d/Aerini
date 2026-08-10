// @vitest-environment jsdom
 

import { describe, it, expect, vi, beforeEach } from "vitest";
import { serialize, deserialize, DEFAULT_CHAT_SETTINGS } from "../canvas/CanvasSerializer";
import { CanvasNode } from "../canvas/Node";
import { Connector } from "../canvas/Connector";
import { WorkflowManager } from "../workflow-manager";
import { invoke } from "@tauri-apps/api/core";

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

  const json = serialize("wf_rt", "Roundtrip", nodes, edges, undefined, undefined, undefined, ["prod", "webhook"]);
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

  it("restores tags through a full serialize → deserialize cycle", () => {
    expect(rt.tags).toEqual(["prod", "webhook"]);
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

  it("defaults unlimitedDuration to false", () => {
    expect(deserialize(emptyJson).unlimitedDuration).toBe(false);
  });

  it("defaults chatSettings to DEFAULT_CHAT_SETTINGS", () => {
    expect(deserialize(emptyJson).chatSettings).toEqual(DEFAULT_CHAT_SETTINGS);
  });

  it("defaults tags to []", () => {
    expect(deserialize(emptyJson).tags).toEqual([]);
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

describe("serialize/deserialize — unlimited_duration flag", () => {
  it("omits unlimited_duration from JSON when false (sparse-output convention)", () => {
    const doc = JSON.parse(serialize("wf_ud_off", "UD Off", new Map(), new Map(), undefined, undefined, undefined, undefined, false));
    expect(doc.unlimited_duration).toBeUndefined();
  });

  it("restores unlimited_duration: true through a full serialize → deserialize cycle", () => {
    const json = serialize("wf_ud", "UD On", new Map(), new Map(), undefined, undefined, undefined, undefined, true);
    expect(deserialize(json).unlimitedDuration).toBe(true);
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
// disabled flag: ContextMenu's Disable/Enable toggle must survive
// save (serialize) and load (deserialize).
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

// ---------------------------------------------------------------------------
// duplicateWorkflow: must route through deserialize/
// serialize() so a pre-existing dangling edge is dropped rather than
// propagated verbatim into the copy — the old implementation hand-edited
// the raw parsed JSON and had no such filter.
// ---------------------------------------------------------------------------

const LS_KEY = "aerini_workflows_v1";

/**
 * Node's own built-in Web Storage global (nodejs/node#57666) can shadow
 * jsdom's window.localStorage with a non-functional stand-in (getItem/
 * setItem undefined unless the process was started with
 * --localstorage-file), depending on Node version — the same gotcha
 * modal-manager.test.ts already documents and works around. duplicateWorkflow
 * reads/writes localStorage directly, so give every test in this describe
 * block a real, working in-memory implementation regardless of what the
 * host Node build provides.
 */
function installLocalStorageStub(): void {
  const store = new Map<string, string>();
  const stub: Pick<Storage, "getItem" | "setItem" | "removeItem" | "clear"> = {
    getItem: (key: string) => (store.has(key) ? store.get(key)! : null),
    setItem: (key: string, value: string) => { store.set(key, String(value)); },
    removeItem: (key: string) => { store.delete(key); },
    clear: () => { store.clear(); },
  };
  globalThis.localStorage = stub as Storage;
}

function lsSaveRaw(id: string, name: string, json: string): void {
  const all = JSON.parse(localStorage.getItem(LS_KEY) ?? "{}");
  all[id] = { id, name, json, updated_at: new Date().toISOString() };
  localStorage.setItem(LS_KEY, JSON.stringify(all));
}

function fakeManagerThis() {
  return { onToast: vi.fn(), refreshWorkflowList: vi.fn(async () => {}) };
}

describe("duplicateWorkflow — dangling-edge filter", () => {
  beforeEach(() => {
    installLocalStorageStub();
  });

  it("drops a pre-existing dangling edge instead of propagating it into the copy", async () => {
    const n1 = makeNode("n1", 0, 0);
    const n2 = makeNode("n2", 100, 0);
    const validConn = makeConnector("e_valid", "n1", "n2");
    const goodJson = serialize("wf_src", "Source", new Map([["n1", n1], ["n2", n2]]), new Map([["e_valid", validConn]]));

    // Simulate a pre-existing dangling edge (e.g. via the undo-sharing bug)
    // by splicing one directly into the saved document —
    // serialize's own filter would otherwise never let one exist
    // in a freshly-serialized document.
    const doc = JSON.parse(goodJson);
    doc.edges.push({
      id: "e_dangling", from_node: "n1", to_node: "NODE_THAT_DOES_NOT_EXIST",
      from_port: "output", to_port: "input",
      condition: null, on_success: null, on_failure: null,
    });
    lsSaveRaw("wf_src", "Source", JSON.stringify(doc));

    const fakeThis = fakeManagerThis();
    await WorkflowManager.prototype.duplicateWorkflow.call(fakeThis as never, "wf_src", "Source");

    const all = JSON.parse(localStorage.getItem(LS_KEY) ?? "{}");
    const copyEntry = Object.values(all as Record<string, { id: string; json: string }>)
      .find(e => e.id !== "wf_src");
    expect(copyEntry).toBeDefined();

    const copyDoc = JSON.parse(copyEntry!.json);
    expect(copyDoc.edges).toHaveLength(1);
    expect(copyDoc.nodes).toHaveLength(2);
    // The surviving edge must reference the *remapped* node ids, not the
    // dangling target and not the original source ids.
    const survivingEdge = copyDoc.edges[0];
    const newIds = copyDoc.nodes.map((n: { id: string }) => n.id);
    expect(newIds).toContain(survivingEdge.from_node);
    expect(newIds).toContain(survivingEdge.to_node);
    expect(survivingEdge.to_node).not.toBe("NODE_THAT_DOES_NOT_EXIST");
  });

  it("gives the duplicate new node/edge ids, distinct from the source", async () => {
    const n1 = makeNode("n1", 0, 0);
    const n2 = makeNode("n2", 100, 0);
    const conn = makeConnector("e1", "n1", "n2");
    const json = serialize("wf_src2", "Source2", new Map([["n1", n1], ["n2", n2]]), new Map([["e1", conn]]));
    lsSaveRaw("wf_src2", "Source2", json);

    const fakeThis = fakeManagerThis();
    await WorkflowManager.prototype.duplicateWorkflow.call(fakeThis as never, "wf_src2", "Source2");

    const all = JSON.parse(localStorage.getItem(LS_KEY) ?? "{}");
    const copyEntry = Object.values(all as Record<string, { id: string; json: string }>)
      .find(e => e.id !== "wf_src2");
    const copyDoc = JSON.parse(copyEntry!.json);

    const copyNodeIds = copyDoc.nodes.map((n: { id: string }) => n.id);
    expect(copyNodeIds).not.toContain("n1");
    expect(copyNodeIds).not.toContain("n2");
    expect(copyDoc.edges[0].id).not.toBe("e1");
    expect(fakeThis.onToast).toHaveBeenCalledWith(expect.stringContaining("Duplicated as"), "success");
  });
});

// ---------------------------------------------------------------------------
// handleExport: must carry metadata.author/tags from the serialized document
// instead of building the exported file object from scratch.
// ---------------------------------------------------------------------------

describe("handleExport", () => {
  it("carries author and tags from the serialized metadata instead of dropping them", () => {
    const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;
    invokeMock.mockReset();
    invokeMock.mockResolvedValue("workspace/export-me.aerini");

    const fakeThis = {
      canvas: { nodes: new Map([["n1", makeNode("n1", 0, 0)]]), connectors: new Map() },
      currentId: "wf_exp", currentName: "Export Me",
      parallelExecution: false, maxConcurrentNodes: 8,
      chatSettings: DEFAULT_CHAT_SETTINGS, currentTags: ["prod", "webhook"],
      onToast: vi.fn(),
    };

    WorkflowManager.prototype.handleExport.call(fakeThis as never);

    const call = invokeMock.mock.calls.find(c => c[0] === "save_file_dialog");
    expect(call).toBeDefined();
    const file = JSON.parse((call![1] as { content: string }).content);
    expect(file.tags).toEqual(["prod", "webhook"]);
    expect(file.author).toBe("user");
  });
});

// ---------------------------------------------------------------------------
// handleNew: must reset currentTags along with the other per-workflow state.
// ---------------------------------------------------------------------------

describe("handleNew", () => {
  it("resets currentTags to [] along with id/name/execution settings", async () => {
    document.body.innerHTML = '<div id="output-drawer"></div>';
    const fakeThis = {
      hasUnsaved: false,
      currentId: "wf_old", currentName: "Old Name", currentTags: ["stale"],
      parallelExecution: true, maxConcurrentNodes: 16,
      chatSettings: { ...DEFAULT_CHAT_SETTINGS, show_branding: false },
      canvas: { nodes: new Map([["n1", makeNode("n1", 0, 0)]]), connectors: new Map(), clearSelection: vi.fn() },
      markUnsaved: vi.fn(),
      onPanelClose: undefined, onTitleChange: vi.fn(),
      onStatusChange: vi.fn(), refreshWorkflowList: vi.fn(async () => {}),
      onNavigate: undefined, confirmFn: vi.fn(),
    };

    await WorkflowManager.prototype.handleNew.call(fakeThis as never);

    expect(fakeThis.currentTags).toEqual([]);
    expect(fakeThis.currentName).toBe("Untitled");
  });
});

// ---------------------------------------------------------------------------
// prepareForBgRun: the browser (non-Tauri) save path went through lsSave()
// with a hardcoded 3-arg call that silently dropped tags.
// ---------------------------------------------------------------------------

describe("prepareForBgRun — browser (non-Tauri) path", () => {
  beforeEach(() => {
    installLocalStorageStub();
  });

  it("persists via lsSave with the current tags", async () => {
    const fakeThis = {
      canvas: { nodes: new Map([["n1", makeNode("n1", 0, 0)]]), connectors: new Map() },
      currentId: "wf_bg", currentName: "Bg Run",
      parallelExecution: false, maxConcurrentNodes: 8, chatSettings: DEFAULT_CHAT_SETTINGS,
      currentTags: ["scheduled"],
      markUnsaved: vi.fn(), refreshWorkflowList: vi.fn(async () => {}), onToast: vi.fn(),
    };

    const result = await WorkflowManager.prototype.prepareForBgRun.call(fakeThis as never);

    expect(result?.id).toBe("wf_bg");
    const stored = JSON.parse(localStorage.getItem(LS_KEY) ?? "{}");
    expect(stored["wf_bg"].tags).toEqual(["scheduled"]);
  });
});
