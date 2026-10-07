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
}));
vi.mock("../ipc/workflow", () => ipcMocks);

import { WorkflowManager } from "../workflow-manager";
import { serialize } from "../canvas/CanvasSerializer";
import type { Canvas } from "../canvas/Canvas";

function makeManager() {
  const onToast   = vi.fn();
  const onTitle   = vi.fn();
  const onUnsaved = vi.fn();
  const onStatus  = vi.fn();
  const confirm   = vi.fn().mockResolvedValue(true);
  const canvas = {
    nodes: new Map(), connectors: new Map(),
    clearSelection: vi.fn(), fitToScreen: vi.fn(), warnArityViolations: vi.fn(),
  } as unknown as Canvas;
  const mgr = new WorkflowManager(canvas, { onUnsaved, onTitle, onStatus, onToast, confirm });
  mgr.currentId = "wf-1";
  mgr.currentName = "Test Workflow";
  return { mgr, onToast, onTitle, onUnsaved, canvas };
}

function setTauri(on: boolean): void {
  if (on) (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};
  else delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
}

beforeEach(() => {
  for (const fn of Object.values(ipcMocks)) fn.mockReset();
});
afterEach(() => setTauri(false));

describe("getVersions", () => {
  it("returns [] and makes no IPC call outside Tauri", async () => {
    setTauri(false);
    const { mgr } = makeManager();
    await expect(mgr.getVersions()).resolves.toEqual([]);
    expect(ipcMocks.listVersions).not.toHaveBeenCalled();
  });

  it("returns listVersions(currentId) inside Tauri", async () => {
    setTauri(true);
    const rows = [{ id: "v1", workflow_id: "wf-1", message: null, created_at: "2024-01-01T00:00:00Z" }];
    ipcMocks.listVersions.mockResolvedValue(rows);
    const { mgr } = makeManager();
    await expect(mgr.getVersions()).resolves.toEqual(rows);
    expect(ipcMocks.listVersions).toHaveBeenCalledWith("wf-1");
  });

  it("returns [] instead of throwing when listVersions rejects", async () => {
    setTauri(true);
    ipcMocks.listVersions.mockRejectedValue(new Error("db error"));
    const { mgr } = makeManager();
    await expect(mgr.getVersions()).resolves.toEqual([]);
  });
});

describe("getVersionJson", () => {
  it("returns null outside Tauri", async () => {
    setTauri(false);
    const { mgr } = makeManager();
    await expect(mgr.getVersionJson("v1")).resolves.toBeNull();
    expect(ipcMocks.getVersion).not.toHaveBeenCalled();
  });

  it("returns the snapshot JSON inside Tauri", async () => {
    setTauri(true);
    ipcMocks.getVersion.mockResolvedValue('{"id":"wf-1"}');
    const { mgr } = makeManager();
    await expect(mgr.getVersionJson("v1")).resolves.toBe('{"id":"wf-1"}');
    expect(ipcMocks.getVersion).toHaveBeenCalledWith("v1");
  });

  it("returns null instead of throwing when getVersion rejects", async () => {
    setTauri(true);
    ipcMocks.getVersion.mockRejectedValue(new Error("not found"));
    const { mgr } = makeManager();
    await expect(mgr.getVersionJson("v1")).resolves.toBeNull();
  });
});

describe("getCurrentJson", () => {
  it("serializes the live canvas without any IPC call, regardless of Tauri state", () => {
    setTauri(false);
    const { mgr } = makeManager();
    mgr.currentTags = ["prod"];
    const json = mgr.getCurrentJson();
    const doc = JSON.parse(json);
    expect(doc.id).toBe("wf-1");
    expect(doc.name).toBe("Test Workflow");
    expect(doc.metadata.tags).toEqual(["prod"]);
    expect(ipcMocks.saveWorkflow).not.toHaveBeenCalled();
  });
});

describe("restoreVersion", () => {
  it("returns null and makes no IPC call outside Tauri", async () => {
    setTauri(false);
    const { mgr } = makeManager();
    await expect(mgr.restoreVersion("v1")).resolves.toBeNull();
    expect(ipcMocks.getVersion).not.toHaveBeenCalled();
  });

  it("normal case: backs up current state, restores the snapshot, and reports the new id/name", async () => {
    setTauri(true);
    ipcMocks.loadWorkflow.mockResolvedValue('{"id":"wf-1","name":"Before"}');
    ipcMocks.saveVersion.mockResolvedValue(undefined);
    const snapshot = serialize("wf-old", "Restored Name", new Map(), new Map(), undefined, undefined, undefined, ["archived"]);
    ipcMocks.getVersion.mockResolvedValue(snapshot);
    const { mgr, onTitle, onUnsaved, canvas } = makeManager();

    const result = await mgr.restoreVersion("v1");
    expect(canvas.warnArityViolations).toHaveBeenCalledOnce();

    expect(ipcMocks.saveVersion).toHaveBeenCalledWith("wf-1", '{"id":"wf-1","name":"Before"}', "Before restore");
    expect(result).toEqual({ id: "wf-old", name: "Restored Name" });
    expect(mgr.currentId).toBe("wf-old");
    expect(mgr.currentName).toBe("Restored Name");
    expect(mgr.currentTags).toEqual(["archived"]);
    expect(onTitle).toHaveBeenCalledWith("Restored Name");
    expect(onUnsaved).toHaveBeenCalledWith(true);
  });

  it("edge case: skips the pre-restore backup when the workflow was never persisted", async () => {
    setTauri(true);
    ipcMocks.loadWorkflow.mockResolvedValue(null);
    const snapshot = serialize("wf-old", "Restored", new Map(), new Map());
    ipcMocks.getVersion.mockResolvedValue(snapshot);
    const { mgr } = makeManager();

    const result = await mgr.restoreVersion("v1");

    expect(ipcMocks.saveVersion).not.toHaveBeenCalled();
    expect(result).toEqual({ id: "wf-old", name: "Restored" });
  });

  it("returns null without throwing when the requested version does not exist", async () => {
    setTauri(true);
    ipcMocks.loadWorkflow.mockResolvedValue(null);
    ipcMocks.getVersion.mockResolvedValue(null);
    const { mgr } = makeManager();
    await expect(mgr.restoreVersion("missing")).resolves.toBeNull();
  });

  it("returns null (caught) when getVersion rejects", async () => {
    setTauri(true);
    ipcMocks.loadWorkflow.mockResolvedValue(null);
    ipcMocks.getVersion.mockRejectedValue(new Error("db error"));
    const { mgr } = makeManager();
    await expect(mgr.restoreVersion("v1")).resolves.toBeNull();
  });

  it("propagates (does not swallow) a failure in the pre-restore backup itself", async () => {
    setTauri(true);
    ipcMocks.loadWorkflow.mockResolvedValue('{"id":"wf-1"}');
    ipcMocks.saveVersion.mockRejectedValue(new Error("disk full"));
    const { mgr } = makeManager();
    await expect(mgr.restoreVersion("v1")).rejects.toThrow("disk full");
  });
});

describe("deleteVersion", () => {
  it("makes no IPC call outside Tauri", async () => {
    setTauri(false);
    const { mgr } = makeManager();
    await mgr.deleteVersion("v1");
    expect(ipcMocks.deleteVersion).not.toHaveBeenCalled();
  });

  it("calls the IPC delete inside Tauri", async () => {
    setTauri(true);
    ipcMocks.deleteVersion.mockResolvedValue(undefined);
    const { mgr } = makeManager();
    await mgr.deleteVersion("v1");
    expect(ipcMocks.deleteVersion).toHaveBeenCalledWith("v1");
  });
});

describe("saveNamedVersion", () => {
  it("returns false and makes no IPC call outside Tauri", async () => {
    setTauri(false);
    const { mgr } = makeManager();
    await expect(mgr.saveNamedVersion("msg")).resolves.toBe(false);
    expect(ipcMocks.saveVersion).not.toHaveBeenCalled();
  });

  it("returns false without saving a version when the workflow was never persisted", async () => {
    setTauri(true);
    ipcMocks.loadWorkflow.mockResolvedValue(null);
    const { mgr } = makeManager();
    await expect(mgr.saveNamedVersion("msg")).resolves.toBe(false);
    expect(ipcMocks.saveVersion).not.toHaveBeenCalled();
  });

  it("saves the serialized canvas with the trimmed message and returns true", async () => {
    setTauri(true);
    ipcMocks.loadWorkflow.mockResolvedValue('{"id":"wf-1"}');
    ipcMocks.saveVersion.mockResolvedValue(undefined);
    const { mgr } = makeManager();
    mgr.currentTags = ["prod", "webhook"];

    await expect(mgr.saveNamedVersion("  Checkpoint  ")).resolves.toBe(true);

    expect(ipcMocks.saveVersion).toHaveBeenCalledTimes(1);
    const [wfId, json, message] = ipcMocks.saveVersion.mock.calls[0];
    expect(wfId).toBe("wf-1");
    expect(JSON.parse(json).id).toBe("wf-1");
    expect(JSON.parse(json).metadata.tags).toEqual(["prod", "webhook"]);
    expect(message).toBe("Checkpoint");
  });

  it("passes undefined (not an empty string) for a whitespace-only message", async () => {
    setTauri(true);
    ipcMocks.loadWorkflow.mockResolvedValue('{"id":"wf-1"}');
    ipcMocks.saveVersion.mockResolvedValue(undefined);
    const { mgr } = makeManager();

    await mgr.saveNamedVersion("   ");

    expect(ipcMocks.saveVersion.mock.calls[0][2]).toBeUndefined();
  });
});
