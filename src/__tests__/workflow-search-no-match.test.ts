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
import { bindWorkflowSectionControls } from "../sidebar-sections";
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

function wf(id: string, name: string): WorkflowSummary {
  return { id, name, updated_at: "2024-01-01T00:00:00Z", tags: [] };
}

function typeQuery(value: string): void {
  const search = document.getElementById("wf-search") as HTMLInputElement;
  search.value = value;
  search.dispatchEvent(new Event("input"));
}

describe("workflow search — 'no match' message", () => {
  let mgr: WorkflowManager;

  beforeEach(async () => {
    document.body.innerHTML = `
      <input id="wf-search" />
      <button id="btn-wf-sort" data-sort="updated_desc"></button>
      <div id="workflow-list"></div>
    `;
    for (const fn of Object.values(ipcMocks)) fn.mockReset();
    ipcMocks.getSetting.mockResolvedValue(null);
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};
    ipcMocks.listWorkflows.mockResolvedValue([wf("a", "Alpha"), wf("b", "Beta")]);
    mgr = makeManager();
    bindWorkflowSectionControls(() => {}, q => mgr.setSearchQuery(q));
    await mgr.refreshWorkflowList();
  });
  afterEach(() => { delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__; });

  it("shows a message naming the query when nothing matches", () => {
    typeQuery("zzz-nope");
    const msg = document.getElementById("wf-search-no-match");
    expect(msg).toBeTruthy();
    expect(msg!.textContent).toContain("zzz-nope");
  });

  it("removes the message once a match reappears or the query is cleared", () => {
    typeQuery("zzz-nope");
    expect(document.getElementById("wf-search-no-match")).toBeTruthy();

    typeQuery("Alpha");
    expect(document.getElementById("wf-search-no-match")).toBeFalsy();
    expect(document.querySelectorAll(".workflow-item")).toHaveLength(1);
  });

  it("never shows the message when the library is empty (the plain empty-library state already covers it)", async () => {
    ipcMocks.listWorkflows.mockResolvedValue([]);
    await mgr.refreshWorkflowList();
    typeQuery("anything");
    expect(document.getElementById("wf-search-no-match")).toBeFalsy();
    expect(document.getElementById("workflow-list")!.textContent).toContain("No workflows yet");
  });
});
