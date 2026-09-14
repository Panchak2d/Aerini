import type { Canvas } from "./canvas/Canvas";
import type { WorkflowManager } from "./workflow-manager";
import type { ScheduledJobRow } from "./ipc/workflow";
import { getScheduledJobs, setAlwaysOn, startScheduledWorkflow } from "./ipc/workflow";
import { isTauri } from "./utils";
import { NODE_IDS } from "./node-ids";
import { hideTooltipFor } from "./tooltip-manager";

type Toast = (msg: string, type: "success" | "error" | "info") => void;

const NO_TRIGGER_TOOLTIP = "Add a Schedule or Webhook trigger to enable Run on launch";
const SCHEDULABLE_TRIGGER_IDS: Set<string> = new Set([NODE_IDS.SCHEDULE, NODE_IDS.WEBHOOK]);

export async function updateAlwaysOnBtn(canvas: Canvas, wfManager: WorkflowManager): Promise<void> {
  const btn = document.getElementById("btn-always-on") as HTMLButtonElement | null;
  if (!btn) return;

  const hasSchedulableTrigger = [...canvas.nodes.values()].some(n =>
    SCHEDULABLE_TRIGGER_IDS.has(n.data.node_type_id)
  );

  const wrap = document.getElementById("always-on-wrap");
  if (!hasSchedulableTrigger) {
    btn.classList.add("hidden");
    if (wrap) {
      wrap.style.display = "";
      wrap.setAttribute("data-tooltip", NO_TRIGGER_TOOLTIP);
    }
    return;
  }

  btn.classList.remove("hidden");
  if (wrap) {
    wrap.removeAttribute("data-tooltip");
    hideTooltipFor(wrap);
  }

  let alwaysOn = false;
  if (isTauri()) {
    try {
      const jobs = await getScheduledJobs();
      const job  = jobs.find((j: ScheduledJobRow) => j.workflow_id === wfManager.currentId);
      alwaysOn   = job?.always_on ?? false;
    } catch { /* non-fatal */ }
  }

  btn.classList.toggle("btn-always-on--active", alwaysOn);
  btn.title = alwaysOn
    ? "Run on launch is ON — this workflow starts automatically when Aerini opens. Click to disable."
    : "Run on launch is OFF — click to make this workflow start automatically when Aerini opens.";
}

export function bindAlwaysOnToggle(canvas: Canvas, wfManager: WorkflowManager, toast: Toast): void {
  const btn = document.getElementById("btn-always-on") as HTMLButtonElement | null;
  if (!btn) return;

  btn.addEventListener("click", async () => {
    const isActive = btn.classList.contains("btn-always-on--active");
    const enabling = !isActive;

    if (enabling) {
      const scheduleNode = [...canvas.nodes.values()].find(n => n.data.node_type_id === NODE_IDS.SCHEDULE);
      const mode  = scheduleNode?.data.config["mode"] as string | undefined;
      const runAt = scheduleNode?.data.config["run_at"] as string | undefined;
      if (mode === "once" && runAt && new Date(runAt).getTime() <= Date.now()) {
        toast(
          "This Schedule is a one-time run already in the past — turning on Run on launch would fire it again on every app launch. Set a new date, or switch to Interval or Cron.",
          "error",
        );
        return;
      }
    }

    btn.disabled = true;
    try {
      if (enabling) {
        const snapshot = await wfManager.prepareForBgRun();
        if (!snapshot) { btn.disabled = false; return; }
        const jobs        = await getScheduledJobs();
        const existingJob = jobs.find((j: ScheduledJobRow) => j.workflow_id === wfManager.currentId);
        const isRunning   = existingJob?.status === "active";
        if (!isRunning) {
          await startScheduledWorkflow(wfManager.currentId, undefined, true);
        } else {
          await setAlwaysOn(wfManager.currentId, true);
        }
      } else {
        await setAlwaysOn(wfManager.currentId, false);
      }
      btn.classList.toggle("btn-always-on--active", enabling);
      btn.title = enabling
        ? "Run on launch is ON — this workflow starts automatically when Aerini opens. Click to disable."
        : "Run on launch is OFF — click to make this workflow start automatically when Aerini opens.";
      toast(
        enabling ? "Run on launch enabled — workflow will start on every app launch" : "Run on launch disabled",
        enabling ? "success" : "info",
      );
    } catch (e) {
      toast(`Could not update run on launch: ${e}`, "error");
    } finally {
      btn.disabled = false;
    }
  });
}
