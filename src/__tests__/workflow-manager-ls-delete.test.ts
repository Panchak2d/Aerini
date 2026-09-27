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
import type { Canvas } from "../canvas/Canvas";

const LS_KEY = "aerini_workflows_v1";

function seed(ids: string[]): void {
  const all: Record<string, unknown> = {};
  for (const id of ids) all[id] = { id, name: `WF ${id}`, json: "{}", updated_at: "2024-01-01T00:00:00Z", tags: [] };
  localStorage.setItem(LS_KEY, JSON.stringify(all));
}
const stored = (): string[] => Object.keys(JSON.parse(localStorage.getItem(LS_KEY) ?? "{}"));

function makeManager() {
  const canvas = {
    nodes: new Map(), connectors: new Map(),
    clearSelection: vi.fn(), fitToScreen: vi.fn(),
  } as unknown as Canvas;
  const onToast = vi.fn();
  const mgr = new WorkflowManager(canvas, {
    onUnsaved: vi.fn(), onTitle: vi.fn(), onStatus: vi.fn(), onToast,
    confirm: vi.fn().mockResolvedValue(true),
  });
  return { mgr, onToast };
}

beforeEach(() => {
  document.body.innerHTML = `<div id="workflow-list"></div><button id="wf-bulk-delete">Delete</button>`;
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  localStorage.clear();
  for (const fn of Object.values(ipcMocks)) fn.mockReset();
  ipcMocks.getSetting.mockResolvedValue(null);
});

afterEach(() => vi.restoreAllMocks());

describe("browser-mode delete", () => {
  it("single delete removes the workflow from storage", async () => {
    seed(["a", "b"]);
    const { mgr, onToast } = makeManager();
    await mgr.refreshWorkflowList();
    document.querySelector<HTMLButtonElement>(".workflow-item-del")!.click();
    await vi.waitFor(() => expect(stored().length).toBe(1));
    expect(onToast).not.toHaveBeenCalledWith(expect.stringContaining("failed"), "error");
  });

  it("single delete whose storage write fails: error toast, the workflow stays and its button is usable again", async () => {
    seed(["a"]);
    const { mgr, onToast } = makeManager();
    await mgr.refreshWorkflowList();
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("quota"); });
    vi.spyOn(console, "error").mockImplementation(() => {});
    const btn = document.querySelector<HTMLButtonElement>(".workflow-item-del")!;
    btn.click();
    await vi.waitFor(() => expect(onToast).toHaveBeenCalledWith(expect.stringContaining("Delete failed"), "error"));
    expect(stored()).toEqual(["a"]);
    expect(btn.disabled).toBe(false);
  });

  it("bulk delete with one failed write reports the shortfall instead of 'Deleted N'", async () => {
    seed(["a", "b"]);
    const { mgr, onToast } = makeManager();
    (mgr as unknown as { selectedIds: Set<string> }).selectedIds = new Set(["a", "b"]);
    const realSet = Storage.prototype.setItem;
    vi.spyOn(Storage.prototype, "setItem")
      .mockImplementationOnce(() => { throw new Error("quota"); })
      .mockImplementation(function (this: Storage, k: string, v: string) { realSet.call(this, k, v); });
    vi.spyOn(console, "error").mockImplementation(() => {});
    document.getElementById("wf-bulk-delete")!.click();
    await vi.waitFor(() => expect(onToast).toHaveBeenCalled());

    expect(onToast).toHaveBeenCalledWith("Deleted 1 of 2 workflows; 1 could not be deleted", "error");
    expect(onToast).not.toHaveBeenCalledWith(expect.stringMatching(/^Deleted 2/), "success");
  });
});
