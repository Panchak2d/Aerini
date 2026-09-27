// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";
import { serialize, deserialize } from "../canvas/CanvasSerializer";
import {
  groupWorkflowsByCollection,
  visibleCollectionGroups,
  computeSelectionRange,
  collectionColorVar,
  WorkflowManager,
  type CollectionDef,
} from "../workflow-manager";
import type { WorkflowSummary } from "../ipc/workflow";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function wf(id: string, collection_id: string | null = null): WorkflowSummary {
  return { id, name: id, updated_at: "2024-01-01T00:00:00Z", tags: [], collection_id };
}

function col(id: string, order: number): CollectionDef {
  return { id, name: id, color: "blue", order, collapsed: false };
}

// ---------------------------------------------------------------------------
// groupWorkflowsByCollection
// ---------------------------------------------------------------------------

describe("groupWorkflowsByCollection", () => {
  it("buckets workflows by collection_id, ordered by CollectionDef.order, Uncategorized last", () => {
    const collections = [col("c2", 1), col("c1", 0)]; // deliberately out of order
    const wfs = [wf("a", "c1"), wf("b", "c2"), wf("c", null), wf("d", "c1")];

    const groups = groupWorkflowsByCollection(wfs, collections);

    expect(groups).toHaveLength(3);
    expect(groups[0].collection?.id).toBe("c1");
    expect(groups[0].items.map(w => w.id)).toEqual(["a", "d"]);
    expect(groups[1].collection?.id).toBe("c2");
    expect(groups[1].items.map(w => w.id)).toEqual(["b"]);
    expect(groups[2].collection).toBeNull();
    expect(groups[2].items.map(w => w.id)).toEqual(["c"]);
  });

  it("always includes the Uncategorized bucket even when empty and no workflows are uncategorized", () => {
    const groups = groupWorkflowsByCollection([wf("a", "c1")], [col("c1", 0)]);
    const uncategorized = groups.find(g => g.collection === null);
    expect(uncategorized).toBeDefined();
    expect(uncategorized!.items).toEqual([]);
  });

  // Edge case found in self-audit: a collection_id that doesn't match any
  // known collection (e.g. deleted from another tab since this list was
  // last loaded) must not make the workflow disappear from every bucket.
  it("falls back a workflow with an orphaned collection_id to Uncategorized instead of dropping it", () => {
    const groups = groupWorkflowsByCollection([wf("a", "deleted-collection")], [col("c1", 0)]);
    const c1 = groups.find(g => g.collection?.id === "c1")!;
    const uncategorized = groups.find(g => g.collection === null)!;
    expect(c1.items).toEqual([]);
    expect(uncategorized.items.map(w => w.id)).toEqual(["a"]);
  });
});

// ---------------------------------------------------------------------------
// visibleCollectionGroups
// ---------------------------------------------------------------------------

describe("visibleCollectionGroups", () => {
  it("drops the Uncategorized bucket when it has no items", () => {
    const groups = groupWorkflowsByCollection([wf("a", "c1")], [col("c1", 0)]);
    const visible = visibleCollectionGroups(groups);
    expect(visible.some(g => g.collection === null)).toBe(false);
  });

  it("keeps Uncategorized when it has items, and never drops a named (possibly empty) collection", () => {
    const groups = groupWorkflowsByCollection([wf("a", "c1"), wf("b", null)], [col("c1", 0), col("c2", 1)]);
    const visible = visibleCollectionGroups(groups);
    expect(visible.find(g => g.collection === null)?.items.map(w => w.id)).toEqual(["b"]);
    expect(visible.some(g => g.collection?.id === "c2")).toBe(true); // empty named collection stays
  });
});

// ---------------------------------------------------------------------------
// computeSelectionRange
// ---------------------------------------------------------------------------

describe("computeSelectionRange", () => {
  const ids = ["a", "b", "c", "d", "e"];

  it("returns the inclusive range regardless of click direction", () => {
    expect(computeSelectionRange(ids, "b", "d")).toEqual(["b", "c", "d"]);
    expect(computeSelectionRange(ids, "d", "b")).toEqual(["b", "c", "d"]);
  });

  it("returns a single-element range when both endpoints are the same id", () => {
    expect(computeSelectionRange(ids, "c", "c")).toEqual(["c"]);
  });

  // Edge case: the anchor was filtered out of the visible list (e.g. by
  // search) between being set and the shift-click — must not guess a range.
  it("returns an empty range when either endpoint isn't in the given order", () => {
    expect(computeSelectionRange(ids, "gone", "d")).toEqual([]);
    expect(computeSelectionRange(ids, "b", "gone")).toEqual([]);
    expect(computeSelectionRange([], "a", "b")).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// collectionColorVar
// ---------------------------------------------------------------------------

describe("collectionColorVar", () => {
  it("maps every stored color name to a CSS variable reference, never a literal hex", () => {
    for (const color of ["blue", "green", "purple", "amber", "red", "slate"] as const) {
      expect(collectionColorVar(color)).toMatch(/^var\(--[\w-]+\)$/);
    }
  });
});

// ---------------------------------------------------------------------------
// CanvasSerializer — collection_id round-trip (mirrors the existing tags
// round-trip test in workflow-manager.test.ts)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// buildCollectionGroup — inline collection-rename, commit on outside click
// ---------------------------------------------------------------------------

function callBuildCollectionGroup(
  fakeThis: unknown,
  collection: CollectionDef | null,
  items: WorkflowSummary[],
): HTMLElement {
  return (WorkflowManager.prototype as unknown as {
    buildCollectionGroup: (c: CollectionDef | null, i: WorkflowSummary[]) => HTMLElement;
  }).buildCollectionGroup.call(fakeThis, collection, items);
}

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("buildCollectionGroup — collection rename commit on outside click", () => {
  it("normal case: a mousedown outside the input commits via commitCollectionRename", async () => {
    const collection = col("c1", 0);
    const fakeThis = {
      editingCollectionId: "c1",
      commitCollectionRename: vi.fn(),
      cancelCollectionRename: vi.fn(),
    };

    const el = callBuildCollectionGroup(fakeThis, collection, []);
    document.body.appendChild(el);
    const inp = el.querySelector("input.workflow-collection-rename-input") as HTMLInputElement;
    inp.value = "Renamed Collection";

    await new Promise(r => setTimeout(r, 0));
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));

    expect(fakeThis.commitCollectionRename).toHaveBeenCalledWith("c1", "Renamed Collection");
    expect(fakeThis.cancelCollectionRename).not.toHaveBeenCalled();
  });

  it("edge case: Escape cancels and a later outside click does not also commit", async () => {
    const collection = col("c1", 0);
    const fakeThis = {
      editingCollectionId: "c1",
      commitCollectionRename: vi.fn(),
      cancelCollectionRename: vi.fn(),
    };

    const el = callBuildCollectionGroup(fakeThis, collection, []);
    document.body.appendChild(el);
    const inp = el.querySelector("input.workflow-collection-rename-input") as HTMLInputElement;
    inp.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

    await new Promise(r => setTimeout(r, 0));
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));

    expect(fakeThis.cancelCollectionRename).toHaveBeenCalledWith("c1");
    expect(fakeThis.commitCollectionRename).not.toHaveBeenCalled();
  });
});

describe("serialize/deserialize — collection_id", () => {
  it("round-trips a set collection_id", () => {
    const json = serialize(
      "wf_c", "Collection Test", new Map(), new Map(),
      undefined, undefined, undefined, [], undefined, "col_123",
    );
    const doc = JSON.parse(json);
    expect(doc.metadata.collection_id).toBe("col_123");
    expect(deserialize(json).collectionId).toBe("col_123");
  });

  it("defaults to null (Uncategorized) when omitted, and for legacy JSON with no metadata.collection_id", () => {
    const json = serialize("wf_u", "Uncategorized Test", new Map(), new Map());
    expect(JSON.parse(json).metadata.collection_id).toBeNull();
    expect(deserialize(json).collectionId).toBeNull();

    const legacyJson = JSON.stringify({ id: "wf_legacy", name: "Legacy", nodes: [], edges: [] });
    expect(deserialize(legacyJson).collectionId).toBeNull();
  });
});
