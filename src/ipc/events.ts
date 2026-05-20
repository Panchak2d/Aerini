import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { invoke } from "@tauri-apps/api/core";

export type NodeStatus = "running" | "success" | "error" | "skipped";

export interface NodeStatusEvent {
  workflow_id: string;
  node_id: string;
  status: NodeStatus;
}

let _unlistenNodeStatus: (() => void) | null = null;
let _nodeStatusRegistered = false;

export async function listenNodeStatus(
  cb: (event: NodeStatusEvent) => void
): Promise<() => void> {
  // Fix #10: guard against double-registration (e.g. Vite HMR in dev).
  // If already registered, return the existing unlisten handle without re-registering.
  if (_nodeStatusRegistered && _unlistenNodeStatus) {
    return _unlistenNodeStatus;
  }
  if (_unlistenNodeStatus) {
    _unlistenNodeStatus();
    _unlistenNodeStatus = null;
  }
  _nodeStatusRegistered = true;
  const win = getCurrentWebviewWindow();
  const unlisten = await win.listen<NodeStatusEvent>("node-status", (e) => {
    cb(e.payload);
  });
  _unlistenNodeStatus = unlisten;
  return unlisten;
}

/**
 * Listen for the app close button. When hasUnsaved() returns true,
 * the confirm dialog is shown; if user cancels, the window stays open.
 * Call this once during app init.
 */
export async function listenCloseRequested(
  hasUnsaved: () => boolean,
  showConfirm: (msg: string) => Promise<boolean>
): Promise<void> {
  const win = getCurrentWebviewWindow();
  await win.listen("tauri://close-requested", async () => {
    if (hasUnsaved()) {
      const ok = await showConfirm(
        "You have unsaved changes. Close anyway and lose them?"
      );
      if (!ok) return;
    }
    // Close for real: call the Tauri destroy command
    await invoke("close_window").catch(() => win.destroy());
  });
}

export interface SchedulerStatusEvent {
  workflow_id:   string;
  workflow_name: string;
  /** "waiting" | "running" | "done" | "error" | "stopped" */
  status:        string;
  run_count:     number;
  last_run_at:   string | null;
  next_run_at:   string | null;
  last_error:    string | null;
  /** Full WorkflowResult — only present on "waiting" (after success) or "error" status. */
  last_result:   import("./workflow").WorkflowResult | null;
}

let _unlistenSchedulerStatus: (() => void) | null = null;
let _schedulerRegistered = false;

export async function listenSchedulerStatus(
  cb: (event: SchedulerStatusEvent) => void
): Promise<() => void> {
  // Fix #10: guard against double-registration.
  if (_schedulerRegistered && _unlistenSchedulerStatus) {
    return _unlistenSchedulerStatus;
  }
  if (_unlistenSchedulerStatus) {
    _unlistenSchedulerStatus();
    _unlistenSchedulerStatus = null;
  }
  _schedulerRegistered = true;
  const win = getCurrentWebviewWindow();
  const unlisten = await win.listen<SchedulerStatusEvent>("scheduler-status", (e) => {
    cb(e.payload);
  });
  _unlistenSchedulerStatus = unlisten;
  return unlisten;
}

export interface SchedulerSkipEvent {
  workflow_id: string;
  reason: string;
}

let _unlistenSchedulerSkip: (() => void) | null = null;
let _schedulerSkipRegistered = false;

export async function listenSchedulerSkip(
  cb: (event: SchedulerSkipEvent) => void
): Promise<() => void> {
  if (_schedulerSkipRegistered && _unlistenSchedulerSkip) {
    return _unlistenSchedulerSkip;
  }
  if (_unlistenSchedulerSkip) {
    _unlistenSchedulerSkip();
    _unlistenSchedulerSkip = null;
  }
  _schedulerSkipRegistered = true;
  const win = getCurrentWebviewWindow();
  const unlisten = await win.listen<SchedulerSkipEvent>("scheduler-skip", (e) => {
    cb(e.payload);
  });
  _unlistenSchedulerSkip = unlisten;
  return unlisten;
}

/// Called once after `listenSchedulerStatus` resolves.
/// Triggers the backend to re-emit current scheduler state for all active jobs
/// and emit `scheduler-ready`. Replaces the old 800 ms startup delay.
export async function requestSchedulerState(): Promise<void> {
  await invoke<void>("request_scheduler_state");
}
