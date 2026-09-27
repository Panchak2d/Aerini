/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import { RunManager } from "../run-manager";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

function setDom(): void {
  document.body.innerHTML = `
    <div id="output-drawer" class="hidden">
      <div class="drawer-header" id="drawer-header">
        <span>Output</span>
        <div class="drawer-tabs" id="drawer-tabs"></div>
        <button id="drawer-tab-performance" class="drawer-tab drawer-tab-static tab-neutral" data-tab="performance">Performance</button>
      </div>
      <div id="output-content"></div>
      <div id="output-content-performance" class="hidden"></div>
    </div>
  `;
}

function makeManager(): RunManager {
  return new RunManager({} as unknown as Canvas, vi.fn(), vi.fn());
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe("openHistoryDrawer — History tab click wiring", () => {
  it("normal case: clicking History after Performance re-shows the history pane", () => {
    setDom();
    const rm = makeManager();
    rm.setCurrentWorkflow("wf-1", "First");
    rm.openHistoryDrawer();

    const historyTab = document.querySelector<HTMLElement>("#drawer-tabs .drawer-tab")!;
    const perfTab     = document.getElementById("drawer-tab-performance")!;
    expect(historyTab.textContent).toBe("History");
    expect(historyTab.classList.contains("active")).toBe(true);

    // Navigate away to the Performance tab.
    perfTab.click();
    expect(document.getElementById("output-content")!.classList.contains("hidden")).toBe(true);
    expect(document.getElementById("output-content-performance")!.classList.contains("hidden")).toBe(false);

    // Click back to History — must actually switch the active tab.
    historyTab.click();
    expect(historyTab.classList.contains("active")).toBe(true);
    expect(perfTab.classList.contains("active")).toBe(false);
    expect(document.getElementById("output-content")!.classList.contains("hidden")).toBe(false);
    expect(document.getElementById("output-content-performance")!.classList.contains("hidden")).toBe(true);
  });

  it("edge case: the history panel content itself survives the Performance/History round trip untouched", () => {
    setDom();
    const rm = makeManager();
    rm.setCurrentWorkflow("wf-1", "First");
    rm.openHistoryDrawer();

    const content = document.getElementById("output-content")!;
    expect(content.children.length).toBeGreaterThan(0); // renderHistoryPanel() appended something
    const panelNode = content.firstElementChild;

    document.getElementById("drawer-tab-performance")!.click();
    document.querySelector<HTMLElement>("#drawer-tabs .drawer-tab")!.click();

    // Same node, not re-created — clicking History re-shows, it doesn't rebuild.
    expect(content.firstElementChild).toBe(panelNode);
  });
});
