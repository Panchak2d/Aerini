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

describe("WorkflowManager.handleLoad fit", () => {
  beforeEach(() => {
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};
    ipcMocks.listWorkflows.mockResolvedValue([]);
    ipcMocks.loadWorkflow.mockResolvedValue(JSON.stringify({ id: "wf-new", name: "New", nodes: [], edges: [] }));
    document.body.innerHTML = '<div id="output-drawer"></div>';
    localStorage.clear();
  });
  afterEach(() => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    vi.clearAllMocks();
  });

  it("fits only after the output drawer has collapsed and currentId is the loaded workflow", async () => {
    let drawerHidden: boolean | null = null;
    let idAtFit: string | null = null;
    const manager: WorkflowManager = new WorkflowManager(
      {
        nodes: new Map(), connectors: new Map(), clearSelection: vi.fn(), warnArityViolations: vi.fn(),
        fitToScreen: vi.fn(() => {
          drawerHidden = document.getElementById("output-drawer")!.classList.contains("hidden");
          idAtFit = manager.currentId;
        }),
      } as unknown as Canvas,
      { onUnsaved: vi.fn(), onTitle: vi.fn(), onStatus: vi.fn(), onToast: vi.fn(), confirm: vi.fn().mockResolvedValue(true) },
    );
    await manager.handleLoad("wf-new");
    expect(drawerHidden).toBe(true);
    expect(idAtFit).toBe("wf-new");
  });
});
