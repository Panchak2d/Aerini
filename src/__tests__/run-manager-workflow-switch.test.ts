/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import { RunManager } from "../run-manager";

function setDom(): void {
  document.body.innerHTML = `
    <div id="output-drawer" class="hidden">
      <div id="drawer-tabs"><button id="tab-summary">Summary</button></div>
      <div id="output-content"><div class="run-placeholder">stale content from a previous run</div></div>
    </div>
  `;
}

function makeManager(): RunManager {
  return new RunManager({} as unknown as Canvas, vi.fn(), vi.fn());
}

describe("RunManager.setCurrentWorkflow — output panel reset", () => {
  it("normal case: switching to a different workflow clears stale tabs/content back to the idle placeholder", () => {
    setDom();
    const rm = makeManager();
    rm.setCurrentWorkflow("wf-1", "First");
    // Simulate a previous run having written real results into the drawer for wf-1.
    document.getElementById("drawer-tabs")!.innerHTML = `<button id="tab-summary">Summary</button>`;
    document.getElementById("output-content")!.innerHTML = `<div class="success-msg">wf-1's old result</div>`;

    rm.setCurrentWorkflow("wf-2", "Second"); // navigate to a different workflow

    expect(document.getElementById("drawer-tabs")!.innerHTML).toBe("");
    expect(document.getElementById("output-content")!.querySelector(".success-msg")).toBeNull();
    expect(document.getElementById("output-idle")).not.toBeNull();
  });

  it("edge case: does not touch the drawer while a run is still in flight, so it can't fight with that run's own DOM writes", () => {
    setDom();
    const rm = makeManager();
    rm.setCurrentWorkflow("wf-1", "First");
    document.getElementById("output-content")!.innerHTML = `<div class="run-placeholder">Running…</div>`;

    (rm as unknown as { state: { start(id: string): void } }).state.start("run-1");
    rm.setCurrentWorkflow("wf-2", "Second");

    expect(document.getElementById("output-content")!.innerHTML).toContain("Running…");
  });

  it("no-op navigation: calling setCurrentWorkflow again with the same id does not reset anything already on screen", () => {
    setDom();
    const rm = makeManager();
    rm.setCurrentWorkflow("wf-1", "First");
    document.getElementById("output-content")!.innerHTML = `<div class="success-msg">still here</div>`;

    rm.setCurrentWorkflow("wf-1", "First");

    expect(document.getElementById("output-content")!.querySelector(".success-msg")).not.toBeNull();
  });
});
