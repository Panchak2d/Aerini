import { describe, it, expect } from "vitest";
import {
  MAX_TAGS_PER_WORKFLOW, MAX_TAG_LENGTH, applyListFilters, applyTagEdit, buildTagFacets,
  buildTagIndex, matchesQuery, matchesTags, normalizeTag, normalizeTags, pruneSelectedTags, suggestTags,
} from "../workflow-tags";

const wf = (name: string, tags: unknown) => ({ name, tags });

describe("normalizeTag", () => {
  it("trims, collapses inner whitespace and strips commas and control characters", () => {
    expect(normalizeTag("  my   tag ")).toBe("my tag");
    expect(normalizeTag("a,b")).toBe("ab");
    expect(normalizeTag("x\u0007y\tz")).toBe("xy z");
  });

  it("rejects non-strings and tags that are empty after cleaning", () => {
    expect(normalizeTag(42)).toBeNull();
    expect(normalizeTag(null)).toBeNull();
    expect(normalizeTag("  , \u0001 ")).toBeNull();
  });

  it("truncates to the maximum length without splitting a surrogate pair", () => {
    const long = "😀".repeat(MAX_TAG_LENGTH + 5);
    expect(Array.from(normalizeTag(long)!)).toHaveLength(MAX_TAG_LENGTH);
  });

  it("normalizes to NFC", () => {
    expect(normalizeTag("e\u0301")).toBe("\u00e9");
  });
});

describe("normalizeTags", () => {
  it("dedupes case-insensitively keeping the first casing and drops junk", () => {
    expect(normalizeTags(["Prod", "prod", 7, "", "ai"])).toEqual(["Prod", "ai"]);
    expect(normalizeTags("not an array")).toEqual([]);
  });

  it("adopts the library casing for a known tag", () => {
    const lib = buildTagIndex([wf("a", ["Prod"])]);
    expect(normalizeTags(["prod"], lib)).toEqual(["Prod"]);
  });

  it("caps the list at the per-workflow limit", () => {
    const many = Array.from({ length: 50 }, (_, i) => `t${i}`);
    expect(normalizeTags(many)).toHaveLength(MAX_TAGS_PER_WORKFLOW);
  });
});

describe("buildTagIndex", () => {
  it("counts each workflow once per tag and keeps first-seen casing", () => {
    const idx = buildTagIndex([wf("a", ["Prod", "prod", "ai"]), wf("b", ["PROD"]), wf("c", undefined)]);
    expect(idx.get("prod")).toEqual({ label: "Prod", count: 2 });
    expect(idx.get("ai")).toEqual({ label: "ai", count: 1 });
  });
});

describe("matching", () => {
  it("matches a query against each tag separately, never across a tag boundary", () => {
    expect(matchesQuery(wf("x", ["chat", "ai"]), "t a")).toBe(false);
    expect(matchesQuery(wf("x", ["chat", "ai"]), "HAT")).toBe(true);
    expect(matchesQuery(wf("Daily Report", []), "report")).toBe(true);
  });

  it("requires every selected tag (AND)", () => {
    const sel = new Set(["a", "b"]);
    expect(matchesTags(wf("x", ["A", "b", "c"]), sel)).toBe(true);
    expect(matchesTags(wf("x", ["a"]), sel)).toBe(false);
  });

  it("combines text and tag filters with AND", () => {
    const items = [wf("Alpha", ["x"]), wf("Beta", ["x"]), wf("Alpha2", ["y"])];
    expect(applyListFilters(items, "alpha", new Set(["x"])).map(i => i.name)).toEqual(["Alpha"]);
  });
});

describe("buildTagFacets", () => {
  const items = [wf("a", ["x", "y"]), wf("b", ["x"]), wf("c", ["z"])];

  it("counts what a click would show and orders by count then label", () => {
    expect(buildTagFacets(items, "", new Set()).map(f => [f.label, f.count])).toEqual([["x", 2], ["y", 1], ["z", 1]]);
  });

  it("hides unselected tags that would match nothing and lists selected ones first", () => {
    const facets = buildTagFacets(items, "", new Set(["x"]));
    expect(facets.map(f => f.label)).toEqual(["x", "y"]);
    expect(facets[0].selected).toBe(true);
  });

  it("narrows counts by the search text", () => {
    expect(buildTagFacets(items, "a", new Set()).map(f => f.label)).toEqual(["x", "y"]);
  });
});

describe("pruneSelectedTags", () => {
  it("drops selected keys that no longer exist in the library", () => {
    const idx = buildTagIndex([wf("a", ["x"])]);
    expect([...pruneSelectedTags(idx, new Set(["x", "gone"]))]).toEqual(["x"]);
  });
});

describe("suggestTags", () => {
  const idx = buildTagIndex([wf("1", ["beta", "alpha"]), wf("2", ["alphabet", "beta"]), wf("3", ["beta"])]);

  it("excludes present tags and ranks prefix matches before substring matches", () => {
    expect(suggestTags(idx, new Set(["beta"]), "bet", 8).map(e => e.label)).toEqual(["alphabet"]);
    expect(suggestTags(idx, new Set(), "ha", 8).map(e => e.label)).toEqual(["alpha", "alphabet"]);
  });

  it("lists by count descending when the input is empty", () => {
    expect(suggestTags(idx, new Set(), "", 2).map(e => e.label)).toEqual(["beta", "alpha"]);
  });
});

describe("applyTagEdit", () => {
  it("adds and removes by add/remove sets, leaving other tags untouched", () => {
    const r = applyTagEdit(["keep", "drop"], { add: ["New"], remove: ["DROP"] });
    expect(r.tags).toEqual(["keep", "New"]);
    expect(r.changed).toBe(true);
  });

  it("reports no change when the edit has no effect", () => {
    expect(applyTagEdit(["a"], { add: ["A"], remove: ["zzz"] }).changed).toBe(false);
  });

  it("counts adds skipped because the list is full", () => {
    const full = Array.from({ length: MAX_TAGS_PER_WORKFLOW }, (_, i) => `t${i}`);
    const r = applyTagEdit(full, { add: ["extra"], remove: [] });
    expect(r.skippedForLimit).toBe(1);
    expect(r.tags).toHaveLength(MAX_TAGS_PER_WORKFLOW);
  });
});
