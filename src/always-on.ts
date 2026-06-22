import type { Canvas } from "./canvas/Canvas";
import type { WorkflowManager } from "./workflow-manager";
import type { ScheduledJobRow } from "./ipc/workflow";
import { getScheduledJobs, setAlwaysOn, startScheduledWorkflow } from "./ipc/workflow";
import { isTauri } from "./utils";
import { TRIGGER_NODE_IDS } from "./node-ids";

type Toast = (msg: string, type: "success" | "error" | "info") => void;

export async function updateAlwaysOnBtn(canvas: Canvas, wfManager: WorkflowManager): Promise<void> {
  const btn = document.getElementById("btn-always-on") as HTMLButtonElement | null;
  if (!btn) return;

  const hasSchedulableTrigger = [...canvas.nodes.values()].some(n =>
    TRIGGER_NODE_IDS.has(n.data.node_type_id as string)
  );

  const wrap = document.getElementById("always-on-wrap");
  if (!hasSchedulableTrigger) {
    btn.classList.add("hidden");
    if (wrap) wrap.style.display = "";
    return;
  }

  btn.classList.remove("hidden");
  if (wrap) wrap.removeAttribute("data-tooltip");

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

export function bindAlwaysOnToggle(wfManager: WorkflowManager, toast: Toast): void {
  const btn = document.getElementById("btn-always-on") as HTMLButtonElement | null;
  if (!btn) return;

  btn.addEventListener("click", async () => {
    const isActive = btn.classList.contains("btn-always-on--active");
    btn.disabled = true;
    try {
      const enabling = !isActive;
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
