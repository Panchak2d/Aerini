/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("../ipc/workflow", () => ({
  startScheduledWorkflow: vi.fn(),
  parseSchedulerError: (raw: string) => {
    try { return JSON.parse(raw); } catch { return { error_kind: "other", message: raw }; }
  },
}));

import { startWithPortFallback } from "../panels/MonitorPanel";
import { startScheduledWorkflow } from "../ipc/workflow";

const mockStart = vi.mocked(startScheduledWorkflow);

beforeEach(() => {
  mockStart.mockReset();
});

describe("MonitorPanel.startWithPortFallback", () => {
  it("normal case: starts on the first try, reports no fallback was needed", async () => {
    mockStart.mockResolvedValueOnce(undefined);

    const busyPort = await startWithPortFallback("wf-1");

    expect(busyPort).toBeNull();
    expect(mockStart).toHaveBeenCalledWith("wf-1", undefined);
  });

  it("edge case: a port_conflict is retried on the next port and reports the original busy port", async () => {
    const conflict = JSON.stringify({
      error_kind: "port_conflict",
      port: 3456,
      held_by_workflow_id: "wf-2",
      held_by_workflow_name: "Other Workflow",
    });
    mockStart.mockRejectedValueOnce(conflict).mockResolvedValueOnce(undefined);

    const busyPort = await startWithPortFallback("wf-1");

    expect(busyPort).toBe(3456);
    expect(mockStart).toHaveBeenNthCalledWith(1, "wf-1", undefined);
    expect(mockStart).toHaveBeenNthCalledWith(2, "wf-1", 3457);
  });
});
