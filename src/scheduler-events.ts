import type { Canvas } from "./canvas/Canvas";
import type { WorkflowManager } from "./workflow-manager";
import type { RunManager } from "./run-manager";
import type { WorkflowResult } from "./ipc/workflow";
import type { SchedulerStatusEvent } from "./ipc/events";
import { listenNodeStatus, listenSchedulerStatus, listenSchedulerSkip, requestSchedulerState } from "./ipc/events";
import { getScheduledJobs } from "./ipc/workflow";
import { setWorkflowRunning } from "./workflow-manager";
import { getBgJobs, updateBgJobStoreFromEvent, hydrateBgJobsFromScheduler } from "./run-manager";
import { saveRunToHistory } from "./run-history";
import { isTauri } from "./utils";
import { activateZone, getCurrentZone } from "./sidebar-sections";
import { renderBgJobs, renderBgJobsDebounced, updateBgRunButton } from "./panels/BgJobsPanel";

type Toast  = (msg: string, type: "success" | "error" | "info") => void;
type Status = (msg: string) => void;

// listenSchedulerStatus (ipc/events.ts) guards against double win.listen()
// registration: the second caller gets back the existing unlisten handle
// without its own callback ever being wired up. bindSchedulerEvents below is
// always the first caller (registered at app init), so any other module that
// needs scheduler-status events (e.g. ChatPanel) must subscribe here instead
// of calling listenSchedulerStatus directly.
const _extraSchedulerListeners = new Set<(evt: SchedulerStatusEvent) => void>();

export function addSchedulerStatusListener(cb: (evt: SchedulerStatusEvent) => void): () => void {
  _extraSchedulerListeners.add(cb);
  return () => { _extraSchedulerListeners.delete(cb); };
}

export async function bindSchedulerEvents(
  canvas:        Canvas,
  wfManager:     WorkflowManager,
  runManager:    RunManager,
  toast:         Toast,
  setStatus:     Status,
  refreshRunBtn: () => void,
): Promise<void> {
  await listenNodeStatus((evt) => {
    runManager.onNodeStatusEvent(evt.workflow_id, evt.node_id, evt.status);
  });

  // Session-scoped set: tracks which workflow IDs have already activated the bg zone
  // this session. Prevents re-switching on every restart.
  const _bgZoneActivated = new Set<string>();

  await listenSchedulerStatus((evt: SchedulerStatusEvent) => {
    updateBgJobStoreFromEvent(evt);
    renderBgJobsDebounced(wfManager, runManager, "all", "", toast);
    updateBgRunButton(wfManager.currentId);
    refreshRunBtn();

    if (evt.status === "running" && evt.workflow_id === wfManager.currentId) {
      canvas.resetAllStatus();
      const drawer      = document.getElementById("output-drawer");
      const drawerTabs  = document.getElementById("drawer-tabs");
      const drawerContent = document.getElementById("output-content");
      if (drawer && !drawer.classList.contains("hidden")
          && drawerTabs && drawerContent
          && !drawer.querySelector(".history-panel")) {
        drawerTabs.innerHTML   = `<span class="drawer-tab tab-neutral active">Running…</span>`;
        drawerContent.innerHTML = `<div class="run-spinner-wrap"><div class="run-spinner"></div><div class="run-spinner-label">Scheduled run in progress…</div></div>`;
      }
    }
    if ((evt.status === "stopped" || evt.status === "error") && evt.workflow_id === wfManager.currentId) {
      canvas.resetAllStatus();
    }

    setWorkflowRunning(evt.workflow_id, evt.status === "running" || evt.status === "waiting");

    if (evt.status === "running" && !_bgZoneActivated.has(evt.workflow_id)) {
      _bgZoneActivated.add(evt.workflow_id);
      activateZone("bgruns");
    }

    if ((evt.status === "waiting" || evt.status === "error") && evt.last_result) {
      try {
        const result = evt.last_result as WorkflowResult;
        if (evt.workflow_id === wfManager.currentId) {
          runManager.showResultFromScheduler(evt.workflow_name, result);
        } else {
          const runId = `run_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
          saveRunToHistory(runId, evt.workflow_id, evt.workflow_name, result);
          if (!result.success) {
            toast(`"${evt.workflow_name}" failed — check Background Runs for details`, "error");
          }
        }
      } catch { /* non-fatal */ }
    }

    for (const fn of _extraSchedulerListeners) {
      try { fn(evt); } catch { /* a subscriber error must not break core scheduler UI */ }
    }
  });

  await listenSchedulerSkip((evt) => {
    const show = evt.workflow_id === wfManager.currentId || getCurrentZone() === "bgruns";
    if (!show) return;
    const job  = getBgJobs().find(j => j.id === evt.workflow_id);
    const name = job?.name ?? evt.workflow_id;
    toast(`"${name}" run skipped — previous run still in progress`, "info");
  });

  if (isTauri()) {
    requestSchedulerState().catch(() => {});

    getScheduledJobs().then(rows => {
      hydrateBgJobsFromScheduler(rows);
      renderBgJobs(wfManager, runManager, "all", "", toast);
      const activeCount = rows.filter(r => r.status === "active").length;
      if (activeCount > 0) {
        const label = activeCount === 1
          ? "1 workflow running in background"
          : `${activeCount} workflows running in background`;
        setStatus(label);
        toast(label, "info");
      }
    }).catch(() => {});
  }
}
