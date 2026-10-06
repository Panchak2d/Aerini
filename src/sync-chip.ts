import type { Canvas } from "./canvas/Canvas";
import type { WorkflowManager } from "./workflow-manager";
import { isWorkflowRunning } from "./workflow-manager";
import { canvasTriggerFingerprint, type RunningSync } from "./running-sync";

export type SyncSignal = "autosave_failed" | "trigger_changed" | null;

export function syncSignal(autoSaveFailed: boolean, running: boolean, triggerChanged: boolean): SyncSignal {
  if (autoSaveFailed && running) return "autosave_failed";
  return triggerChanged ? "trigger_changed" : null;
}

const CHIP_TEXT: Record<Exclude<SyncSignal, null>, { text: string; title: string }> = {
  autosave_failed: { text: "Not saved", title: "Autosave failed, so the running workflow can't see your latest changes. Press Ctrl+S to retry." },
  trigger_changed: { text: "Restart to apply trigger", title: "The trigger changed since this workflow started. Stop it and start it again to apply the change." },
};

export function initSyncChip(canvas: Canvas, wfManager: WorkflowManager, sync: RunningSync): void {
  const chip = document.getElementById("sync-chip");
  if (!chip) return;
  const render = (): void => {
    const id = wfManager.currentId;
    const signal = syncSignal(wfManager.autoSaveFailed, isWorkflowRunning(id), sync.triggerChanged(id, canvasTriggerFingerprint(canvas)));
    chip.classList.toggle("hidden", signal === null);
    chip.textContent = signal ? CHIP_TEXT[signal].text : "";
    chip.title = signal ? CHIP_TEXT[signal].title : "";
  };
  sync.onChange(render);
  render();
}
