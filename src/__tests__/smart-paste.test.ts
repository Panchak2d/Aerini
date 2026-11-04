import { afterEach, describe, expect, it } from "vitest";
import { setRunOutputs } from "../data-sources";
import { findLiveCandidate } from "../smart-paste";

const feed = { id: "a", name: "Feed" };
const other = { id: "b", name: "Other" };

afterEach(() => setRunOutputs(null));

describe("findLiveCandidate", () => {
  it("matches_a_whole_value_and_builds_the_reference", () => {
    setRunOutputs({ a: { latest: { title: "Hello world" }, count: 12345 } });
    expect(findLiveCandidate("  Hello world ", [feed])).toMatchObject({
      nodeId: "a", nodeName: "Feed", path: "latest.title", expr: "{{Feed.output.latest.title}}", value: "Hello world",
    });
    expect(findLiveCandidate("12345", [feed])?.expr).toBe("{{Feed.output.count}}");
  });

  it("ignores_partial_short_braced_and_dismissed_text", () => {
    setRunOutputs({ a: { title: "Hello world", id: "abc", ref: "{{x}}" } });
    expect(findLiveCandidate("Hello", [feed])).toBeNull();
    expect(findLiveCandidate("abc", [feed])).toBeNull();
    expect(findLiveCandidate("{{x}}", [feed])).toBeNull();
    expect(findLiveCandidate("Say Hello world", [feed])).toBeNull();
    expect(findLiveCandidate("Hello world", [feed], new Set(["Hello world"]))).toBeNull();
  });

  it("needs_the_value_to_come_from_exactly_one_node", () => {
    setRunOutputs({ a: { title: "Same text" }, b: { name: "Same text" } });
    expect(findLiveCandidate("Same text", [feed, other])).toBeNull();
    expect(findLiveCandidate("Same text", [feed])?.nodeId).toBe("a");
    expect(findLiveCandidate("Same text", [other])?.path).toBe("name");
  });

  it("prefers_the_named_path_over_an_array_copy_and_refuses_when_still_ambiguous", () => {
    setRunOutputs({ a: { items: [{ title: "Hello world" }], latest: { title: "Hello world" } } });
    expect(findLiveCandidate("Hello world", [feed])?.path).toBe("latest.title");
    setRunOutputs({ a: { from: "same@x.com", to: "same@x.com" } });
    expect(findLiveCandidate("same@x.com", [feed])).toBeNull();
    setRunOutputs({ a: { items: [{ id: "item-1" }, { id: "item-1" }] } });
    expect(findLiveCandidate("item-1", [feed])).toBeNull();
    setRunOutputs({ a: { items: [{ id: "item-1" }] } });
    expect(findLiveCandidate("item-1", [feed])?.path).toBe("items[0].id");
  });

  it("skips_booleans_and_text_longer_than_the_index_keeps", () => {
    setRunOutputs({ a: { ok: true, long: "x".repeat(300) } });
    expect(findLiveCandidate("true", [feed])).toBeNull();
    expect(findLiveCandidate("x".repeat(200), [feed])).toBeNull();
  });

  it("only_looks_at_the_nodes_it_is_given", () => {
    setRunOutputs({ a: { title: "Hello world" } });
    expect(findLiveCandidate("Hello world", [other])).toBeNull();
  });
});
