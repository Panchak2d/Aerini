import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { invoke } from "@tauri-apps/api/core";


export interface NodeBreakdown {
  node_id: string;
  node_type_id: string;
  started_at_ms: number;
  live_bytes: number;
}

export interface RunBreakdown {
  workflow_id: string;
  started_at_ms: number;
  /** Bytes attributed to the run's own overhead only — does NOT include
   *  `nodes[*].live_bytes`. Sum both for a run's true total. */
  live_bytes: number;
  nodes: NodeBreakdown[];
}

/**
 * One-off snapshot. `[]` whenever no workflow is currently running — the
 * backend disables allocation tracking entirely at idle (see
 * mem_tracking.rs's "Overhead when idle" doc), so `[]` is a real "nothing
 * to report" state, not a stale/missing read.
 *
 * Used for: the indicator's first paint (before the periodic event's first
 * ~2s tick lands), and an on-demand refresh on run start/end — the
 * periodic event below only fires while >=1 run is in flight, so it never
 * tells the frontend "a run just ended, this is now empty" on its own.
 */
export async function getMemoryBreakdown(): Promise<RunBreakdown[]> {
  return invoke<RunBreakdown[]>("get_memory_breakdown");
}

let _unlistenMemoryBreakdown: (() => void) | null = null;
let _memoryBreakdownRegistered = false;

/**
 * Live updates while >=1 workflow run is in flight (~2s interval,
 * src-tauri/src/lib.rs). Never fires while idle — see getMemoryBreakdown
 * above for how callers detect "now empty" instead.
 */
export async function listenMemoryBreakdown(
  cb: (breakdown: RunBreakdown[]) => void
): Promise<() => void> {
  // Same double-registration guard as the other ipc/events.ts listeners
  // (e.g. Vite HMR in dev re-running init()).
  if (_memoryBreakdownRegistered && _unlistenMemoryBreakdown) {
    return _unlistenMemoryBreakdown;
  }
  if (_unlistenMemoryBreakdown) {
    _unlistenMemoryBreakdown();
    _unlistenMemoryBreakdown = null;
  }
  _memoryBreakdownRegistered = true;
  const win = getCurrentWebviewWindow();
  const unlisten = await win.listen<RunBreakdown[]>("memory-breakdown", (e) => {
    cb(e.payload);
  });
  _unlistenMemoryBreakdown = unlisten;
  return unlisten;
}
