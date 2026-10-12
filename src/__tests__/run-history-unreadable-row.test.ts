/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));
vi.mock("../confirm", () => ({ showConfirm: vi.fn() }));

function rec(result_json: string) {
  return {
    id: "r1", workflow_id: "wf", workflow_name: "W", ran_at: new Date().toISOString(),
    success: true, duration_ms: 5, status: "completed", result_json,
  };
}

async function renderWith(records: unknown[], onRestore: (r: unknown) => void) {
  invokeMock.mockImplementation((cmd: string) => Promise.resolve(cmd === "list_run_records" ? records : []));
  const { renderHistoryPanel } = await import("../run-history");
  const panel = renderHistoryPanel("wf", onRestore as never, vi.fn());
  await new Promise(r => setTimeout(r, 0));
  await new Promise(r => setTimeout(r, 0));
  return panel;
}

describe("run history restore", () => {
  beforeEach(() => { invokeMock.mockReset(); vi.spyOn(console, "warn").mockImplementation(() => {}); });

  it("normal case: clicking a readable run restores its parsed result", async () => {
    const onRestore = vi.fn();
    const panel = await renderWith([rec(JSON.stringify({ success: true, node_outputs: {}, logs: [] }))], onRestore);
    panel.querySelector<HTMLElement>(".history-item")!.click();
    expect(onRestore).toHaveBeenCalledWith({ success: true, node_outputs: {}, logs: [] });
  });

  it("edge case: a corrupt result_json marks the row unreadable instead of failing silently", async () => {
    const onRestore = vi.fn();
    const panel = await renderWith([rec("{not json")], onRestore);
    const row = panel.querySelector<HTMLElement>(".history-item")!;
    row.click();
    expect(onRestore).not.toHaveBeenCalled();
    expect(row.classList.contains("history-unreadable")).toBe(true);
    expect(row.getAttribute("aria-disabled")).toBe("true");
  });
});
