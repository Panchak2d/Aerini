import type { WorkflowManager } from "../workflow-manager";
import type { RunManager } from "../run-manager";
import { getBgJobs, removeBgJob } from "../run-manager";
import { stopScheduledWorkflow, startScheduledWorkflow, parseSchedulerError } from "../ipc/workflow";
import { updateActivityBadge, updateBgRunningState } from "../sidebar-sections";
import { formatDuration } from "../monitor-helpers";

type Toast = (msg: string, type: "success" | "error" | "info") => void;

const _countdownIntervals = new Map<string, ReturnType<typeof setInterval>>();
let _renderBgJobsTimer: ReturnType<typeof setTimeout> | null = null;

export function renderBgJobsDebounced(
  wfManager:    WorkflowManager,
  runManager:   RunManager,
  filterStatus  = "all",
  filterQuery   = "",
  toastFn?:     Toast,
): void {
  if (_renderBgJobsTimer !== null) clearTimeout(_renderBgJobsTimer);
  _renderBgJobsTimer = setTimeout(() => {
    _renderBgJobsTimer = null;
    renderBgJobs(wfManager, runManager, filterStatus, filterQuery, toastFn);
  }, 60);
}

export function renderBgJobs(
  wfManager:    WorkflowManager,
  runManager:   RunManager,
  filterStatus  = "all",
  filterQuery   = "",
  toastFn?:     Toast,
): void {
  const list = document.getElementById("bg-jobs-list");
  if (!list) return;

  let jobs = getBgJobs();

  updateActivityBadge("bgruns", jobs.length);
  updateBgRunningState(jobs.some(j => j.status === "running"));

  if (filterStatus !== "all") {
    jobs = jobs.filter(j => j.status === filterStatus);
  }
  if (filterQuery) {
    jobs = jobs.filter(j => j.name.toLowerCase().includes(filterQuery));
  }

  _countdownIntervals.forEach(id => clearInterval(id));
  _countdownIntervals.clear();

  list.innerHTML = "";

  if (!jobs.length) {
    const empty = document.createElement("div");
    empty.className = "bg-jobs-empty";
    empty.innerHTML = filterStatus !== "all" || filterQuery
      ? "<span>No runs match the filter</span>"
      : "<svg width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='2' stroke-linecap='round' opacity='0.35'><circle cx='12' cy='12' r='9'/><polygon points='10 8 16 12 10 16 10 8' fill='currentColor' stroke='none'/></svg><span>No scheduled workflows running</span><small>Use <strong>Schedule Run</strong> in the toolbar to run a workflow with a Schedule, Webhook, or trigger-plugin trigger in the background.</small>";
    list.appendChild(empty);
    return;
  }

  for (const job of jobs) {
    const item = document.createElement("div");
    item.className = "bg-job-item";

    const dot = document.createElement("span");
    dot.className = `bg-job-dot bg-job-dot--${job.status}`;

    const nameEl = document.createElement("span");
    nameEl.className = "bg-job-name";
    nameEl.textContent = job.name;
    nameEl.title = job.name;
    nameEl.setAttribute("data-tooltip", job.name);

    const statusEl = document.createElement("span");
    statusEl.className = `bg-job-status bg-job-status--${job.status}`;

    if (job.status === "running") {
      if (job.nextRunAt) {
        statusEl.dataset.nextRunAt = job.nextRunAt;
        const updateCountdown = () => {
          const target = statusEl.dataset.nextRunAt;
          if (!target) { statusEl.textContent = "running"; return; }
          const secsLeft = Math.max(0, Math.round(
            (new Date(target).getTime() - Date.now()) / 1000
          ));
          if (secsLeft === 0) {
            statusEl.textContent = "running…";
          } else if (secsLeft < 60) {
            statusEl.textContent = `next in ${secsLeft}s`;
          } else {
            const m = Math.floor(secsLeft / 60);
            const s = secsLeft % 60;
            statusEl.textContent = `next in ${m}m ${s}s`;
          }
        };
        updateCountdown();
        const timerId = setInterval(() => {
          if (!document.getElementById("bg-jobs-list")?.contains(item)) {
            clearInterval(timerId);
            return;
          }
          updateCountdown();
        }, 1000);
        _countdownIntervals.set(job.id, timerId);
      } else {
        statusEl.textContent = "running";
      }
    } else if (job.status === "done") {
      statusEl.textContent = formatDuration(job.finishedAt ? job.finishedAt - job.startedAt : 0);
    } else if (job.status === "stopped") {
      statusEl.textContent = "stopped";
    } else {
      statusEl.textContent = "failed";
    }

    const actions = document.createElement("span");
    actions.className = "bg-job-hover-actions";

    if (job.status === "running") {
      const stopBtn = document.createElement("button");
      stopBtn.className = "bg-job-action-btn bg-job-action-stop";
      stopBtn.title = "Stop workflow";
      stopBtn.setAttribute("data-tooltip", "Stop workflow");
      stopBtn.innerHTML = `<svg width="9" height="9" viewBox="0 0 24 24" fill="currentColor"><rect x="3" y="3" width="18" height="18" rx="2"/></svg>`;
      stopBtn.addEventListener("click", async (e) => {
        e.stopPropagation();
        stopBtn.disabled = true;
        try {
          await stopScheduledWorkflow(job.id);
        } catch {
          stopBtn.disabled = false; // event updates handle the success case; on failure, nothing else will
        }
      });
      actions.appendChild(stopBtn);
    } else {
      const restartBtn = document.createElement("button");
      restartBtn.className = "bg-job-action-btn bg-job-action-restart";
      restartBtn.title = "Restart workflow";
      restartBtn.setAttribute("data-tooltip", "Restart workflow");
      restartBtn.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="23 4 23 10 17 10"/><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/></svg>`;
      restartBtn.addEventListener("click", async (e) => {
        e.stopPropagation();
        restartBtn.disabled = true;
        try {
          await startScheduledWorkflow(job.id);
        } catch (err) {
          restartBtn.disabled = false;
          const parsed = parseSchedulerError(String(err));
          if (parsed.error_kind === "workflow_not_found") {
            toastFn?.(`Cannot restart "${job.name}" — workflow was deleted`, "error");
          } else {
            toastFn?.(`Restart failed: ${(parsed as { message?: string }).message ?? String(err)}`, "error");
          }
        }
      });
      actions.appendChild(restartBtn);

      const dismissBtn = document.createElement("button");
      dismissBtn.className = "bg-job-action-btn bg-job-action-dismiss";
      dismissBtn.title = "Remove from list";
      dismissBtn.setAttribute("data-tooltip", "Remove from list");
      dismissBtn.innerHTML = `<svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
      dismissBtn.addEventListener("click", (e) => {
        e.stopPropagation();
        removeBgJob(job.id);
        wfManager.refreshWorkflowList();
      });
      actions.appendChild(dismissBtn);
    }

    item.appendChild(dot);
    item.appendChild(nameEl);
    item.appendChild(statusEl);
    item.appendChild(actions);

    item.addEventListener("click", async () => {
      item.style.opacity = "0.5";
      item.style.pointerEvents = "none";
      try {
        await wfManager.handleLoad(job.id);
      } catch {
        toastFn?.("Workflow no longer exists", "error");
        item.classList.add("bg-job-item--stale");
        return;
      } finally {
        item.style.opacity = "";
        item.style.pointerEvents = "";
      }
      runManager.openHistoryDrawer();
    });

    list.appendChild(item);
  }
}

export function updateBgRunButton(currentWorkflowId: string): void {
  const btn = document.getElementById("btn-bg-run") as HTMLButtonElement | null;
  if (!btn) return;
  const jobs      = getBgJobs();
  const isRunning = jobs.some(j => j.id === currentWorkflowId && j.status === "running");
  btn.disabled    = isRunning;
  btn.textContent = isRunning ? "Running…" : "Schedule Run";
  btn.title = isRunning
    ? "This workflow is already running in the background. Stop it from the Background Runs panel to restart."
    : "Run silently in the background — result stored in history";
  btn.setAttribute("data-tooltip", isRunning ? "Already running in background" : "Run silently in the background");
}
