import { describe, it, expect, beforeEach } from "vitest";
import {
  getUpstreamNodes, listPaths, setRunOutputs, getRunOutput, buildValueIndex,
  closestPath, sourceColorIndex, SOURCE_COLOR_COUNT, type SourceGraph,
} from "../data-sources";

function graph(edges: Array<[string, string]>, names: Record<string, string>): SourceGraph {
  return {
    connectors: new Map(edges.map(([f, t], i) => [`e${i}`, { data: { from_node: f, to_node: t } }])),
    nodes: new Map(Object.entries(names).map(([id, name]) => [id, { data: { id, name } }])),
  };
}

beforeEach(() => setRunOutputs(null));

describe("getUpstreamNodes", () => {
  it("upstream_only_excludes_downstream_and_unconnected_nodes", () => {
    const g = graph([["a", "b"], ["b", "c"], ["c", "d"]], { a: "A", b: "B", c: "C", d: "D", z: "Z" });
    expect(getUpstreamNodes(g, "c").map(n => n.name)).toEqual(["B", "A"]);
  });

  it("terminates_and_excludes_self_when_the_graph_has_a_cycle", () => {
    const g = graph([["a", "b"], ["b", "a"]], { a: "A", b: "B" });
    expect(getUpstreamNodes(g, "a").map(n => n.name)).toEqual(["B"]);
  });
});

describe("listPaths", () => {
  it("nested_paths_come_from_sample_when_schema_is_untyped", () => {
    const paths = listPaths({}, { latest: { title: "x" }, items: [{ id: 1 }], tags: ["a"] }).map(p => p.path);
    expect(paths).toEqual(["latest", "latest.title", "items", "items[0].id", "tags", "tags[0]"]);
  });

  it("schema_fields_come_first_and_sample_adds_only_missing_keys", () => {
    const schema = { properties: { title: { type: "string", description: "Headline" } } };
    const out = listPaths(schema, { title: "x", extra: 1 });
    expect(out[0]).toEqual({ path: "title", type: "string", description: "Headline" });
    expect(out.map(p => p.path)).toEqual(["title", "extra"]);
  });

  it("keys_that_cannot_be_written_in_an_expression_are_skipped", () => {
    expect(listPaths({}, { "a.b": 1, "c[0]": 2, ok: 3 }).map(p => p.path)).toEqual(["ok"]);
  });

  it("depth_and_count_are_capped", () => {
    const deep = { a: { b: { c: { d: { e: 1 } } } } };
    expect(listPaths({}, deep).map(p => p.path)).toEqual(["a", "a.b", "a.b.c", "a.b.c.d"]);
    const wide = Object.fromEntries(Array.from({ length: 500 }, (_, i) => [`k${i}`, i]));
    expect(listPaths({}, wide).length).toBe(200);
  });
});

describe("run cache and value index", () => {
  it("cache_is_cleared_by_null_and_replaced_by_a_new_run", () => {
    setRunOutputs({ a: { v: 1 } });
    expect(getRunOutput("a")).toEqual({ v: 1 });
    setRunOutputs({ b: 2 });
    expect(getRunOutput("a")).toBeUndefined();
    setRunOutputs(null);
    expect(getRunOutput("b")).toBeUndefined();
  });

  it("index_flattens_leaf_values_with_full_references", () => {
    setRunOutputs({ a: { latest: { title: "Hello" }, n: 3 }, s: "text" });
    const idx = buildValueIndex([{ id: "a", name: "Feed" }, { id: "s", name: "S" }]);
    expect(idx.map(e => [e.ref, e.display])).toEqual([
      ["Feed.output.latest.title", "Hello"], ["Feed.output.n", "3"], ["S.output", "text"],
    ]);
  });

  it("index_truncates_long_strings_and_bounds_entries_and_nodes", () => {
    setRunOutputs({ a: { long: "x".repeat(5000), ...Object.fromEntries(Array.from({ length: 1000 }, (_, i) => [`k${i}`, i])) } });
    const one = buildValueIndex([{ id: "a", name: "A" }]);
    expect(one[0].display.length).toBe(200);
    expect(one.length).toBe(300);
    const outputs = Object.fromEntries(Array.from({ length: 100 }, (_, i) => [`n${i}`, { v: i }]));
    setRunOutputs(outputs);
    const many = buildValueIndex(Object.keys(outputs).map(id => ({ id, name: id })));
    expect(many.length).toBe(30);
  });
});

describe("closestPath", () => {
  it("finds_a_transposed_or_miscased_field", () => {
    expect(closestPath("titel", ["author", "title"])).toBe("title");
    expect(closestPath("CONTENT", ["content"])).toBe("content");
  });

  it("returns_null_when_nothing_is_close", () => {
    expect(closestPath("xyz", ["title", "author"])).toBeNull();
    expect(closestPath("", ["a"])).toBeNull();
  });
});

describe("sourceColorIndex", () => {
  it("is_stable_per_node_and_stays_inside_the_palette", () => {
    const first = sourceColorIndex("node-1");
    for (let i = 0; i < 20; i++) sourceColorIndex(`other-${i}`);
    expect(sourceColorIndex("node-1")).toBe(first);
    expect(sourceColorIndex("other-19")).toBeLessThan(SOURCE_COLOR_COUNT);
  });
});

describe("run state feeds the cache", () => {
  it("setLastResult_fills_the_cache_and_null_clears_it", async () => {
    const { RunStateMachine } = await import("../run-manager/state-machine");
    const state = new RunStateMachine();
    state.setLastResult({ execution_id: "e", workflow_id: "w", success: true, node_outputs: { a: { v: 1 } }, logs: [] });
    expect(getRunOutput("a")).toEqual({ v: 1 });
    state.setLastResult(null);
    expect(getRunOutput("a")).toBeUndefined();
  });
});
