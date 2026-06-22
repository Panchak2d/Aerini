import type { WorkflowManager } from "./workflow-manager";

export function bindWfSettings(wfManager: WorkflowManager): void {
  const $ = (id: string) => document.getElementById(id)!;

  $("btn-wf-settings").addEventListener("click", () => {
    const modal           = $("wf-settings-modal");
    const parallelChk     = document.getElementById("wf-setting-parallel") as HTMLInputElement;
    const concurrencyRow  = document.getElementById("wf-setting-concurrency-row") as HTMLElement;
    const maxConcInput    = document.getElementById("wf-setting-max-concurrent") as HTMLInputElement;
    parallelChk.checked   = wfManager.parallelExecution;
    maxConcInput.value    = String(wfManager.maxConcurrentNodes);
    concurrencyRow.hidden = !wfManager.parallelExecution;

    const cs = wfManager.chatSettings;
    (document.getElementById("wf-setting-chat-attachments") as HTMLInputElement).checked = cs.allow_attachments;
    (document.getElementById("wf-setting-chat-images")      as HTMLInputElement).checked = cs.allow_image_responses;
    (document.getElementById("wf-setting-chat-max-length")  as HTMLInputElement).value    = String(cs.max_message_length);
    (document.getElementById("wf-setting-chat-persistence") as HTMLInputElement).checked = cs.session_persistence;
    (document.getElementById("wf-setting-chat-branding")    as HTMLInputElement).checked = cs.show_branding;

    modal.classList.remove("hidden");
  });

  $("btn-close-wf-settings").addEventListener("click", () => {
    $("wf-settings-modal").classList.add("hidden");
  });

  document.getElementById("wf-setting-parallel")!.addEventListener("change", (e) => {
    const checked             = (e.target as HTMLInputElement).checked;
    wfManager.parallelExecution = checked;
    wfManager.markUnsaved(true);
    document.getElementById("wf-setting-concurrency-row")!.hidden = !checked;
  });

  document.getElementById("wf-setting-max-concurrent")!.addEventListener("change", (e) => {
    const val = parseInt((e.target as HTMLInputElement).value, 10);
    if (!isNaN(val) && val >= 1 && val <= 64) {
      wfManager.maxConcurrentNodes = val;
      wfManager.markUnsaved(true);
    }
  });

  document.getElementById("wf-setting-chat-attachments")!.addEventListener("change", (e) => {
    wfManager.chatSettings.allow_attachments = (e.target as HTMLInputElement).checked;
    wfManager.markUnsaved(true);
  });

  document.getElementById("wf-setting-chat-images")!.addEventListener("change", (e) => {
    wfManager.chatSettings.allow_image_responses = (e.target as HTMLInputElement).checked;
    wfManager.markUnsaved(true);
  });

  document.getElementById("wf-setting-chat-max-length")!.addEventListener("change", (e) => {
    const val = parseInt((e.target as HTMLInputElement).value, 10);
    if (!isNaN(val) && val >= 1 && val <= 100_000) {
      wfManager.chatSettings.max_message_length = val;
      wfManager.markUnsaved(true);
    }
  });

  document.getElementById("wf-setting-chat-persistence")!.addEventListener("change", (e) => {
    wfManager.chatSettings.session_persistence = (e.target as HTMLInputElement).checked;
    wfManager.markUnsaved(true);
  });

  document.getElementById("wf-setting-chat-branding")!.addEventListener("change", (e) => {
    wfManager.chatSettings.show_branding = (e.target as HTMLInputElement).checked;
    wfManager.markUnsaved(true);
  });

  $("wf-settings-modal").addEventListener("click", (e) => {
    if (e.target === $("wf-settings-modal")) $("wf-settings-modal").classList.add("hidden");
  });
}
