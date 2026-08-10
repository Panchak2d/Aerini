import { describe, it, expect } from "vitest";
import { diffWorkflowJson } from "../canvas/WorkflowDiff";

function baseDoc(overrides: Record<string, unknown> = {}) {
  return {
    schema_version: "1.0",
    id: "wf_1",
    name: "My Workflow",
    description: "",
    nodes: [],
    edges: [],
    metadata: { author: "user", created_at: "2024-01-01T00:00:00Z", updated_at: "2024-01-01T00:00:00Z", version: "1.0.0", tags: [] },
    parallel_execution: false,
    max_concurrent_nodes: 8,
    settings: { chat: {} },
    ...overrides,
  };
}

function baseNode(overrides: Record<string, unknown> = {}) {
  return {
    id: "n1",
    node_type_id: "http_request",
    node_type: "action",
    name: "Node 1",
    config: { url: "https://example.com" },
    credentials: {},
    ports: { inputs: [{ id: "input", label: "Input", position: "left" }], outputs: [] },
    input_schema: {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    disabled: false,
    position: { x: 0, y: 0 },
    ...overrides,
  };
}

function baseEdge(overrides: Record<string, unknown> = {}) {
  return {
    id: "e1", from_node: "n1", from_port: "output", to_node: "n2", to_port: "input",
    condition: null, on_success: null, on_failure: null,
    ...overrides,
  };
}

describe("diffWorkflowJson — identical documents", () => {
  it("reports isEmpty: true with no metadata/node/edge entries", () => {
    const doc = baseDoc({ nodes: [baseNode()], edges: [baseEdge()] });
    const json = JSON.stringify(doc);
    const result = diffWorkflowJson(json, json);
    expect(result.isEmpty).toBe(true);
    expect(result.metadata).toEqual([]);
    expect(result.nodes).toEqual([]);
    expect(result.edges).toEqual([]);
  });

  it("is order-independent for nested object fields (key order differs, values don't)", () => {
    const a = baseDoc({ nodes: [baseNode({ config: { url: "https://x", method: "GET" } })] });
    const b = baseDoc({ nodes: [baseNode({ config: { method: "GET", url: "https://x" } })] });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.isEmpty).toBe(true);
  });
});

describe("diffWorkflowJson — workflow-level metadata changes", () => {
  it("reports a changed Name field", () => {
    const a = baseDoc({ name: "Old" });
    const b = baseDoc({ name: "New" });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.metadata).toContainEqual({ field: "Name", from: "Old", to: "New" });
  });

  it("reports Description, Tags, Parallel execution, and Max concurrent nodes independently", () => {
    const a = baseDoc({ description: "a", metadata: { ...baseDoc().metadata, tags: ["x"] }, parallel_execution: false, max_concurrent_nodes: 8 });
    const b = baseDoc({ description: "b", metadata: { ...baseDoc().metadata, tags: ["y", "z"] }, parallel_execution: true, max_concurrent_nodes: 4 });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    const fields = result.metadata.map(c => c.field);
    expect(fields).toEqual(expect.arrayContaining(["Description", "Tags", "Parallel execution", "Max concurrent nodes"]));
    expect(result.metadata.find(c => c.field === "Tags")).toEqual({ field: "Tags", from: ["x"], to: ["y", "z"] });
  });

  it("reports a changed Chat setting (allow_attachments)", () => {
    const a = baseDoc({ settings: { chat: { allow_attachments: false } } });
    const b = baseDoc({ settings: { chat: { allow_attachments: true } } });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.metadata).toContainEqual({ field: "Chat: allow attachments", from: false, to: true });
  });

  it("does not report unset chat fields as changed (both sides fall back to the same default)", () => {
    const a = baseDoc({ settings: {} });
    const b = baseDoc({ settings: { chat: {} } });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.metadata.some(c => c.field.startsWith("Chat:"))).toBe(false);
  });

  it("never reports metadata.author, metadata.version, or metadata timestamps (excluded by design)", () => {
    const a = baseDoc({ metadata: { author: "user", created_at: "2024-01-01T00:00:00Z", updated_at: "2024-01-01T00:00:00Z", version: "1.0.0", tags: [] } });
    const b = baseDoc({ metadata: { author: "someone-else", created_at: "2099-01-01T00:00:00Z", updated_at: "2099-01-01T00:00:00Z", version: "9.9.9", tags: [] } });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.isEmpty).toBe(true);
  });
});

describe("diffWorkflowJson — unlimited_duration", () => {
  it("does not report unlimited_duration when unchanged", () => {
    const a = baseDoc({ unlimited_duration: false });
    const b = baseDoc({ unlimited_duration: false });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.metadata.some(c => c.field === "Unlimited duration")).toBe(false);
  });

  it("reports unlimited_duration changing from false to true", () => {
    const a = baseDoc({ unlimited_duration: false });
    const b = baseDoc({ unlimited_duration: true });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.metadata).toContainEqual({ field: "Unlimited duration", from: false, to: true });
  });
});

describe("diffWorkflowJson — node added / removed / changed", () => {
  it("reports a node present only in the new doc as added", () => {
    const a = baseDoc({ nodes: [] });
    const b = baseDoc({ nodes: [baseNode({ id: "n2" })] });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.nodes).toEqual([{ id: "n2", name: "Node 1", status: "added", changes: [] }]);
  });

  it("reports a node present only in the old doc as removed", () => {
    const a = baseDoc({ nodes: [baseNode({ id: "n2" })] });
    const b = baseDoc({ nodes: [] });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.nodes).toEqual([{ id: "n2", name: "Node 1", status: "removed", changes: [] }]);
  });

  it("reports a field-level change for a node present on both sides, naming the field and old/new value", () => {
    const a = baseDoc({ nodes: [baseNode({ config: { url: "https://old" } })] });
    const b = baseDoc({ nodes: [baseNode({ config: { url: "https://new" } })] });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.nodes).toHaveLength(1);
    expect(result.nodes[0].status).toBe("changed");
    expect(result.nodes[0].changes).toEqual([{ field: "config", from: { url: "https://old" }, to: { url: "https://new" } }]);
  });

  it("reports no entry for a node present on both sides with no field changes", () => {
    const a = baseDoc({ nodes: [baseNode()] });
    const b = baseDoc({ nodes: [baseNode()] });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.nodes).toEqual([]);
  });

  it("excludes ports, input_schema, output_schema, and dynamic_ports from node diffing (registry-sourced, not user-edited)", () => {
    const a = baseNode({ ports: { inputs: [], outputs: [] }, input_schema: { a: 1 }, output_schema: { b: 1 }, dynamic_ports: false });
    const b = baseNode({ ports: { inputs: [{ id: "x", label: "X", position: "left" }], outputs: [] }, input_schema: { a: 2 }, output_schema: { b: 2 }, dynamic_ports: true });
    const result = diffWorkflowJson(JSON.stringify(baseDoc({ nodes: [a] })), JSON.stringify(baseDoc({ nodes: [b] })));
    expect(result.nodes).toEqual([]);
  });

  it("uses id as the display name fallback when a node has no name", () => {
    const a = baseDoc({ nodes: [] });
    const noName = baseNode({ id: "n9" });
    delete (noName as { name?: string }).name;
    const b = baseDoc({ nodes: [noName] });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.nodes[0].name).toBe("n9");
  });
});

describe("diffWorkflowJson — edge added / removed / changed", () => {
  it("reports an added and a removed edge correctly, keyed by id", () => {
    const a = baseDoc({ edges: [baseEdge({ id: "e_old" })] });
    const b = baseDoc({ edges: [baseEdge({ id: "e_new" })] });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.edges).toHaveLength(2);
    expect(result.edges.find(e => e.id === "e_old")).toMatchObject({ status: "removed" });
    expect(result.edges.find(e => e.id === "e_new")).toMatchObject({ status: "added" });
  });

  it("reports a changed edge field (e.g. condition) with from/to", () => {
    const a = baseDoc({ edges: [baseEdge({ condition: null })] });
    const b = baseDoc({ edges: [baseEdge({ condition: "output.ok" })] });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.edges[0].changes).toEqual([{ field: "condition", from: null, to: "output.ok" }]);
  });
});

describe("diffWorkflowJson — id coercion and missing arrays", () => {
  it("matches nodes/edges by String(id) even when one side's id is a different JS type", () => {
    const a = baseDoc({ nodes: [baseNode({ id: 1 as unknown as string })] });
    const b = baseDoc({ nodes: [baseNode({ id: "1" })] });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.nodes).toEqual([]);
  });

  it("treats a missing nodes/edges array as empty rather than throwing", () => {
    const a = { schema_version: "1.0", id: "wf_1", name: "A" };
    const b = baseDoc({ nodes: [baseNode()], edges: [baseEdge()] });
    const result = diffWorkflowJson(JSON.stringify(a), JSON.stringify(b));
    expect(result.nodes).toHaveLength(1);
    expect(result.nodes[0].status).toBe("added");
    expect(result.edges).toHaveLength(1);
  });
});

describe("diffWorkflowJson — invalid input", () => {
  it("throws when either argument is not valid JSON", () => {
    expect(() => diffWorkflowJson("not json", JSON.stringify(baseDoc()))).toThrow();
    expect(() => diffWorkflowJson(JSON.stringify(baseDoc()), "not json")).toThrow();
  });
});
