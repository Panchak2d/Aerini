// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";
import type { VersionRow } from "../ipc/workflow";
import type { WorkflowManager } from "../workflow-manager";

const confirmMock = vi.hoisted(() => ({ showConfirm: vi.fn() }));
vi.mock("../confirm", () => ({ showConfirm: confirmMock.showConfirm }));

const diffPanelMock = vi.hoisted(() => ({ showDiffPanel: vi.fn() }));
vi.mock("../panels/DiffPanel", () => ({ showDiffPanel: diffPanelMock.showDiffPanel }));

import { showVersionPanel } from "../panels/VersionPanel";

function row(id: string, message: string | null, created_at = "2024-01-15T10:30:00Z"): VersionRow {
  return { id, workflow_id: "wf-1", message, created_at };
}

function makeWfManager(overrides: Partial<WorkflowManager> = {}): WorkflowManager {
  return {
    hasUnsaved: false,
    getVersions:     vi.fn().mockResolvedValue([]),
    getVersionJson:  vi.fn(),
    getCurrentJson:  vi.fn().mockReturnValue('{"id":"current"}'),
    restoreVersion:  vi.fn(),
    deleteVersion:   vi.fn().mockResolvedValue(undefined),
    saveNamedVersion: vi.fn(),
    ...overrides,
  } as unknown as WorkflowManager;
}

// Waits one microtask turn so the async body of showVersionPanel (the
// `await wfManager.getVersions()` at its top) has settled and rendered.
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  document.body.innerHTML = "";
  confirmMock.showConfirm.mockReset().mockResolvedValue(true);
  diffPanelMock.showDiffPanel.mockReset();
});

describe("showVersionPanel — list rendering", () => {
  it("shows the empty state when there are no versions", async () => {
    const wf = makeWfManager({ getVersions: vi.fn().mockResolvedValue([]) });
    await showVersionPanel(wf, vi.fn());
    await flush();
    expect(document.querySelector(".version-empty")?.textContent).toContain("No saved versions yet");
    expect(document.querySelectorAll(".version-item")).toHaveLength(0);
  });

  it("renders one item per version, newest (index 0) first as delivered by the manager", async () => {
    const versions = [row("v2", "Second"), row("v1", "First")];
    const wf = makeWfManager({ getVersions: vi.fn().mockResolvedValue(versions) });
    await showVersionPanel(wf, vi.fn());
    await flush();
    const items = document.querySelectorAll(".version-item");
    expect(items).toHaveLength(2);
    expect(items[0].querySelector(".version-item-name")?.textContent).toContain("Second");
  });

  it("falls back to 'Saved' for a version with no message", async () => {
    const wf = makeWfManager({ getVersions: vi.fn().mockResolvedValue([row("v1", null)]) });
    await showVersionPanel(wf, vi.fn());
    await flush();
    expect(document.querySelector(".version-item-name")?.textContent).toContain("Saved");
  });

  it("shows the Current badge on index 0 when there are no unsaved changes", async () => {
    const wf = makeWfManager({ hasUnsaved: false, getVersions: vi.fn().mockResolvedValue([row("v1", "A"), row("v2", "B")]) });
    await showVersionPanel(wf, vi.fn());
    await flush();
    const items = document.querySelectorAll(".version-item");
    expect(items[0].querySelector(".version-current-badge")).not.toBeNull();
    expect(items[1].querySelector(".version-current-badge")).toBeNull();
  });

  it("hides the Current badge on index 0 when there are unsaved changes", async () => {
    const wf = makeWfManager({ hasUnsaved: true, getVersions: vi.fn().mockResolvedValue([row("v1", "A")]) });
    await showVersionPanel(wf, vi.fn());
    await flush();
    expect(document.querySelector(".version-current-badge")).toBeNull();
  });
});

describe("showVersionPanel — search/filter", () => {
  it("hides non-matching items and shows the filter-empty message when nothing matches", async () => {
    const wf = makeWfManager({ getVersions: vi.fn().mockResolvedValue([row("v1", "Alpha release"), row("v2", "Bugfix")]) });
    await showVersionPanel(wf, vi.fn());
    await flush();

    const input = document.querySelector<HTMLInputElement>(".version-search-input")!;
    input.value = "alpha";
    input.dispatchEvent(new Event("input"));

    const items = document.querySelectorAll(".version-item");
    expect(items[0].classList.contains("hidden")).toBe(false);
    expect(items[1].classList.contains("hidden")).toBe(true);
    expect(document.querySelector(".version-filter-empty")?.classList.contains("hidden")).toBe(true);

    input.value = "nothing-matches-this";
    input.dispatchEvent(new Event("input"));
    expect(document.querySelector(".version-filter-empty")?.classList.contains("hidden")).toBe(false);
  });
});

describe("showVersionPanel — Compare", () => {
  it("fetches the version JSON and opens the diff panel against the current canvas", async () => {
    const wf = makeWfManager({
      getVersions: vi.fn().mockResolvedValue([row("v1", "Snap")]),
      getVersionJson: vi.fn().mockResolvedValue('{"id":"old"}'),
      getCurrentJson: vi.fn().mockReturnValue('{"id":"now"}'),
    });
    await showVersionPanel(wf, vi.fn());
    await flush();

    document.querySelector<HTMLButtonElement>(".version-compare-btn")!.click();
    await flush();

    expect(wf.getVersionJson).toHaveBeenCalledWith("v1");
    expect(diffPanelMock.showDiffPanel).toHaveBeenCalledWith(
      expect.stringContaining("Snap"), '{"id":"old"}', "Current canvas", '{"id":"now"}',
    );
  });

  it("toasts an error and does not open the diff panel when the version JSON is missing", async () => {
    const toast = vi.fn();
    const wf = makeWfManager({
      getVersions: vi.fn().mockResolvedValue([row("v1", "Snap")]),
      getVersionJson: vi.fn().mockResolvedValue(null),
    });
    await showVersionPanel(wf, toast);
    await flush();

    document.querySelector<HTMLButtonElement>(".version-compare-btn")!.click();
    await flush();

    expect(diffPanelMock.showDiffPanel).not.toHaveBeenCalled();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("Compare failed"), "error");
  });
});

describe("showVersionPanel — Restore", () => {
  it("asks for confirmation, then restores and toasts success", async () => {
    const toast = vi.fn();
    const wf = makeWfManager({
      getVersions: vi.fn().mockResolvedValue([row("v1", "Snap")]),
      restoreVersion: vi.fn().mockResolvedValue({ id: "wf-1", name: "Snap" }),
    });
    await showVersionPanel(wf, toast);
    await flush();

    document.querySelector<HTMLButtonElement>(".version-restore-btn")!.click();
    await flush();

    expect(confirmMock.showConfirm).toHaveBeenCalled();
    expect(wf.restoreVersion).toHaveBeenCalledWith("v1");
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("Restored to"), "success");
    // The panel closes as soon as the restore is confirmed, not after it resolves.
    expect(document.getElementById("version-panel-overlay")).toBeNull();
  });

  it("does not restore when the confirmation is declined", async () => {
    confirmMock.showConfirm.mockResolvedValue(false);
    const wf = makeWfManager({ getVersions: vi.fn().mockResolvedValue([row("v1", "Snap")]) });
    await showVersionPanel(wf, vi.fn());
    await flush();

    document.querySelector<HTMLButtonElement>(".version-restore-btn")!.click();
    await flush();

    expect(wf.restoreVersion).not.toHaveBeenCalled();
    expect(document.getElementById("version-panel-overlay")).not.toBeNull();
  });

  it("toasts a failure message when restoreVersion resolves to null", async () => {
    const toast = vi.fn();
    const wf = makeWfManager({
      getVersions: vi.fn().mockResolvedValue([row("v1", "Snap")]),
      restoreVersion: vi.fn().mockResolvedValue(null),
    });
    await showVersionPanel(wf, toast);
    await flush();

    document.querySelector<HTMLButtonElement>(".version-restore-btn")!.click();
    await flush();

    expect(toast).toHaveBeenCalledWith(expect.stringContaining("Restore failed"), "error");
  });

  it("toasts the error message when restoreVersion rejects", async () => {
    const toast = vi.fn();
    const wf = makeWfManager({
      getVersions: vi.fn().mockResolvedValue([row("v1", "Snap")]),
      restoreVersion: vi.fn().mockRejectedValue(new Error("disk full")),
    });
    await showVersionPanel(wf, toast);
    await flush();

    document.querySelector<HTMLButtonElement>(".version-restore-btn")!.click();
    await flush();

    expect(toast).toHaveBeenCalledWith(expect.stringContaining("disk full"), "error");
  });
});

describe("showVersionPanel — Delete", () => {
  it("asks for a dangerous confirmation, then deletes and re-renders without the item", async () => {
    const wf = makeWfManager({ getVersions: vi.fn().mockResolvedValue([row("v1", "A"), row("v2", "B")]) });
    await showVersionPanel(wf, vi.fn());
    await flush();

    document.querySelectorAll<HTMLButtonElement>(".version-delete-btn")[0].click();
    await flush();

    expect(confirmMock.showConfirm).toHaveBeenCalledWith(expect.any(String), true, "Delete");
    expect(wf.deleteVersion).toHaveBeenCalledWith("v1");
    expect(document.querySelectorAll(".version-item")).toHaveLength(1);
  });

  it("does not delete when the confirmation is declined", async () => {
    confirmMock.showConfirm.mockResolvedValue(false);
    const wf = makeWfManager({ getVersions: vi.fn().mockResolvedValue([row("v1", "A")]) });
    await showVersionPanel(wf, vi.fn());
    await flush();

    document.querySelector<HTMLButtonElement>(".version-delete-btn")!.click();
    await flush();

    expect(wf.deleteVersion).not.toHaveBeenCalled();
    expect(document.querySelectorAll(".version-item")).toHaveLength(1);
  });

  it("toasts an error and leaves the item in place when deleteVersion rejects", async () => {
    const toast = vi.fn();
    const wf = makeWfManager({
      getVersions: vi.fn().mockResolvedValue([row("v1", "A")]),
      deleteVersion: vi.fn().mockRejectedValue(new Error("locked")),
    });
    await showVersionPanel(wf, toast);
    await flush();

    document.querySelector<HTMLButtonElement>(".version-delete-btn")!.click();
    await flush();

    expect(toast).toHaveBeenCalledWith(expect.stringContaining("locked"), "error");
    expect(document.querySelectorAll(".version-item")).toHaveLength(1);
  });
});

describe("showVersionPanel — Save Snapshot", () => {
  it("saves, clears the input, and toasts success when a new version was actually created", async () => {
    const toast = vi.fn();
    const getVersions = vi.fn()
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([row("v1", "New snap")]);
    const wf = makeWfManager({ getVersions, saveNamedVersion: vi.fn().mockResolvedValue(true) });
    await showVersionPanel(wf, toast);
    await flush();

    const input = document.querySelector<HTMLInputElement>(".version-snapshot-input")!;
    input.value = "New snap";
    document.querySelector<HTMLButtonElement>(".version-snapshot-btn")!.click();
    await flush();

    expect(wf.saveNamedVersion).toHaveBeenCalledWith("New snap");
    expect(input.value).toBe("");
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("Snapshot saved"), "success");
  });

  it("toasts 'no changes' (info) when saveNamedVersion succeeds but the list length is unchanged", async () => {
    const toast = vi.fn();
    const versions = [row("v1", "Existing")];
    const wf = makeWfManager({
      getVersions: vi.fn().mockResolvedValue(versions),
      saveNamedVersion: vi.fn().mockResolvedValue(true),
    });
    await showVersionPanel(wf, toast);
    await flush();

    document.querySelector<HTMLButtonElement>(".version-snapshot-btn")!.click();
    await flush();

    expect(toast).toHaveBeenCalledWith(expect.stringContaining("No changes"), "info");
  });

  it("toasts an error and does not refetch when saveNamedVersion returns false (workflow never saved)", async () => {
    const toast = vi.fn();
    const getVersions = vi.fn().mockResolvedValue([]);
    const wf = makeWfManager({ getVersions, saveNamedVersion: vi.fn().mockResolvedValue(false) });
    await showVersionPanel(wf, toast);
    await flush();
    getVersions.mockClear();

    document.querySelector<HTMLButtonElement>(".version-snapshot-btn")!.click();
    await flush();

    expect(toast).toHaveBeenCalledWith(expect.stringContaining("Save the workflow"), "error");
    expect(getVersions).not.toHaveBeenCalled();
  });

  it("triggers the same save on Enter in the snapshot input", async () => {
    const wf = makeWfManager({ saveNamedVersion: vi.fn().mockResolvedValue(true) });
    await showVersionPanel(wf, vi.fn());
    await flush();

    const input = document.querySelector<HTMLInputElement>(".version-snapshot-input")!;
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    await flush();

    expect(wf.saveNamedVersion).toHaveBeenCalled();
  });
});

describe("showVersionPanel — overlay lifecycle", () => {
  it("replaces a prior version panel overlay instead of stacking a second one", async () => {
    const wf = makeWfManager();
    await showVersionPanel(wf, vi.fn());
    await flush();
    await showVersionPanel(wf, vi.fn());
    await flush();
    expect(document.querySelectorAll("#version-panel-overlay")).toHaveLength(1);
  });

  it("closes on close-button click and on backdrop click, not on click inside the panel", async () => {
    const wf = makeWfManager();
    await showVersionPanel(wf, vi.fn());
    await flush();

    document.querySelector(".version-panel")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.getElementById("version-panel-overlay")).not.toBeNull();

    document.querySelector<HTMLButtonElement>(".popover-close")!.click();
    expect(document.getElementById("version-panel-overlay")).toBeNull();
  });
});
