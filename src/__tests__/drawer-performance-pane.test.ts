/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import { RunManager } from "../run-manager";

function setDom(): void {
  document.body.innerHTML = `
    <div id="output-drawer">
      <div class="drawer-tabs" id="drawer-tabs">
        <button class="drawer-tab" id="tab-summary">Summary</button>
      </div>
      <button id="drawer-tab-performance" class="drawer-tab drawer-tab-static">Performance</button>
      <div id="output-content"><div id="output-idle">idle</div></div>
      <div id="output-content-performance" class="hidden"><div id="perf-empty">no data yet</div></div>
    </div>
  `;
}

function makeManager(): RunManager {
  return new RunManager({} as unknown as Canvas, vi.fn(), vi.fn());
}

describe("RunManager — output drawer Performance pane toggle (ERROR-RECOVERY)", () => {
  it("normal case: clicking the Performance tab hides #output-content and reveals #output-content-performance", () => {
    setDom();
    makeManager(); // constructor wires #drawer-tab-performance's click handler

    document.getElementById("drawer-tab-performance")!.click();

    expect(document.getElementById("output-content")!.classList.contains("hidden")).toBe(true);
    expect(document.getElementById("output-content-performance")!.classList.contains("hidden")).toBe(false);
    expect(document.getElementById("drawer-tab-performance")!.classList.contains("active")).toBe(true);
    expect(document.getElementById("tab-summary")!.classList.contains("active")).toBe(false);
  });

  // showEphemeralPane() (the guard added to handleRun/handleRunSingleNode/
  // showNodeErrorDetail so they don't write into a hidden #output-content
  // while Performance is active) is a 2-line subset of the same toggle
  // covered above, and not independently unit-tested here: reaching any of
  // those three methods requires a real Canvas (node/connector maps, the
  // dangerous-node confirmation gate) that a minimal mock can't satisfy
  // without fabricating internals this fix didn't touch. Covered instead
  // by the full tsc + vitest + vite-build pass (zero regressions) plus the
  // manual trace in this fix's patch notes.
});
