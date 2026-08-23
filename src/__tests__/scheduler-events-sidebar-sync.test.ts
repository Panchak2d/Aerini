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

async function bind() {
  const canvas    = { resetAllStatus: vi.fn() } as unknown as Canvas;
  const wfManager = { currentId: "wf-1", refreshWorkflowList: vi.fn() } as unknown as WorkflowManager;
  const runManager = { showResultFromScheduler: vi.fn() } as unknown as RunManager;
  await bindSchedulerEvents(canvas, wfManager, runManager, vi.fn(), vi.fn(), vi.fn(), vi.fn());
  return { wfManager };
}

beforeEach(() => { schedulerStatusCb = null; });

describe("bindSchedulerEvents — sidebar list stays in sync with running state", () => {
  it("normal case: a running status event refreshes the sidebar list, not just the dot", async () => {
    const { wfManager } = await bind();
    await schedulerStatusCb!(makeEvent({ status: "running" }));
    expect(wfManager.refreshWorkflowList).toHaveBeenCalled();
  });

  it("edge case: a stopped status event also refreshes the list, so the workflow reappears", async () => {
    const { wfManager } = await bind();
    await schedulerStatusCb!(makeEvent({ status: "running" }));
    (wfManager.refreshWorkflowList as ReturnType<typeof vi.fn>).mockClear();

    await schedulerStatusCb!(makeEvent({ status: "stopped" }));
    expect(wfManager.refreshWorkflowList).toHaveBeenCalled();
  });
});
