// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowManager } from "../workflow-manager";
import type { RunBreakdown } from "../ipc/memory";

vi.mock("../ipc/memory", () => ({
  getMemoryBreakdown: vi.fn(),
  listenMemoryBreakdown: vi.fn().mockResolvedValue(() => {}),
}));

// (Performance Monitor Redesign): chip's post-run fallback.
vi.mock("../ipc/performance", () => ({
  getRecentPerformance: vi.fn(),
}));

import { initStatusBarFields, initMemChip } from "../statusbar-fields";
import { getMemoryBreakdown } from "../ipc/memory";
import { getRecentPerformance } from "../ipc/performance";
import type { PerformanceReport } from "../ipc/performance";

function makeFakeCanvas(overrides: Partial<{ nodes: number; connectors: number; selected: number; zoom: number }> = {}): Canvas {
  return {
    nodes:         new Map(Array.from({ length: overrides.nodes ?? 0 },      (_, i) => [String(i), {}])),
    connectors:    new Map(Array.from({ length: overrides.connectors ?? 0 }, (_, i) => [String(i), {}])),
    selectedNodes: new Set(Array.from({ length: overrides.selected ?? 0 },   (_, i) => String(i))),
    zoom: overrides.zoom ?? 1,
  } as unknown as Canvas;
}

function makeFakeWfManager(id = "wf-1", name = "My Workflow"): WorkflowManager {
  return { currentId: id, currentName: name } as unknown as WorkflowManager;
}

function baseDom(): void {
  document.body.innerHTML = `
    <div id="status-bar">
      <span id="status-text">Ready</span>
      <span class="status-hint" id="status-hint"></span>
    </div>
    <div id="canvas-stat-bar"></div>
    <div id="run-dropdown-wrap"></div>
    <div id="output-drawer" class="hidden"></div>
  `;
}

describe("initStatusBarFields", () => {
  beforeEach(() => { vi.useFakeTimers(); baseDom(); });
  afterEach(() => { vi.useRealTimers(); });

  it("mounts into #canvas-stat-bar, not #status-bar, and #status-bar keeps only its original two children", () => {
    initStatusBarFields(makeFakeCanvas(), makeFakeWfManager());
    const statusBar = document.getElementById("status-bar")!;
    expect(statusBar.querySelector(".statusbar-fields")).toBeNull();
    expect(statusBar.querySelector(".mem-chip")).toBeNull();
    expect(statusBar.children.length).toBe(2); // #status-text, .status-hint — untouched
    expect(document.querySelector("#canvas-stat-bar .statusbar-field")).not.toBeNull();
  });

  it("normal case: nodes/conn/selected/zoom update on the 150ms poll and reflect canvas state", () => {
    const canvas = makeFakeCanvas({ nodes: 3, connectors: 2, selected: 1, zoom: 1.5 });
    initStatusBarFields(canvas, makeFakeWfManager());
    vi.advanceTimersByTime(150);
    expect(document.getElementById("stat-nodes")!.textContent).toBe("3");
    expect(document.getElementById("stat-conn")!.textContent).toBe("2");
    expect(document.getElementById("stat-selected")!.textContent).toBe("1");
    expect(document.getElementById("stat-zoom")!.textContent).toBe("150%");
  });

  it("edge case: run-state mirrors #run-dropdown-wrap's data-run-state, mapping \"success\" -> \"ok\", and falls back to \"idle\"/\"Ready\" when the attribute is absent", () => {
    initStatusBarFields(makeFakeCanvas(), makeFakeWfManager());
    const wrap  = document.getElementById("run-dropdown-wrap")!;
    const state = document.getElementById("statusbar-state")!;
    const label = document.getElementById("stat-run-state")!;

    vi.advanceTimersByTime(150); // absent attribute -> idle/Ready
    expect(state.dataset.state).toBe("idle");
    expect(label.textContent).toBe("Ready");

    wrap.setAttribute("data-run-state", "success");
    vi.advanceTimersByTime(150);
    expect(state.dataset.state).toBe("ok"); // "success" (DOM attr) -> "ok" (token name)
    expect(label.textContent).toBe("Success");

    wrap.setAttribute("data-run-state", "error");
    vi.advanceTimersByTime(150);
    expect(state.dataset.state).toBe("error");
    expect(label.textContent).toBe("Error");
  });

  it("mem-chip and its popover still render inside the new container", () => {
    initStatusBarFields(makeFakeCanvas(), makeFakeWfManager());
    expect(document.querySelector("#canvas-stat-bar #mem-chip")).not.toBeNull();
    expect(document.querySelector("#canvas-stat-bar #mem-popover")).not.toBeNull();
  });
});

describe("initMemChip", () => {
  const memChipDom = () => {
    document.body.innerHTML = `
      <button id="mem-chip" data-level="idle" aria-expanded="false">
        <span class="mem-dot"></span>MEM <b id="mem-chip-value">—</b>
      </button>
      <div class="mem-popover" id="mem-popover" hidden></div>
      <span class="zone-mem-badge hidden" id="bg-mem-badge" title=""></span>
      <div id="output-drawer" class="hidden"></div>
    `;
  };

  beforeEach(() => {
    memChipDom();
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};
  });
  afterEach(() => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    vi.mocked(getMemoryBreakdown).mockReset();
    vi.mocked(getRecentPerformance).mockReset();
  });

  const flush = async () => { await Promise.resolve(); await Promise.resolve(); };
  // One extra pair of ticks: the fallback path chains a second async call
  // (getRecentPerformance) inside render()'s own .then(), one microtask
  // boundary deeper than the other tests in this file need.
  const flushFallback = async () => { await flush(); await flush(); };

  function makeReport(overrides: Partial<PerformanceReport> = {}): PerformanceReport {
    return {
      workflow_id: "wf-1", status: "success",
      started_at_ms: 0, finished_at_ms: 1000, duration_ms: 1000,
      sampling_interval_ms: 150, baseline_bytes: 0, final_bytes: 5 * 1024 * 1024,
      peak_bytes: 6 * 1024 * 1024, peak_at_ms: 500, minimum_bytes: 0,
      average_bytes: 3 * 1024 * 1024, delta_bytes: 5 * 1024 * 1024,
      sample_count: 7, history: [],
      ...overrides,
    };
  }

  it("CSP-safe: no element in the rendered popover has a templated style attribute except the bar, whose width is set via CSSOM to the expected percentage", async () => {
    const data: RunBreakdown[] = [{
      workflow_id: "wf-1", live_bytes: 1000,
      nodes: [{ node_type_id: "http_request", live_bytes: 3000 }],
    }] as unknown as RunBreakdown[];
    vi.mocked(getMemoryBreakdown).mockResolvedValue(data);

    initMemChip(makeFakeWfManager("wf-1", "My Workflow"));
    await flush();

    const popover = document.getElementById("mem-popover")!;
    const styled = popover.querySelectorAll("[style]");
    // Exactly one styled element per row (the bar <i>) — 2 rows here
    // (Run overhead + 1 node) — and nothing else in the popover.
    expect(styled.length).toBe(2);
    styled.forEach(el => expect(el.tagName).toBe("I"));

    const bars = popover.querySelectorAll<HTMLElement>(".mem-row-bar i");
    // maxRowBytes = 3000 (the larger row) -> overhead row = 1000/3000 = 33.33%
    expect(bars[0].style.width).toBe(`${(1000 / 3000 * 100)}%`);
    expect(bars[1].style.width).toBe("100%");
  });

  it("escapes workflow/job names: a malicious name renders as literal text, never parsed as markup", async () => {
    const evil = `<img src=x onerror=alert(1)>`;
    const data: RunBreakdown[] = [{
      workflow_id: "wf-1", live_bytes: 500, nodes: [],
    }] as unknown as RunBreakdown[];
    vi.mocked(getMemoryBreakdown).mockResolvedValue(data);

    initMemChip(makeFakeWfManager("wf-1", evil));
    await flush();

    const popover = document.getElementById("mem-popover")!;
    expect(popover.querySelector("img")).toBeNull();
    expect(popover.querySelector(".mem-run-name")!.textContent).toBe(evil);
  });

  it("normal case: headline total and popover are scoped to the current workflow only, not summed across every run in the breakdown", async () => {
    const data: RunBreakdown[] = [
      { workflow_id: "wf-1", live_bytes: 2 * 1024 * 1024, nodes: [] }, // current: 2.0 MB
      { workflow_id: "wf-2", live_bytes: 9 * 1024 * 1024, nodes: [] }, // background: 9.0 MB
    ] as unknown as RunBreakdown[];
    vi.mocked(getMemoryBreakdown).mockResolvedValue(data);

    initMemChip(makeFakeWfManager("wf-1", "My Workflow"));
    await flush();

    // Headline reflects wf-1 alone (2.0 MB), not the 11.0 MB combined total.
    expect(document.getElementById("mem-chip-value")!.textContent).toBe("2.0 MB");
    // Popover shows only wf-1's run.
    const popover = document.getElementById("mem-popover")!;
    expect(popover.querySelectorAll(".mem-run").length).toBe(1);
  });

  it("bg-mem-badge shows the aggregate across ALL live runs, including the one currently open, and stops hiding on that account", async () => {
    const wf1 = { workflow_id: "wf-1", live_bytes: 2 * 1024 * 1024, nodes: [] } as unknown as RunBreakdown;
    const wf2 = { workflow_id: "wf-2", live_bytes: 3 * 1024 * 1024, nodes: [] } as unknown as RunBreakdown;
    const wf3 = { workflow_id: "wf-3", live_bytes: 4 * 1024 * 1024, nodes: [] } as unknown as RunBreakdown;

    vi.mocked(getMemoryBreakdown).mockResolvedValue([wf1, wf2, wf3]);
    const { refresh } = initMemChip(makeFakeWfManager("wf-1", "My Workflow"));
    await flush();

    const badge = document.getElementById("bg-mem-badge")!;
    expect(badge.classList.contains("hidden")).toBe(false);
    // 2.0 + 3.0 + 4.0 = 9.0 MB -- ALL live runs, not "all except wf-1".
    expect(badge.textContent).toBe("9.0 MB");

    // wf-1 (the open workflow) is now the ONLY run left. It must not be
    // excluded from the total just because it's the open workflow --
    // wf-1 still has live memory, so the badge must keep showing it.
    vi.mocked(getMemoryBreakdown).mockResolvedValue([wf1]);
    refresh();
    await flush();
    expect(badge.classList.contains("hidden")).toBe(false);
    expect(badge.textContent).toBe("2.0 MB");
  });

  it("bg-mem-badge hides only when genuinely nothing is running anywhere", async () => {
    const wf1 = { workflow_id: "wf-1", live_bytes: 2 * 1024 * 1024, nodes: [] } as unknown as RunBreakdown;
    vi.mocked(getMemoryBreakdown).mockResolvedValue([wf1]);
    const { refresh } = initMemChip(makeFakeWfManager("wf-1", "My Workflow"));
    await flush();

    const badge = document.getElementById("bg-mem-badge")!;
    expect(badge.classList.contains("hidden")).toBe(false);

    vi.mocked(getMemoryBreakdown).mockResolvedValue([]);
    refresh();
    await flush();
    expect(badge.classList.contains("hidden")).toBe(true);
    expect(badge.textContent).toBe("");
  });

  // (Performance Monitor Redesign): the "goes blank"
  // fix. mem_tracking clears the instant a run ends; perf_monitor::RECENT
  // does not.
  it("normal case: falls back to the last completed run's total instead of showing \"\u2014\" when mem_tracking has cleared", async () => {
    vi.mocked(getMemoryBreakdown).mockResolvedValue([]);
    vi.mocked(getRecentPerformance).mockResolvedValue(makeReport({ final_bytes: 5 * 1024 * 1024 }));

    initMemChip(makeFakeWfManager("wf-1", "My Workflow"));
    await flushFallback();

    expect(getRecentPerformance).toHaveBeenCalledWith("wf-1");
    expect(document.getElementById("mem-chip-value")!.textContent).toBe("5.0 MB");
    expect(document.getElementById("mem-popover")!.textContent).toContain("Last run: 5.0 MB");
  });

  it("edge case: a workflow that has never run this session stays on the generic empty state, not a fabricated number", async () => {
    vi.mocked(getMemoryBreakdown).mockResolvedValue([]);
    vi.mocked(getRecentPerformance).mockResolvedValue(null);

    initMemChip(makeFakeWfManager("wf-1", "My Workflow"));
    await flushFallback();

    expect(document.getElementById("mem-chip-value")!.textContent).toBe("\u2014");
    expect(document.getElementById("mem-popover")!.textContent).toContain("isn't currently running");
  });

  it("edge case: a stale fallback for a workflow the user has since switched away from is discarded, not shown", async () => {
    let resolveFetch: (v: PerformanceReport | null) => void;
    vi.mocked(getMemoryBreakdown).mockResolvedValue([]);
    vi.mocked(getRecentPerformance).mockReturnValue(new Promise(res => { resolveFetch = res; }));

    const wfManager = makeFakeWfManager("wf-1", "My Workflow");
    initMemChip(wfManager);
    await flush(); // getMemoryBreakdown resolves, getRecentPerformance kicked off but not yet resolved

    (wfManager as { currentId: string }).currentId = "wf-2"; // user switches workflow before the fetch lands
    resolveFetch!(makeReport({ final_bytes: 5 * 1024 * 1024 }));
    await flushFallback();

    // Must not show wf-1's stale fallback now that wf-2 is open.
    expect(document.getElementById("mem-chip-value")!.textContent).toBe("\u2014");
  });
});
