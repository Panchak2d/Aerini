import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { invoke } from "@tauri-apps/api/core";


export type PerfStatus = "running" | "success" | "failed";

export interface HistorySample {
  at_ms: number;
  bytes: number;
}

export interface PerformanceReport {
  workflow_id: string;
  status: PerfStatus;
  started_at_ms: number;
  /** Real completion time when status !== "running"; when status ===
   *  "running" (only possible from getLivePerformance/listenPerformanceLive,
   *  never from getRecentPerformance), this is "as of" the reading instead. */
  finished_at_ms: number;
  duration_ms: number;
  sampling_interval_ms: number;
  baseline_bytes: number;
  final_bytes: number;
  peak_bytes: number;
  peak_at_ms: number;
  minimum_bytes: number;
  average_bytes: number;
  /** Signed — a run can legitimately end below its own baseline. */
  delta_bytes: number;
  sample_count: number;
  history: HistorySample[];
}

/**
 * Point-in-time read of one workflow currently in progress. `null` if it
 * isn't running right now — same "empty is a real state, not a stale
 * read" convention as getMemoryBreakdown (ipc/memory.ts).
 */
export async function getLivePerformance(workflowId: string): Promise<PerformanceReport | null> {
  return invoke<PerformanceReport | null>("get_live_performance", { workflowId });
}

/**
 * The most recently completed run's frozen report for a workflow. `null`
 * if it has never run this session, or its recent slot was cleared.
 * Unlike getLivePerformance, this does NOT vanish the instant a run ends
 * — it's the MEM chip/popover's post-run fallback (statusbar-fields.ts).
 */
export async function getRecentPerformance(workflowId: string): Promise<PerformanceReport | null> {
  return invoke<PerformanceReport | null>("get_recent_performance", { workflowId });
}

let _unlistenPerformanceLive: (() => void) | null = null;
let _performanceLiveRegistered = false;

/**
 * Live updates while >=1 workflow run is in flight (~300ms interval,
 * src-tauri/src/lib.rs) — every currently-running workflow at once, same
 * "all-live, frontend filters/aggregates per workflow" shape
 * listenMemoryBreakdown already established. Never fires while idle.
 */
export async function listenPerformanceLive(
  cb: (reports: PerformanceReport[]) => void
): Promise<() => void> {
  // Same double-registration guard as ipc/memory.ts's listenMemoryBreakdown.
  if (_performanceLiveRegistered && _unlistenPerformanceLive) {
    return _unlistenPerformanceLive;
  }
  if (_unlistenPerformanceLive) {
    _unlistenPerformanceLive();
    _unlistenPerformanceLive = null;
  }
  _performanceLiveRegistered = true;
  const win = getCurrentWebviewWindow();
  const unlisten = await win.listen<PerformanceReport[]>("performance-live", (e) => {
    cb(e.payload);
  });
  _unlistenPerformanceLive = unlisten;
  return unlisten;
}
