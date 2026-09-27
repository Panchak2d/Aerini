// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level)
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
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowSummary } from "../ipc/workflow";

function makeManager() {
  const canvas = {
    nodes: new Map(), connectors: new Map(),
    clearSelection: vi.fn(), fitToScreen: vi.fn(),
  } as unknown as Canvas;
  return new WorkflowManager(canvas, {
    onUnsaved: vi.fn(), onTitle: vi.fn(), onStatus: vi.fn(), onToast: vi.fn(),
    confirm: vi.fn().mockResolvedValue(true),
  });
}

function setTauri(on: boolean): void {
  if (on) (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};
  else delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
}

function makeWorkflows(n: number): WorkflowSummary[] {
  return Array.from({ length: n }, (_, i) => ({
    id: `wf-${i}`, name: `Workflow ${i}`, updated_at: "2024-01-01T00:00:00Z", tags: [],
  }));
}

beforeEach(() => {
  document.body.innerHTML = `<div id="workflow-list"></div>`;
  for (const fn of Object.values(ipcMocks)) fn.mockReset();
  ipcMocks.getSetting.mockResolvedValue(null);
  setTauri(true);
});
afterEach(() => setTauri(false));

describe("refreshWorkflowList — roving tabindex", () => {
  it("falls back to making the first row tabbable when no workflow is open yet", async () => {
    ipcMocks.listWorkflows.mockResolvedValue(makeWorkflows(3));
    const mgr = makeManager();
    await mgr.refreshWorkflowList();

    const items = document.querySelectorAll<HTMLElement>(".workflow-item");
    expect(items[0].tabIndex).toBe(0);
    expect(items[1].tabIndex).toBe(-1);
    expect(items[2].tabIndex).toBe(-1);
  });

  it("makes the currently-open workflow tabbable and marks it aria-current, instead of the first row", async () => {
    const wfs = makeWorkflows(3);
    ipcMocks.listWorkflows.mockResolvedValue(wfs);
    const mgr = makeManager();
    mgr.currentId = "wf-2";
    await mgr.refreshWorkflowList();

    const items = document.querySelectorAll<HTMLElement>(".workflow-item");
    expect(items[0].tabIndex).toBe(-1);
    expect(items[2].tabIndex).toBe(0);
    expect(items[2].getAttribute("aria-current")).toBe("true");
    expect(items[2].classList.contains("active")).toBe(true);
  });
});

describe("refreshWorkflowList — load failure", () => {
  it("shows a distinct 'couldn't load' state with Retry, not the plain empty-library message", async () => {
    ipcMocks.listWorkflows.mockRejectedValue(new Error("db locked"));
    const mgr = makeManager();
    await mgr.refreshWorkflowList();

    const list = document.getElementById("workflow-list")!;
    expect(list.textContent).toContain("Couldn't load your workflows");
    expect(list.textContent).not.toContain("No workflows yet");
    expect(list.querySelector("button")?.textContent).toBe("Retry");
  });

  it("Retry re-fetches and renders normally once the underlying call succeeds", async () => {
    ipcMocks.listWorkflows.mockRejectedValueOnce(new Error("db locked"));
    ipcMocks.listWorkflows.mockResolvedValueOnce(makeWorkflows(2));
    const mgr = makeManager();
    await mgr.refreshWorkflowList();

    document.querySelector<HTMLButtonElement>(".workflow-list-empty button")!.click();
    await vi.waitFor(() => {
      expect(document.querySelectorAll(".workflow-item").length).toBe(2);
    });
  });
});

describe("collection header — double-click-to-rename does not flicker collapse state", () => {
  it("stays expanded through the two clicks that precede a dblclick on the collection name", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([
      { id: "wf-1", name: "A", updated_at: "2024-01-01T00:00:00Z", tags: [], collection_id: "col-1" },
    ]);
    ipcMocks.getSetting.mockImplementation((key: string) =>
      Promise.resolve(key === "workflow_collections_v1"
        ? JSON.stringify({ collections: [{ id: "col-1", name: "Folder", color: "blue", order: 0, collapsed: false }], uncategorizedCollapsed: false })
        : null)
    );
    const mgr = makeManager();
    await mgr.refreshWorkflowList();

    const group = document.querySelector<HTMLElement>(".workflow-collection-group")!;
    const name  = group.querySelector<HTMLElement>(".workflow-collection-name")!;

    // Simulate the real click/click/dblclick sequence a double-click on the
    // name dispatches; without the e.detail guard this would toggle
    // collapsed -> expanded -> collapsed before the rename input ever showed.
    name.dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 1 }));
    name.dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 2 }));

    expect(group.classList.contains("collapsed")).toBe(false);
  });
});

describe("browser mode -- deleting a workflow", () => {
  const LS_KEY = "aerini_workflows_v1";

  beforeEach(() => {
    setTauri(false);
    localStorage.clear();
    localStorage.setItem(LS_KEY, JSON.stringify({
      "wf-1": { id: "wf-1", name: "A", json: "{}", updated_at: "2024-01-01T00:00:00Z", tags: [], collection_id: null },
    }));
  });

  afterEach(() => vi.restoreAllMocks());

  it("removes the workflow from localStorage and the rendered list", async () => {
    const mgr = makeManager();
    await mgr.refreshWorkflowList();

    document.querySelector<HTMLButtonElement>(".workflow-item-del")!.click();

    await vi.waitFor(() => {
      expect(document.querySelectorAll(".workflow-item").length).toBe(0);
    });
    expect(JSON.parse(localStorage.getItem(LS_KEY)!)).toEqual({});
  });

  it("logs the error instead of swallowing it when localStorage cannot be written", async () => {
    const mgr = makeManager();
    await mgr.refreshWorkflowList();
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("quota"); });

    document.querySelector<HTMLButtonElement>(".workflow-item-del")!.click();

    await vi.waitFor(() => {
      expect(err).toHaveBeenCalledWith("Aerini: localStorage delete failed", expect.any(Error));
    });
  });
});
