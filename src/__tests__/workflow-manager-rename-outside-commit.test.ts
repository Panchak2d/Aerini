// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";
import { WorkflowManager } from "../workflow-manager";
import type { WorkflowSummary } from "../ipc/workflow";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

beforeEach(() => {
  document.body.innerHTML = "";
});

function mousedownOn(target: EventTarget): void {
  target.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
}

// ---------------------------------------------------------------------------
// startRename — title bar rename (2a)
// ---------------------------------------------------------------------------

describe("startRename — commit on outside click", () => {
  it("normal case: a mousedown outside the input commits the trimmed value", async () => {
    const el = document.createElement("span");
    document.body.appendChild(el);
    const fakeThis = { currentName: "Old Name", markUnsaved: vi.fn() };

    const pending = WorkflowManager.prototype.startRename.call(fakeThis as never, el);
    const inp = document.querySelector("input.workflow-rename-input") as HTMLInputElement;
    inp.value = "New Title";

    // The dismiss listener attaches via setTimeout(0); flush it before clicking away.
    await new Promise(r => setTimeout(r, 0));
    mousedownOn(document.body);

    const result = await pending;
    expect(result).toBe("New Title");
    expect(fakeThis.currentName).toBe("New Title");
    expect(document.querySelector("input.workflow-rename-input")).toBeNull();
    expect(document.getElementById("workflow-name-label")?.textContent).toBe("New Title");
  });

  it("edge case: a mousedown on the input itself does not commit", async () => {
    const el = document.createElement("span");
    document.body.appendChild(el);
    const fakeThis = { currentName: "Old Name", markUnsaved: vi.fn() };

    WorkflowManager.prototype.startRename.call(fakeThis as never, el);
    const inp = document.querySelector("input.workflow-rename-input") as HTMLInputElement;
    inp.value = "Still Typing";

    await new Promise(r => setTimeout(r, 0));
    mousedownOn(inp);

    expect(document.querySelector("input.workflow-rename-input")).not.toBeNull();
    expect(fakeThis.currentName).toBe("Old Name");
  });
});

// ---------------------------------------------------------------------------
// startItemRename — sidebar workflow-list rename (2b)
// ---------------------------------------------------------------------------

describe("startItemRename — commit on outside click", () => {
  const wf: WorkflowSummary = { id: "wf1", name: "Old", updated_at: "2024-01-01T00:00:00Z", tags: [], collection_id: null };

  it("normal case: a mousedown outside the input persists the new name via renameWorkflowById", async () => {
    const nameEl = document.createElement("span");
    document.body.appendChild(nameEl);
    const fakeThis = { renameWorkflowById: vi.fn() };

    (WorkflowManager.prototype as unknown as { startItemRename: (n: HTMLElement, w: WorkflowSummary) => void })
      .startItemRename.call(fakeThis as never, nameEl, wf);
    const inp = document.querySelector("input.workflow-item-rename-input") as HTMLInputElement;
    inp.value = "New";

    await new Promise(r => setTimeout(r, 0));
    mousedownOn(document.body);

    expect(fakeThis.renameWorkflowById).toHaveBeenCalledWith("wf1", "New");
    expect(document.querySelector("input.workflow-item-rename-input")).toBeNull();
    expect(document.querySelector("span.workflow-item-name")?.textContent).toBe("New");
  });

  it("edge case: Escape cancels and a later outside click does not re-fire the commit", async () => {
    const nameEl = document.createElement("span");
    document.body.appendChild(nameEl);
    const fakeThis = { renameWorkflowById: vi.fn() };

    (WorkflowManager.prototype as unknown as { startItemRename: (n: HTMLElement, w: WorkflowSummary) => void })
      .startItemRename.call(fakeThis as never, nameEl, wf);
    const inp = document.querySelector("input.workflow-item-rename-input") as HTMLInputElement;
    inp.value = "Abandoned Edit";
    inp.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

    await new Promise(r => setTimeout(r, 0));
    mousedownOn(document.body);

    expect(fakeThis.renameWorkflowById).not.toHaveBeenCalled();
    expect(document.querySelector("span.workflow-item-name")?.textContent).toBe("Old");
  });
});
