/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import { RunManager } from "../run-manager";

function setDom(): void {
  document.body.innerHTML = `
    <div id="output-drawer" class="hidden">
      <div id="drawer-tabs"></div>
      <div id="output-content"></div>
    </div>
  `;
}

function makeManager(): RunManager {
  const canvas = { resetAllStatus: vi.fn() } as unknown as Canvas;
  return new RunManager(canvas, vi.fn(), vi.fn());
}

describe("RunManager — onRunStateChange / addRunStateListener", () => {
  it("normal case: onRunStateChange and every addRunStateListener callback all fire for the same transition, none clobbering another", () => {
    setDom();
    const rm = makeManager();
    rm.setCurrentWorkflow("wf-1", "First");
    (rm as unknown as { state: { start(id: string): void } }).state.start("run-1");

    const legacy: boolean[] = [];
    const a: boolean[] = [];
    const b: boolean[] = [];
    rm.onRunStateChange = (running) => legacy.push(running);
    rm.addRunStateListener((running) => a.push(running));
    rm.addRunStateListener((running) => b.push(running));

    rm.forceReset();

    expect(legacy).toEqual([false]);
    expect(a).toEqual([false]);
    expect(b).toEqual([false]);
  });

  it("edge case: forceReset() while nothing is running is a no-op — no listener fires spuriously", () => {
    setDom();
    const rm = makeManager();
    rm.setCurrentWorkflow("wf-1", "First");

    const calls: boolean[] = [];
    rm.addRunStateListener((running) => calls.push(running));

    rm.forceReset();

    expect(calls).toEqual([]);
  });
});
