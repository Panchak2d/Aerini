import { afterEach, describe, expect, it } from "vitest";
import { setRunOutputs, valueAt, primaryOutputKey, suggestMainResult } from "../data-sources";
import { previewField } from "../field-preview";

const nodes = [{ id: "n1", name: "Feed" }, { id: "n2", name: "Code (JS)" }];
afterEach(() => setRunOutputs(null));

describe("previewField", () => {
  it("plain_text_has_no_preview", () => {
    expect(previewField("hello", nodes).status).toBe("none");
  });

  it("resolves_references_inside_surrounding_text", () => {
    setRunOutputs({ n1: { latest: { title: "Hi" }, items: [{ id: 7 }] } });
    expect(previewField("News: {{Feed.output.latest.title}} / {{Feed.output.items[0].id}}", nodes))
      .toMatchObject({ status: "ok", text: "News: Hi / 7" });
  });

  it("names_the_node_and_path_when_the_value_is_missing", () => {
    setRunOutputs({ n1: { latest: {} } });
    expect(previewField("{{Feed.output.latest.title}}", nodes))
      .toMatchObject({ status: "empty", nodeName: "Feed", path: "latest.title", nodeId: "n1" });
  });

  it("reports_unrun_when_the_node_has_no_output_yet", () => {
    expect(previewField("{{Feed.output.x}}", nodes)).toMatchObject({ status: "unrun", nodeName: "Feed" });
  });

  it("functions_and_unknown_nodes_are_unsupported_not_guessed", () => {
    setRunOutputs({ n1: { x: "a" } });
    expect(previewField("{{upper(Feed.output.x)}}", nodes).status).toBe("unsupported");
    expect(previewField("{{Nope.output.x}}", nodes).status).toBe("unsupported");
    expect(previewField("{{$run.id}}", nodes).status).toBe("unsupported");
  });

  it("node_names_with_parentheses_resolve", () => {
    setRunOutputs({ n2: { out: "ok" } });
    expect(previewField("{{Code (JS).output.out}}", nodes)).toMatchObject({ status: "ok", text: "ok" });
  });
});

describe("data-sources helpers", () => {
  it("valueAt_walks_objects_and_array_indexes", () => {
    expect(valueAt({ a: { b: [1, { c: 2 }] } }, "a.b[1].c")).toBe(2);
    expect(valueAt({ a: 1 }, "a.b")).toBeUndefined();
  });

  it("primaryOutputKey_prefers_the_flag_then_a_sole_property", () => {
    expect(primaryOutputKey({ properties: { a: { type: "string" }, b: { "x-aerini-primary-output": true } } })).toBe("b");
    expect(primaryOutputKey({ properties: { only: { type: "string" } } })).toBe("only");
    expect(primaryOutputKey({ properties: { a: {}, b: {} } })).toBeNull();
  });

  it("suggestMainResult_picks_the_nearest_node_that_has_one", () => {
    const g = {
      connectors: new Map([["e", { data: { from_node: "n1", to_node: "cur" } }]]),
      nodes: new Map([["n1", { data: { id: "n1", name: "Feed", output_schema: { properties: { digest: {} } } } }]]),
    };
    expect(suggestMainResult(g, "cur")?.expr).toBe("{{Feed.output.digest}}");
  });
});
