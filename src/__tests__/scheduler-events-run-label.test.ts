// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowManager } from "../workflow-manager";
import type { RunManager } from "../run-manager";
import type { SchedulerStatusEvent } from "../ipc/events";
import { NODE_IDS } from "../node-ids";

let schedulerStatusCb: ((evt: SchedulerStatusEvent) => void | Promise<void>) | null = null;

vi.mock("../ipc/events", () => ({
  listenNodeStatus: vi.fn().mockResolvedValue(() => {}),
  listenSchedulerStatus: vi.fn((cb: (evt: SchedulerStatusEvent) => void) => {
    schedulerStatusCb = cb;
    return Promise.resolve(() => {});
  }),
  listenSchedulerSkip: vi.fn().mockResolvedValue(() => {}),
  requestSchedulerState: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("../ipc/workflow", () => ({ getScheduledJobs: vi.fn().mockResolvedValue([]) }));
vi.mock("../workflow-manager", () => ({ setWorkflowRunning: vi.fn() }));
vi.mock("../run-manager", () => ({
  getBgJobs: vi.fn().mockReturnValue([]),
  updateBgJobStoreFromEvent: vi.fn(),
  hydrateBgJobsFromScheduler: vi.fn(),
}));
vi.mock("../run-history", () => ({ saveRunToHistory: vi.fn() }));
vi.mock("../utils", () => ({ isTauri: vi.fn().mockReturnValue(false), escapeHtml: (s: string) => s }));
vi.mock("../sidebar-sections", () => ({ getCurrentZone: vi.fn().mockReturnValue("canvas") }));
vi.mock("../bg-panel-loader", () => ({
  loadBgPanel: vi.fn().mockResolvedValue({
    renderBgJobsDebounced: vi.fn(),
    updateBgRunButton: vi.fn(),
  }),
}));

import { bindSchedulerEvents } from "../scheduler-events";

function makeEvent(overrides: Partial<SchedulerStatusEvent> = {}): SchedulerStatusEvent {
  return {
    workflow_id: "wf-1", workflow_name: "wf", status: "running",
    run_count: 1, last_run_at: null, next_run_at: null, last_error: null,
    last_result: null, ...overrides,
  };
}

function setDom(): void {
  document.body.innerHTML = `
    <div id="output-drawer">
      <div id="drawer-tabs"></div>
      <div id="output-content"></div>
    </div>
  `;
}

function canvasWithTrigger(nodeTypeId: string): Canvas {
  return {
    resetAllStatus: vi.fn(),
    nodes: new Map([["n1", { data: { node_type_id: nodeTypeId } }]]),
  } as unknown as Canvas;
}

async function bind(canvas: Canvas, runManager: Partial<RunManager> = {}) {
  const wfManager = { currentId: "wf-1", refreshWorkflowList: vi.fn() } as unknown as WorkflowManager;
  await bindSchedulerEvents(
    canvas, wfManager, runManager as unknown as RunManager,
    vi.fn(), vi.fn(), vi.fn(), vi.fn(),
  );
  return { wfManager };
}

beforeEach(() => { schedulerStatusCb = null; });

describe("bindSchedulerEvents — run-in-progress label", () => {
  it("normal case: a Webhook-triggered run is not labeled \"Scheduled\"", async () => {
    setDom();
    await bind(canvasWithTrigger(NODE_IDS.WEBHOOK));
    await schedulerStatusCb!(makeEvent({ status: "running" }));
    const label = document.querySelector(".run-spinner-label")!.textContent;
    expect(label).toBe("Workflow running…");
  });

  it("edge case: a genuine Schedule-node trigger keeps the \"Scheduled\" label", async () => {
    setDom();
    await bind(canvasWithTrigger(NODE_IDS.SCHEDULE));
    await schedulerStatusCb!(makeEvent({ status: "running" }));
    const label = document.querySelector(".run-spinner-label")!.textContent;
    expect(label).toBe("Scheduled run in progress…");
  });
});

describe("bindSchedulerEvents — stopped/error no longer freeze the spinner", () => {
  it("normal case: stopping a run replaces the frozen spinner with a Stopped notice", async () => {
    setDom();
    await bind(canvasWithTrigger(NODE_IDS.WEBHOOK));
    await schedulerStatusCb!(makeEvent({ status: "running" }));
    expect(document.querySelector(".run-spinner-wrap")).not.toBeNull();

    await schedulerStatusCb!(makeEvent({ status: "stopped" }));

    expect(document.querySelector(".run-spinner-wrap")).toBeNull();
    expect(document.getElementById("output-content")!.textContent).toContain("Stopped");
  });

  it("edge case: an error with no result yet shows the error message instead of a frozen spinner", async () => {
    setDom();
    await bind(canvasWithTrigger(NODE_IDS.WEBHOOK));
    await schedulerStatusCb!(makeEvent({ status: "running" }));

    await schedulerStatusCb!(makeEvent({ status: "error", last_error: "boom", last_result: null }));

    expect(document.querySelector(".run-spinner-wrap")).toBeNull();
    expect(document.getElementById("output-content")!.textContent).toContain("boom");
  });

  it("does not clobber a real result already rendered by showResultFromScheduler", async () => {
    setDom();
    const showResultFromScheduler = vi.fn(() => {
      document.getElementById("output-content")!.innerHTML = "<div class=\"real-result\">done</div>";
    });
    await bind(canvasWithTrigger(NODE_IDS.WEBHOOK), { showResultFromScheduler });

    await schedulerStatusCb!(makeEvent({
      status: "waiting",
      last_result: { success: true } as unknown as SchedulerStatusEvent["last_result"],
    }));
    expect(document.querySelector(".real-result")).not.toBeNull();

    // A later "stopped" with no fresh spinner present must leave the result alone.
    await schedulerStatusCb!(makeEvent({ status: "stopped" }));
    expect(document.querySelector(".real-result")).not.toBeNull();
  });
});
