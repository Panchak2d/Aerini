/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("../ipc/workflow", () => ({
  listWorkflows: vi.fn(),
  startScheduledWorkflow: vi.fn(),
  stopScheduledWorkflow: vi.fn(),
  parseSchedulerError: (raw: string) => ({ error_kind: "other", message: raw }),
}));
vi.mock("../run-manager", () => ({ getBgJobs: vi.fn(() => []) }));
vi.mock("../workflow-manager", () => ({ isWorkflowRunning: vi.fn(() => false) }));
vi.mock("../ipc/memory", () => ({ getMemoryBreakdown: vi.fn(), getProcessMemory: vi.fn() }));
vi.mock("../ipc/performance", () => ({ getRecentPerformance: vi.fn() }));
vi.mock("../utils", () => ({ isTauri: () => true, escapeHtml: (s: string) => s }));
vi.mock("../confirm", () => ({ showConfirm: vi.fn() }));

import { mountMonitorPanel, unmountMonitorPanel } from "../panels/MonitorPanel";
import { getMemoryBreakdown, getProcessMemory } from "../ipc/memory";
import { listWorkflows } from "../ipc/workflow";

describe("MonitorPanel polling", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = `<div id="monitor-area"></div><div id="zone-monitor"></div>`;
    vi.mocked(listWorkflows).mockResolvedValue([]);
    vi.mocked(getProcessMemory).mockResolvedValue({ current_bytes: 1, peak_bytes: 1 } as never);
  });
  afterEach(() => {
    unmountMonitorPanel();
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("a slow tick is not overlapped by the next interval", async () => {
    let release: (v: never[]) => void = () => {};
    vi.mocked(getMemoryBreakdown).mockImplementation(() => new Promise(r => { release = r as never; }));
    mountMonitorPanel();
    await vi.advanceTimersByTimeAsync(20_000);
    expect(vi.mocked(getMemoryBreakdown)).toHaveBeenCalledTimes(1);
    release([]);
    await vi.advanceTimersByTimeAsync(20_000);
    expect(vi.mocked(getMemoryBreakdown).mock.calls.length).toBeGreaterThan(1);
  });

  it("a tick still pending after unmount and remount cannot block the new mount", async () => {
    let release: (v: never[]) => void = () => {};
    vi.mocked(getMemoryBreakdown).mockImplementationOnce(() => new Promise(r => { release = r as never; }));
    vi.mocked(getMemoryBreakdown).mockResolvedValue([]);
    mountMonitorPanel();
    unmountMonitorPanel();
    mountMonitorPanel();
    await vi.advanceTimersByTimeAsync(100);
    expect(vi.mocked(getMemoryBreakdown).mock.calls.length).toBeGreaterThanOrEqual(2);
    release([]);
  });
});
