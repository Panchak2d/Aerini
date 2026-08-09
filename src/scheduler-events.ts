import type { Canvas } from "./canvas/Canvas";
import type { WorkflowManager } from "./workflow-manager";
import type { RunManager } from "./run-manager";
import type { WorkflowResult } from "./ipc/workflow";
import { ICON_CIRCLE_ALERT } from "./output-renderer";
import type { SchedulerStatusEvent } from "./ipc/events";
import { listenNodeStatus, listenSchedulerStatus, listenSchedulerSkip, requestSchedulerState } from "./ipc/events";
import { getScheduledJobs } from "./ipc/workflow";
import { setWorkflowRunning } from "./workflow-manager";
import { getBgJobs, updateBgJobStoreFromEvent, hydrateBgJobsFromScheduler } from "./run-manager";
import { saveRunToHistory } from "./run-history";
import { isTauri, escapeHtml } from "./utils";
import { NODE_IDS, findTriggerNodeTypeId } from "./node-ids";
import { getCurrentZone } from "./sidebar-sections";
import { loadBgPanel } from "./bg-panel-loader";

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
  refreshMem:    () => void,
  //  optional, like initStatusBarFields' own
  // onRunStateDetected — existing callers (and any future one that doesn't
  // care about the Performance panel) are unaffected. See the call below
  // for why a scheduler status transition is exactly when this is needed.
  refreshPerf?:  () => void,
): Promise<void> {
  await listenNodeStatus((evt) => {
    runManager.onNodeStatusEvent(evt.workflow_id, evt.node_id, evt.status);
  });

  await listenSchedulerStatus(async (evt: SchedulerStatusEvent) => {
    updateBgJobStoreFromEvent(evt);
    const bgPanel = await loadBgPanel();
    bgPanel.renderBgJobsDebounced(wfManager, runManager, "all", "", toast);
    bgPanel.updateBgRunButton(wfManager.currentId);
    refreshRunBtn();
    // The periodic memory-breakdown push only fires while the snapshot is
    // non-empty (lib.rs) — it never tells the frontend a run just ended.
    // Every scheduler status transition (start or end) is exactly when that
    // snapshot can change, so pull a fresh on-demand reading here rather
    // than waiting for the next workflow switch to reveal it.
    refreshMem();
    // Same gap, same fix, for the Performance panel: listenPerformanceLive
    // (PerformancePanel.ts) only pushes while >=1 run is in flight and
    // can't itself signal "a run just ended" — normally covered by
    // initStatusBarFields' #run-dropdown-wrap poll, but that element only
    // reflects the manual Run button, never a background/scheduled run.
    // Without this, the panel keeps showing whatever the last live push
    // left behind (often a stale "Running" snapshot) until the *next* run's
    // push overwrites it — visible to the user as the report vanishing.
    refreshPerf?.();

    if (evt.status === "running" && evt.workflow_id === wfManager.currentId) {
      canvas.resetAllStatus();
      const drawer      = document.getElementById("output-drawer");
      const drawerTabs  = document.getElementById("drawer-tabs");
      const drawerContent = document.getElementById("output-content");
      if (drawer && !drawer.classList.contains("hidden")
          && drawerTabs && drawerContent
          && !drawer.querySelector(".history-panel")) {
        // "Scheduled" is only accurate for a genuine Schedule-node trigger —
        // a Webhook or Manual trigger firing (e.g. Chat sending a message)
        // is a real, on-demand execution, not a schedule.
        const triggerType = findTriggerNodeTypeId(canvas.nodes.values());
        const label = triggerType === NODE_IDS.SCHEDULE ? "Scheduled run in progress…" : "Workflow running…";
        drawerTabs.innerHTML   = `<span class="drawer-tab tab-neutral active">Running…</span>`;
        drawerContent.innerHTML = `<div class="run-spinner-wrap"><div class="run-spinner"></div><div class="run-spinner-label">${label}</div></div>`;
      }
    }
    if ((evt.status === "stopped" || evt.status === "error") && evt.workflow_id === wfManager.currentId) {
      canvas.resetAllStatus();
      const drawer        = document.getElementById("output-drawer");
      const drawerTabs     = document.getElementById("drawer-tabs");
      const drawerContent  = document.getElementById("output-content");
      // Only replace content this handler injected itself (the "running"
      // spinner above) — never a real result, the Performance tab, or a
      // history panel just because status flipped. showResultFromScheduler
      // below overwrites this again whenever evt.last_result is present;
      // this only covers the gap where it isn't (stopped, or an error
      // before any result existed) — otherwise the spinner is left frozen
      // forever with no confirmation the run actually ended.
      if (drawer && !drawer.classList.contains("hidden")
          && drawerTabs && drawerContent
          && drawerContent.querySelector(".run-spinner-wrap")
          && !evt.last_result) {
        if (evt.status === "stopped") {
          drawerTabs.innerHTML    = `<span class="drawer-tab tab-neutral active">Stopped</span>`;
          drawerContent.innerHTML = `<div class="run-notice run-notice--warning"><strong><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="18" x="3" y="3" rx="2"/></svg>Stopped</strong>Workflow was stopped manually.</div>`;
        } else {
          const errMsg = evt.last_error ?? "Run failed.";
          drawerTabs.innerHTML    = `<span class="drawer-tab tab-error active">Error</span>`;
          drawerContent.innerHTML = `<div class="run-error-card">
            <div class="run-error-label">${ICON_CIRCLE_ALERT}Execution error</div>
            <div class="run-error-msg">${escapeHtml(errMsg)}</div>
            <button class="run-error-copy" id="sched-error-copy-btn">Copy error</button>
          </div>`;
          const copyBtn = drawerContent.querySelector<HTMLButtonElement>("#sched-error-copy-btn");
          copyBtn?.addEventListener("click", () => {
            navigator.clipboard.writeText(errMsg).then(() => { copyBtn.textContent = "Copied"; });
          });
        }
      }
    }

    setWorkflowRunning(evt.workflow_id, evt.status === "running" || evt.status === "waiting");

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

    getScheduledJobs().then(async rows => {
      hydrateBgJobsFromScheduler(rows);
      const bgPanel = await loadBgPanel();
      bgPanel.renderBgJobs(wfManager, runManager, "all", "", toast);
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
