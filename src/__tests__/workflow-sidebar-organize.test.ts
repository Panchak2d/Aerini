// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

const ipcMocks = vi.hoisted(() => ({
  listWorkflows:  vi.fn(),
  saveWorkflow:   vi.fn(),
  loadWorkflow:   vi.fn(),
  deleteWorkflow: vi.fn(),
  saveVersion:    vi.fn(),
  listVersions:   vi.fn(),
  getVersion:     vi.fn(),
  deleteVersion:  vi.fn(),
  getSetting:     vi.fn(),
  setSetting:     vi.fn(),
}));
vi.mock("../ipc/workflow", () => ipcMocks);

import { WorkflowManager } from "../workflow-manager";
import { closeOpenMenu } from "../menu";
import { closeTagPopover } from "../tag-editor";
import { deserialize } from "../canvas/CanvasSerializer";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowSummary } from "../ipc/workflow";

const flush = () => new Promise(r => setTimeout(r, 0));

function make() {
  const cb = {
    onUnsaved: vi.fn(), onTitle: vi.fn(), onStatus: vi.fn(), onToast: vi.fn(),
    confirm: vi.fn().mockResolvedValue(true),
  };
  const canvas = {
    nodes: new Map(), connectors: new Map(),
    clearSelection: vi.fn(), fitToScreen: vi.fn(),
  } as unknown as Canvas;
  return { mgr: new WorkflowManager(canvas, cb), cb };
}

function sum(id: string, name: string, tags: string[] = [], collection_id: string | null = null): WorkflowSummary {
  return { id, name, updated_at: "2024-01-01T00:00:00Z", tags, collection_id };
}

function stored(id: string, name: string, tags: string[] = []): string {
  return JSON.stringify({ id, name, nodes: [], edges: [], metadata: { tags, collection_id: null } });
}

const rows = () => Array.from(document.querySelectorAll<HTMLElement>(".workflow-item"));
const rowNames = () => rows().map(r => r.querySelector(".workflow-item-name")!.textContent);
const chip = (label: string) =>
  Array.from(document.querySelectorAll<HTMLElement>(".wf-tag-filter-chip")).find(c => c.querySelector(".wf-tag-filter-label")!.textContent === label)!;
const menuLabels = (id = "wf-item-ctx-menu") =>
  Array.from(document.querySelectorAll<HTMLElement>(`#${id} [role="menuitem"]`)).map(b => b.getAttribute("aria-label"));
const menuItem = (label: string, id = "wf-item-ctx-menu") =>
  Array.from(document.querySelectorAll<HTMLElement>(`#${id} [role="menuitem"]`)).find(b => b.getAttribute("aria-label") === label)!;

beforeEach(() => {
  document.body.innerHTML = `
    <span class="activity-btn" data-zone="workflows"><span class="activity-badge"></span></span>
    <input id="wf-search" />
    <div id="wf-tag-filter" class="hidden"></div>
    <div id="wf-bulk-bar" class="hidden"><span id="wf-bulk-count"></span></div>
    <div id="workflow-list"></div>`;
  for (const fn of Object.values(ipcMocks)) fn.mockReset();
  ipcMocks.getSetting.mockResolvedValue(null);
  ipcMocks.saveWorkflow.mockResolvedValue(undefined);
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};
});
afterEach(() => {
  closeOpenMenu();
  closeTagPopover();
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
});

describe("sidebar filtering", () => {
  const data = [sum("1", "Alpha", ["x", "y"]), sum("2", "Beta", ["x"]), sum("3", "Gamma", ["z"])];

  it("keeps the search filter across a refresh", async () => {
    ipcMocks.listWorkflows.mockResolvedValue(data);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    mgr.setSearchQuery("alp");
    await mgr.refreshWorkflowList();
    expect(rowNames()).toEqual(["Alpha"]);
  });

  it("filters without calling the list IPC again", async () => {
    ipcMocks.listWorkflows.mockResolvedValue(data);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    mgr.setSearchQuery("be");
    mgr.toggleTagFilter("x");
    expect(ipcMocks.listWorkflows).toHaveBeenCalledTimes(1);
    expect(rowNames()).toEqual(["Beta"]);
  });

  it("matches a search against single tags, not across the joined tag string", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "One", ["chat", "ai"])]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    mgr.setSearchQuery("t a");
    expect(rows()).toHaveLength(0);
    expect(document.getElementById("wf-search-no-match")).toBeTruthy();
  });

  it("shows filtered group counts and hides collections that the filter empties", async () => {
    ipcMocks.getSetting.mockResolvedValue(JSON.stringify({
      collections: [{ id: "c1", name: "Ops", color: "blue", order: 0, collapsed: false },
                    { id: "c2", name: "Other", color: "green", order: 1, collapsed: false }],
      uncategorizedCollapsed: false,
    }));
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A1", ["x"], "c1"), sum("2", "A2", [], "c1"), sum("3", "B1", ["x"], "c2")]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    mgr.toggleTagFilter("x");
    const counts = Array.from(document.querySelectorAll(".workflow-collection-group")).map(g => [
      g.querySelector(".workflow-collection-name")!.textContent, g.querySelector(".workflow-collection-count")!.textContent]);
    expect(counts).toEqual([["Ops", "1"], ["Other", "1"]]);
    mgr.setSearchQuery("A1");
    expect(document.querySelectorAll(".workflow-collection-group")).toHaveLength(1);
  });

  it("drops a selected tag that no longer exists and keeps the badge at the unfiltered total", async () => {
    ipcMocks.listWorkflows.mockResolvedValue(data);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    mgr.toggleTagFilter("z");
    expect(rowNames()).toEqual(["Gamma"]);
    expect(document.querySelector(".activity-badge")!.textContent).toBe("3");
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "Alpha", ["x"]), sum("3", "Gamma", [])]);
    await mgr.refreshWorkflowList();
    expect(mgr.selectedTagKeys.size).toBe(0);
    expect(rowNames()).toHaveLength(2);
  });

  it("deselects rows that a filter hides so bulk actions never reach them", async () => {
    ipcMocks.listWorkflows.mockResolvedValue(data);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    mgr.selectedIds.add("1");
    mgr.selectedIds.add("3");
    mgr.setSearchQuery("alpha");
    expect([...mgr.selectedIds]).toEqual(["1"]);
  });

  it("names the selected tags in the empty result and clears them from there", async () => {
    ipcMocks.listWorkflows.mockResolvedValue(data);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    mgr.toggleTagFilter("y");
    mgr.toggleTagFilter("z");
    const msg = document.getElementById("wf-search-no-match")!;
    expect(msg.textContent).toContain("No workflows tagged y and z");
    msg.querySelector("button")!.click();
    expect(rows()).toHaveLength(3);
  });

  it("shows the open workflow's in-memory tags instead of the stored ones", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "Alpha", ["old"])]);
    const { mgr } = make();
    mgr.currentId = "1";
    mgr.currentTags = ["live"];
    await mgr.refreshWorkflowList();
    expect(rows()[0].querySelector(".workflow-item-tag")!.textContent).toBe("live");
  });
});

describe("tag filter bar", () => {
  it("is hidden when the library has no tags", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "Alpha")]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    expect(document.getElementById("wf-tag-filter")!.classList.contains("hidden")).toBe(true);
  });

  it("builds pressed-state chips with faceted counts that narrow the list", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A", ["x", "y"]), sum("2", "B", ["x"]), sum("3", "C", ["z"])]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    expect(chip("x").querySelector(".wf-tag-filter-count")!.textContent).toBe("2");
    chip("x").click();
    expect(chip("x").getAttribute("aria-pressed")).toBe("true");
    expect(rowNames()).toEqual(["A", "B"]);
    expect(document.querySelector(".wf-tag-filter-chip[aria-pressed=\"false\"] .wf-tag-filter-label")!.textContent).toBe("y");
    expect(chip("z")).toBeUndefined();
  });

  it("picks up a tag added elsewhere without a reload", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A", ["x"])]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A", ["x", "fresh"])]);
    await mgr.refreshWorkflowList();
    expect(chip("fresh")).toBeTruthy();
  });

  it("collapses long tag lists behind a toggle with aria-expanded", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A", ["a", "b", "c", "d", "e", "f"])]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    expect(document.querySelectorAll(".wf-tag-filter-chip")).toHaveLength(4);
    const toggle = document.querySelector<HTMLElement>(".wf-tag-filter-toggle")!;
    expect(toggle.textContent).toBe("+2 more");
    toggle.click();
    expect(document.querySelectorAll(".wf-tag-filter-chip")).toHaveLength(6);
    expect(document.querySelector(".wf-tag-filter-toggle")!.getAttribute("aria-expanded")).toBe("true");
  });

  it("toggles the filter from a row's tag chip without loading the workflow", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A", ["x"]), sum("2", "B", ["y"])]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    rows()[0].querySelector<HTMLElement>(".workflow-item-tag")!.click();
    expect(mgr.selectedTagKeys.has(rows()[0].querySelector(".workflow-item-tag")!.textContent!.toLowerCase())).toBe(true);
    expect(ipcMocks.loadWorkflow).not.toHaveBeenCalled();
  });
});

describe("setWorkflowTags", () => {
  it("rewrites only the tags of a stored workflow", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "Alpha", ["keep", "drop"])]);
    ipcMocks.loadWorkflow.mockResolvedValue(stored("1", "Alpha", ["keep", "drop"]));
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    await mgr.setWorkflowTags(["1"], { add: ["New"], remove: ["drop"] });
    const saved = JSON.parse(ipcMocks.saveWorkflow.mock.calls[0][0]);
    expect(saved.metadata.tags).toEqual(["keep", "New"]);
    expect(saved.name).toBe("Alpha");
    expect(saved.id).toBe("1");
  });

  it("changes the open workflow in memory and never writes to disk", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "Alpha", ["a"])]);
    const { mgr, cb } = make();
    mgr.currentId = "1";
    mgr.currentTags = ["a"];
    await mgr.refreshWorkflowList();
    await mgr.setWorkflowTags(["1"], { add: ["b"], remove: [] });
    expect(ipcMocks.saveWorkflow).not.toHaveBeenCalled();
    expect(mgr.currentTags).toEqual(["a", "b"]);
    expect(cb.onUnsaved).toHaveBeenCalledWith(true);
    expect(rows()[0].textContent).toContain("b");
  });

  it("skips the save when the edit changes nothing", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "Alpha", ["a"])]);
    ipcMocks.loadWorkflow.mockResolvedValue(stored("1", "Alpha", ["a"]));
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    await mgr.setWorkflowTags(["1"], { add: ["A"], remove: [] });
    expect(ipcMocks.saveWorkflow).not.toHaveBeenCalled();
  });

  it("reports how many workflows were updated when one save fails", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A"), sum("2", "B")]);
    ipcMocks.loadWorkflow.mockImplementation(async (id: string) => stored(id, id));
    ipcMocks.saveWorkflow.mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("disk"));
    const { mgr, cb } = make();
    await mgr.refreshWorkflowList();
    await mgr.setWorkflowTags(["1", "2"], { add: ["t"], remove: [] });
    expect(cb.onToast).toHaveBeenCalledWith("Updated 1 of 2 workflows", "error");
  });

  it("keeps tags that exist on only some of the selected workflows", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A", ["only-a"]), sum("2", "B", ["only-b"])]);
    ipcMocks.loadWorkflow.mockImplementation(async (id: string) => stored(id, id, id === "1" ? ["only-a"] : ["only-b"]));
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    await mgr.setWorkflowTags(["1", "2"], { add: ["shared"], remove: [] });
    const tags = ipcMocks.saveWorkflow.mock.calls.map(c => JSON.parse(c[0]).metadata.tags);
    expect(tags).toEqual([["only-a", "shared"], ["only-b", "shared"]]);
  });
});

describe("row menu", () => {
  it("offers Rename, Duplicate, Move to…, Edit tags… and Delete in that order", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A")]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    rows()[0].querySelector<HTMLElement>(".workflow-item-more")!.click();
    expect(menuLabels()).toEqual(["Rename", "Duplicate", "Move to…", "Edit tags…", "Delete"]);
  });

  it("applies to the whole selection with counted labels and no Rename", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A"), sum("2", "B"), sum("3", "C")]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    mgr.selectedIds.add("1");
    mgr.selectedIds.add("2");
    rows()[0].dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    expect(menuLabels()).toEqual(["Duplicate 2 workflows", "Move 2 workflows to…", "Edit tags (2)…", "Delete 2 workflows"]);
  });

  it("acts on the row alone when the right-clicked row is not selected", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A"), sum("2", "B"), sum("3", "C")]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    mgr.selectedIds.add("1");
    mgr.selectedIds.add("2");
    rows()[2].dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    expect(menuLabels()).toContain("Rename");
  });

  it("is operable from the keyboard: ContextMenu key, Down to Move to…, Enter", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A")]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    const row = rows()[0];
    row.focus();
    row.dispatchEvent(new KeyboardEvent("keydown", { key: "ContextMenu", bubbles: true, cancelable: true }));
    const press = (k: string) => document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
    expect(document.activeElement!.getAttribute("aria-label")).toBe("Rename");
    press("ArrowDown");
    press("ArrowDown");
    press("Enter");
    expect(document.getElementById("wf-dropdown")).toBeTruthy();
    expect(menuLabels("wf-dropdown")).toEqual(["Uncategorized", "New collection…"]);
  });

  it("does not load the workflow when Enter is pressed on a row's action button", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A")]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    rows()[0].querySelector<HTMLElement>(".workflow-item-more")!
      .dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    expect(ipcMocks.loadWorkflow).not.toHaveBeenCalled();
  });

  it("marks the workflow's current collection as disabled in Move to…", async () => {
    ipcMocks.getSetting.mockResolvedValue(JSON.stringify({
      collections: [{ id: "c1", name: "Ops", color: "blue", order: 0, collapsed: false }], uncategorizedCollapsed: false }));
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A", [], "c1")]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    rows()[0].querySelector<HTMLElement>(".workflow-item-more")!.click();
    menuItem("Move to…").click();
    expect(menuItem("Ops", "wf-dropdown").getAttribute("aria-disabled")).toBe("true");
    expect(menuItem("Uncategorized", "wf-dropdown").getAttribute("aria-disabled")).toBeNull();
  });

  it("moves a single row into a new collection even with nothing selected", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A")]);
    ipcMocks.loadWorkflow.mockResolvedValue(stored("1", "A"));
    const { mgr, cb } = make();
    await mgr.refreshWorkflowList();
    rows()[0].querySelector<HTMLElement>(".workflow-item-more")!.click();
    menuItem("Move to…").click();
    menuItem("New collection…", "wf-dropdown").click();
    await flush();
    const input = document.querySelector<HTMLInputElement>(".workflow-collection-rename-input")!;
    input.value = "Fresh";
    input.dispatchEvent(new Event("blur"));
    await vi.waitFor(() => expect(cb.onToast).toHaveBeenCalledWith(expect.stringContaining("Moved 1 workflow"), "success"));
    const saved = JSON.parse(ipcMocks.saveWorkflow.mock.calls[0][0]);
    expect(saved.id).toBe("1");
    expect(saved.metadata.collection_id).toBe(mgr.collections[0].id);
    await mgr.refreshWorkflowList();
  });
});

describe("uncategorized group", () => {
  it("explains itself and offers a New workflow button but no options menu", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([sum("1", "A")]);
    const { mgr } = make();
    await mgr.refreshWorkflowList();
    const header = document.querySelector(".workflow-collection-header")!;
    expect(header.querySelector(".workflow-collection-name")!.getAttribute("title")).toContain("can't be renamed or deleted");
    expect(header.querySelector(".workflow-collection-new-btn")!.getAttribute("aria-label")).toBe("New workflow");
    expect(header.querySelector(".workflow-collection-menu-btn")).toBeNull();
  });
});

describe("collections", () => {
  it("starts each manager with no collections even after another created one", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([]);
    const first = make().mgr;
    await first.createCollection();
    const second = make().mgr;
    await second.refreshWorkflowList();
    expect(second.collections).toHaveLength(0);
  });
});

describe("deserialize", () => {
  it("cleans dirty tags in memory", () => {
    const json = JSON.stringify({ id: "1", name: "A", nodes: [], edges: [], metadata: { tags: ["Prod", "prod", 5, " x ", ""] } });
    expect(deserialize(json).tags).toEqual(["Prod", "x"]);
  });
});
