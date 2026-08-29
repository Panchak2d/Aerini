/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const LS_KEY = "aerini_run_history_v1";

type LegacyRecord = {
  id: string; workflowName: string; ranAt: string;
  success: boolean; durationMs: number; result: { success: boolean; logs: unknown[] };
};

function seedLegacyBlob(records: LegacyRecord[]): void {
  localStorage.setItem(LS_KEY, JSON.stringify(records));
}

const fakeResult = { success: true, logs: [] } as unknown as import("../ipc/workflow").WorkflowResult;

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
  localStorage.clear();
});

describe("run-history legacy migration", () => {
  it("migrates legacy records into the fixed pseudo-workflow bucket, not the calling workflow's id", async () => {
    vi.resetModules();
    const mod = await import("../run-history");
    seedLegacyBlob([
      { id: "old-1", workflowName: "Old Workflow A", ranAt: "2026-01-01T00:00:00Z", success: true, durationMs: 100, result: { success: true, logs: [] } },
      { id: "old-2", workflowName: "Old Workflow B", ranAt: "2026-01-02T00:00:00Z", success: false, durationMs: 200, result: { success: false, logs: [] } },
    ]);

    await mod.saveRunToHistory("new-run-1", "wf_real123", "My Real Workflow", fakeResult);

    const saveCalls = invokeMock.mock.calls.filter(c => c[0] === "save_run_record");
    // 2 migrated legacy records + 1 real, current run.
    expect(saveCalls.length).toBe(3);

    const migrated = saveCalls.filter(c => (c[1] as { record: { id: string } }).record.id.startsWith("old-"));
    expect(migrated.length).toBe(2);
    for (const call of migrated) {
      const record = (call[1] as { record: { workflow_id: string; workflow_name: string } }).record;
      // Bucket is fixed, NOT the calling workflow's id.
      expect(record.workflow_id).toBe(mod.LEGACY_HISTORY_WORKFLOW_ID);
      expect(record.workflow_id).not.toBe("wf_real123");
    }
    // Original per-record names are preserved (not overwritten by the caller's name).
    const names = migrated.map(c => (c[1] as { record: { workflow_name: string } }).record.workflow_name).sort();
    expect(names).toEqual(["Old Workflow A", "Old Workflow B"]);

    // The actual current run is unaffected — still saved under the real workflow id.
    const realCall = saveCalls.find(c => (c[1] as { record: { id: string } }).record.id === "new-run-1");
    expect((realCall![1] as { record: { workflow_id: string } }).record.workflow_id).toBe("wf_real123");
  });

  it("bucket id does not depend on which workflow calls first", async () => {
    vi.resetModules();
    const modA = await import("../run-history");
    seedLegacyBlob([{ id: "old-x", workflowName: "Old X", ranAt: "2026-01-01T00:00:00Z", success: true, durationMs: 1, result: { success: true, logs: [] } }]);
    await modA.saveRunToHistory("run-a", "wf_AAA", "Workflow A", fakeResult);
    const bucketFromA = (invokeMock.mock.calls.find(c => c[0] === "save_run_record" && (c[1] as { record: { id: string } }).record.id === "old-x")![1] as { record: { workflow_id: string } }).record.workflow_id;

    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
    localStorage.clear();
    vi.resetModules();
    const modB = await import("../run-history");
    seedLegacyBlob([{ id: "old-y", workflowName: "Old Y", ranAt: "2026-01-01T00:00:00Z", success: true, durationMs: 1, result: { success: true, logs: [] } }]);
    await modB.saveRunToHistory("run-b", "wf_BBB", "Workflow B", fakeResult);
    const bucketFromB = (invokeMock.mock.calls.find(c => c[0] === "save_run_record" && (c[1] as { record: { id: string } }).record.id === "old-y")![1] as { record: { workflow_id: string } }).record.workflow_id;

    expect(bucketFromA).toBe(bucketFromB);
    expect(bucketFromA).toBe(modA.LEGACY_HISTORY_WORKFLOW_ID);
  });

  it("is a no-op when no legacy blob exists — only the current run is saved", async () => {
    vi.resetModules();
    const mod = await import("../run-history");
    await mod.saveRunToHistory("run-only", "wf_solo", "Solo Workflow", fakeResult);

    const saveCalls = invokeMock.mock.calls.filter(c => c[0] === "save_run_record");
    expect(saveCalls.length).toBe(1);
    expect((saveCalls[0][1] as { record: { workflow_id: string } }).record.workflow_id).toBe("wf_solo");
  });

  it("clears the legacy localStorage key after migration, including on malformed JSON", async () => {
    vi.resetModules();
    const mod = await import("../run-history");
    localStorage.setItem(LS_KEY, "{not valid json");

    await mod.saveRunToHistory("run-1", "wf_x", "X", fakeResult);

    expect(localStorage.getItem(LS_KEY)).toBeNull();
    // Malformed blob must not block the real run from saving.
    const saveCalls = invokeMock.mock.calls.filter(c => c[0] === "save_run_record");
    expect(saveCalls.length).toBe(1);
  });

  it("runs migration at most once per module lifetime", async () => {
    vi.resetModules();
    const mod = await import("../run-history");
    seedLegacyBlob([{ id: "old-once", workflowName: "Old Once", ranAt: "2026-01-01T00:00:00Z", success: true, durationMs: 1, result: { success: true, logs: [] } }]);

    await mod.saveRunToHistory("run-1", "wf_1", "One", fakeResult);
    const afterFirst = invokeMock.mock.calls.filter(c => c[0] === "save_run_record").length;
    expect(afterFirst).toBe(2); // 1 migrated + 1 real

    // Re-seed the key (simulating some other tab writing it again) and save another run.
    seedLegacyBlob([{ id: "old-again", workflowName: "Old Again", ranAt: "2026-01-01T00:00:00Z", success: true, durationMs: 1, result: { success: true, logs: [] } }]);
    await mod.saveRunToHistory("run-2", "wf_1", "One", fakeResult);
    const afterSecond = invokeMock.mock.calls.filter(c => c[0] === "save_run_record").length;
    expect(afterSecond).toBe(3); // no second migration attempt, just the 2nd real run
  });

  it("LEGACY_HISTORY_WORKFLOW_ID can never collide with a real workflow id (wf_-prefixed)", async () => {
    vi.resetModules();
    const mod = await import("../run-history");
    expect(mod.LEGACY_HISTORY_WORKFLOW_ID.startsWith("wf_")).toBe(false);
  });
});
