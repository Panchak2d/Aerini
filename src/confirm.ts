// the dialog is one shared set of DOM elements (#confirm-modal and
// friends) — two overlapping showConfirm() calls used to stack two
// independent sets of onOk/onCancel listeners on the same OK/Cancel buttons,
// so a single click could silently resolve(true) two unrelated confirmations
// at once (e.g. a double-triggered Run firing handleRun() twice). _queue
// serializes every call through this module: a call that arrives while one
// is already showing doesn't open a second overlapping dialog — it waits for
// the current one to fully resolve and clean up (listeners removed, modal
// hidden) before its own message/listeners ever touch the shared elements.
// Each caller still gets its own dialog, its own message, and its own
// correct answer — nothing is skipped or merged, only serialized.
let _queue: Promise<void> = Promise.resolve();

function showConfirmNow(message: string, isDanger: boolean, okLabel: string): Promise<boolean> {
  return new Promise(resolve => {
    const modal      = document.getElementById("confirm-modal")!;
    const msgEl      = document.getElementById("confirm-message")!;
    const okBtn      = document.getElementById("confirm-ok")! as HTMLButtonElement;
    const cancelBtn  = document.getElementById("confirm-cancel")!;
    const backdrop   = modal.querySelector(".confirm-backdrop")!;

    msgEl.textContent = message;
    okBtn.textContent = okLabel;
    okBtn.classList.toggle("confirm-danger", isDanger);
    modal.classList.remove("hidden");

    const cleanup = (result: boolean) => {
      modal.classList.add("hidden");
      okBtn.removeEventListener("click", onOk);
      cancelBtn.removeEventListener("click", onCancel);
      backdrop.removeEventListener("click", onCancel);
      resolve(result);
    };

    const onOk     = () => cleanup(true);
    const onCancel = () => cleanup(false);

    okBtn.addEventListener("click", onOk);
    cancelBtn.addEventListener("click", onCancel);
    backdrop.addEventListener("click", onCancel);
  });
}

// Confirm dialog — replaces window.confirm, which Tauri webview suppresses.
// Returns Promise<boolean>: true on OK, false on Cancel / backdrop click.
export function showConfirm(message: string, isDanger = false, okLabel = "Continue"): Promise<boolean> {
  const result = _queue.then(() => showConfirmNow(message, isDanger, okLabel));
  // Advance the queue regardless of this call's own outcome, so the next
  // queued call always waits for this dialog to close, not for its answer.
  _queue = result.then(() => undefined, () => undefined);
  return result;
}
