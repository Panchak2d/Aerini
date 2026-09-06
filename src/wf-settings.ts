import type { WorkflowManager } from "./workflow-manager";

export function bindWfSettings(wfManager: WorkflowManager): void {
  const $ = (id: string) => document.getElementById(id)!;

  function renderTagChips(): void {
    const chips = $("wf-setting-tags-chips");
    chips.innerHTML = "";
    for (const tag of wfManager.currentTags) {
      const chip = document.createElement("span");
      chip.className = "wf-tag-chip";
      const label = document.createElement("span");
      label.textContent = tag;
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "wf-tag-chip-remove";
      remove.textContent = "✕";
      remove.title = `Remove "${tag}"`;
      remove.addEventListener("click", () => {
        wfManager.currentTags = wfManager.currentTags.filter(t => t !== tag);
        wfManager.markUnsaved(true);
        renderTagChips();
      });
      chip.appendChild(label);
      chip.appendChild(remove);
      chips.appendChild(chip);
    }
  }

  $("btn-wf-settings").addEventListener("click", () => {
    const modal           = $("wf-settings-modal");
    const parallelChk     = document.getElementById("wf-setting-parallel") as HTMLInputElement;
    const concurrencyRow  = document.getElementById("wf-setting-concurrency-row") as HTMLElement;
    const maxConcInput    = document.getElementById("wf-setting-max-concurrent") as HTMLInputElement;
    parallelChk.checked   = wfManager.parallelExecution;
    maxConcInput.value    = String(wfManager.maxConcurrentNodes);
    concurrencyRow.hidden = !wfManager.parallelExecution;
    (document.getElementById("wf-setting-unlimited-duration") as HTMLInputElement).checked = wfManager.unlimitedDuration;
    (document.getElementById("wf-setting-max-duration") as HTMLInputElement).value = wfManager.maxDurationSecs === undefined ? "" : String(wfManager.maxDurationSecs);
    (document.getElementById("wf-setting-max-duration-row") as HTMLElement).hidden = wfManager.unlimitedDuration;

    const cs = wfManager.chatSettings;
    (document.getElementById("wf-setting-chat-attachments") as HTMLInputElement).checked = cs.allow_attachments;
    (document.getElementById("wf-setting-chat-images")      as HTMLInputElement).checked = cs.allow_image_responses;
    (document.getElementById("wf-setting-chat-max-length")  as HTMLInputElement).value    = String(cs.max_message_length);
    (document.getElementById("wf-setting-chat-persistence") as HTMLInputElement).checked = cs.session_persistence;
    (document.getElementById("wf-setting-chat-branding")    as HTMLInputElement).checked = cs.show_branding;

    renderTagChips();
    modal.classList.remove("hidden");
  });

  document.getElementById("wf-setting-tags-input")!.addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key !== "Enter") return;
    e.preventDefault();
    const input = e.target as HTMLInputElement;
    const value = input.value.trim();
    input.value = "";
    if (!value) return;
    const isDuplicate = wfManager.currentTags.some(t => t.toLowerCase() === value.toLowerCase());
    if (isDuplicate) return;
    wfManager.currentTags = [...wfManager.currentTags, value];
    wfManager.markUnsaved(true);
    renderTagChips();
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

  document.getElementById("wf-setting-unlimited-duration")!.addEventListener("change", (e) => {
    const checked = (e.target as HTMLInputElement).checked;
    wfManager.unlimitedDuration = checked;
    wfManager.markUnsaved(true);
    document.getElementById("wf-setting-max-duration-row")!.hidden = checked;
  });

  document.getElementById("wf-setting-max-duration")!.addEventListener("change", (e) => {
    const raw = (e.target as HTMLInputElement).value.trim();
    if (raw === "") {
      wfManager.maxDurationSecs = undefined;
      wfManager.markUnsaved(true);
      return;
    }
    const val = parseInt(raw, 10);
    if (!isNaN(val) && val >= 10 && val <= 86400) {
      wfManager.maxDurationSecs = val;
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
