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

describe("refreshWorkflowList", () => {
  it("renders one row per workflow on a normal call", async () => {
    ipcMocks.listWorkflows.mockResolvedValue(makeWorkflows(3));
    const mgr = makeManager();
    await mgr.refreshWorkflowList();
    const list = document.getElementById("workflow-list")!;
    expect(list.querySelectorAll(".workflow-collection-group").length).toBe(1);
    expect(list.querySelectorAll(".workflow-item").length).toBe(3);
  });

  it("does not duplicate groups when a second call lands while the first is still in flight", async () => {
    ipcMocks.listWorkflows.mockResolvedValue(makeWorkflows(3));
    const mgr = makeManager();

    // Mirrors two scheduler-status events firing back to back: the second
    // call arrives before the first's async gap (collections load, then this
    // listWorkflows IPC call) has resolved.
    const p1 = mgr.refreshWorkflowList();
    const p2 = mgr.refreshWorkflowList();
    await Promise.all([p1, p2]);

    const list = document.getElementById("workflow-list")!;
    expect(list.querySelectorAll(".workflow-collection-group").length).toBe(1);
    expect(list.querySelectorAll(".workflow-item").length).toBe(3);
  });
});
