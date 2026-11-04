import { describe, it, expect, beforeEach } from "vitest";
import {
  buildTestPlan, clearPresetCache, describeTestPlan, hashGraph, recordOutputs,
  type CacheGraph,
} from "../preset-cache";
import { RunStateMachine } from "../run-manager/state-machine";

type Spec = { id: string; name: string; type?: string; config?: Record<string, unknown> };

function graph(nodes: Spec[], edges: Array<[string, string, string?]>): CacheGraph {
  return {
    nodes: new Map(nodes.map(n => [n.id, {
      data: { id: n.id, name: n.name, node_type_id: n.type ?? "transform", config: n.config ?? {} },
    }])),
    connectors: new Map(edges.map(([from, to, port], i) => [`e${i}`, {
      data: { from_node: from, from_port: port ?? "output", to_node: to, to_port: "input" },
    }])),
  };
}

const chain = (config: Record<string, unknown> = {}) =>
  graph([
    { id: "a", name: "Fetch", config },
    { id: "b", name: "Summarize" },
    { id: "c", name: "Notify" },
  ], [["a", "b"], ["b", "c"]]);

beforeEach(() => clearPresetCache());

describe("buildTestPlan", () => {
  it("reuses_recorded_outputs_while_a_step_and_everything_before_it_is_unchanged", async () => {
    const g = chain();
    const hashes = await hashGraph(g);
    recordOutputs(hashes, { a: { body: "x" }, b: { content: "y" } }, new Set(), 1_000);

    const plan = buildTestPlan(g, "c", hashes, true);
    expect(plan.presets).toEqual({ a: { body: "x" }, b: { content: "y" } });
    expect(plan.willRun).toEqual([]);
    expect(describeTestPlan(plan, 1_000 + 120_000)).toBe("Using results from Summarize, Fetch (2 min ago).");

    const everything = buildTestPlan(g, "c", hashes, false);
    expect(everything.presets).toEqual({});
    expect(everything.willRun.map(s => s.name)).toEqual(["Summarize", "Fetch"]);
  });

  it("an_edit_to_an_earlier_step_makes_it_and_the_steps_after_it_run_again", async () => {
    const before = chain({ url: "one" });
    recordOutputs(await hashGraph(before), { a: 1, b: 2 }, new Set(), 1);

    const after = chain({ url: "two" });
    const plan = buildTestPlan(after, "c", await hashGraph(after), true);
    expect(plan.presets).toEqual({});
    expect(plan.willRun.map(s => s.name)).toEqual(["Summarize", "Fetch"]);
    expect(plan.changed.map(s => s.name)).toEqual(["Fetch"]);
    expect(describeTestPlan(plan)).toContain("Earlier results are out of date: Fetch changed.");
  });

  it("renaming_a_step_invalidates_its_record_because_references_use_names", async () => {
    const g = chain();
    recordOutputs(await hashGraph(g), { a: 1 }, new Set(), 1);
    g.nodes.get("a")!.data.name = "Download";
    const plan = buildTestPlan(g, "c", await hashGraph(g), true);
    expect(plan.cachedSteps).toEqual([]);
  });

  it("results_recorded_under_other_run_input_variables_are_not_reused", async () => {
    const g = chain();
    recordOutputs(await hashGraph(g, '{"topic":"a"}'), { a: 1 }, new Set(), 1);
    expect(buildTestPlan(g, "c", await hashGraph(g, ""), true).cachedSteps).toEqual([]);
  });

  it("loop_nodes_and_the_steps_inside_a_loop_are_never_reused", async () => {
    const g = graph([
      { id: "l", name: "Each", type: "loop" },
      { id: "i", name: "Inside" },
      { id: "t", name: "Target" },
    ], [["l", "i", "loop_body"], ["i", "t"]]);
    const hashes = await hashGraph(g);
    recordOutputs(hashes, { l: 1, i: 2 }, new Set(), 1);
    const plan = buildTestPlan(g, "t", hashes, true);
    expect(plan.presets).toEqual({});
    expect(plan.willRun.map(s => s.id).sort()).toEqual(["i", "l"]);
  });

  it("a_step_answered_from_the_record_keeps_its_original_time", async () => {
    const g = chain();
    const hashes = await hashGraph(g);
    recordOutputs(hashes, { a: "first" }, new Set(), 1_000);
    recordOutputs(hashes, { a: "second" }, new Set(["a"]), 9_000);
    const plan = buildTestPlan(g, "c", hashes, true);
    expect(plan.presets).toEqual({ a: "first" });
    expect(plan.oldestAt).toBe(1_000);
  });
});

describe("RunStateMachine.setLastResult", () => {
  it("dropping_the_last_result_also_drops_recorded_outputs", async () => {
    const g = chain();
    const hashes = await hashGraph(g);
    recordOutputs(hashes, { a: 1 }, new Set(), 1);
    new RunStateMachine().setLastResult(null);
    expect(buildTestPlan(g, "c", hashes, true).presets).toEqual({});
  });
});
