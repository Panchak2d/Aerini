// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowManager } from "../workflow-manager";
import type { RunManager } from "../run-manager";
import type { SchedulerStatusEvent } from "../ipc/events";

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
vi.mock("../utils", () => ({ isTauri: vi.fn().mockReturnValue(false) }));
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

beforeEach(() => { schedulerStatusCb = null; });

describe("bindSchedulerEvents — Performance panel refresh (Issue 2)", () => {
  it("normal case: a scheduler status event refreshes both the MEM chip and the Performance panel", async () => {
    const canvas     = { resetAllStatus: vi.fn() } as unknown as Canvas;
    const wfManager   = { currentId: "wf-1", refreshWorkflowList: vi.fn() } as unknown as WorkflowManager;
    const runManager  = { showResultFromScheduler: vi.fn() } as unknown as RunManager;
    const refreshMem  = vi.fn();
    const refreshPerf = vi.fn();

    await bindSchedulerEvents(canvas, wfManager, runManager, vi.fn(), vi.fn(), vi.fn(), refreshMem, refreshPerf);
    expect(schedulerStatusCb).not.toBeNull();

    await schedulerStatusCb!(makeEvent({ status: "waiting" }));

    expect(refreshMem).toHaveBeenCalledTimes(1);
    expect(refreshPerf).toHaveBeenCalledTimes(1);
  });

  it("edge case: refreshPerf is optional — an omitted callback must not throw", async () => {
    const canvas    = { resetAllStatus: vi.fn() } as unknown as Canvas;
    const wfManager  = { currentId: "wf-1", refreshWorkflowList: vi.fn() } as unknown as WorkflowManager;
    const runManager = { showResultFromScheduler: vi.fn() } as unknown as RunManager;

    await bindSchedulerEvents(canvas, wfManager, runManager, vi.fn(), vi.fn(), vi.fn(), vi.fn());
    await schedulerStatusCb!(makeEvent()); // must not throw/reject with refreshPerf omitted
  });
});
