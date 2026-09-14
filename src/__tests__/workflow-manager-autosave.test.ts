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

import { WorkflowManager, setWorkflowRunning } from "../workflow-manager";
import type { Canvas } from "../canvas/Canvas";

const WF_ID = "wf_autosave_test";

function makeManager(onToast = vi.fn()) {
  const canvas = { nodes: new Map(), connectors: new Map() } as unknown as Canvas;
  const mgr = new WorkflowManager(canvas, {
    onUnsaved: vi.fn(), onTitle: vi.fn(), onStatus: vi.fn(), onToast,
    confirm: vi.fn().mockResolvedValue(true),
  });
  mgr.currentId = WF_ID;
  return { mgr, onToast };
}

function setTauri(on: boolean): void {
  if (on) (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};
  else delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
}

beforeEach(() => {
  document.body.innerHTML = "";
  for (const fn of Object.values(ipcMocks)) fn.mockReset();
  setTauri(true);
  setWorkflowRunning(WF_ID, false);
  vi.useFakeTimers();
});
afterEach(() => {
  setWorkflowRunning(WF_ID, false);
  vi.useRealTimers();
  setTauri(false);
});

describe("scheduleAutoSave — debounce delay", () => {
  it("saves after ~500ms when the workflow is running (normal case)", async () => {
    ipcMocks.saveWorkflow.mockResolvedValue(undefined);
    setWorkflowRunning(WF_ID, true);
    const { mgr } = makeManager();

    mgr.markUnsaved(true);
    mgr.scheduleAutoSave();

    await vi.advanceTimersByTimeAsync(499);
    expect(ipcMocks.saveWorkflow).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);
    expect(ipcMocks.saveWorkflow).toHaveBeenCalledTimes(1);
  });

  it("keeps the 30s delay when the workflow is idle (edge case)", async () => {
    ipcMocks.saveWorkflow.mockResolvedValue(undefined);
    const { mgr } = makeManager();

    mgr.markUnsaved(true);
    mgr.scheduleAutoSave();

    await vi.advanceTimersByTimeAsync(29_999);
    expect(ipcMocks.saveWorkflow).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);
    expect(ipcMocks.saveWorkflow).toHaveBeenCalledTimes(1);
  });
});

describe("scheduleAutoSave — failure toast", () => {
  it("toasts when autosave fails while the workflow is running (normal case)", async () => {
    ipcMocks.saveWorkflow.mockRejectedValue(new Error("disk full"));
    setWorkflowRunning(WF_ID, true);
    const { mgr, onToast } = makeManager();

    mgr.markUnsaved(true);
    mgr.scheduleAutoSave();
    await vi.advanceTimersByTimeAsync(500);

    expect(onToast).toHaveBeenCalledWith(expect.stringContaining("Autosave failed"), "error");
  });

  it("does not toast when autosave fails while the workflow is idle (edge case)", async () => {
    ipcMocks.saveWorkflow.mockRejectedValue(new Error("disk full"));
    const { mgr, onToast } = makeManager();

    mgr.markUnsaved(true);
    mgr.scheduleAutoSave();
    await vi.advanceTimersByTimeAsync(30_000);

    expect(onToast).not.toHaveBeenCalled();
  });
});
