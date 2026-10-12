// the dialog is one shared set of DOM elements (#confirm-modal and
// friends) — without serialization, two overlapping showConfirm() calls
// would stack two independent sets of onOk/onCancel listeners on the same
// OK/Cancel buttons, so a single click could silently resolve(true) two
// unrelated confirmations at once (e.g. a double-triggered Run firing
// handleRun() twice). _queue
// serializes every call through this module: a call that arrives while one
// is already showing doesn't open a second overlapping dialog — it waits for
// the current one to fully resolve and clean up (listeners removed, modal
// hidden) before its own message/listeners ever touch the shared elements.
// Each caller still gets its own dialog, its own message, and its own
// correct answer — nothing is skipped or merged, only serialized.
let _queue: Promise<void> = Promise.resolve();

function showConfirmNow(message: string, isDanger: boolean, okLabel: string, iconVariant: "danger" | "neutral"): Promise<boolean> {
  return new Promise(resolve => {
    const modal      = document.getElementById("confirm-modal")!;
    const msgEl      = document.getElementById("confirm-message")!;
    const okBtn      = document.getElementById("confirm-ok")! as HTMLButtonElement;
    const cancelBtn  = document.getElementById("confirm-cancel")!;
    const backdrop   = modal.querySelector(".confirm-backdrop")!;
    const iconDanger  = modal.querySelector(".confirm-icon--danger");
    const iconNeutral = modal.querySelector(".confirm-icon--neutral");

    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;

    msgEl.textContent = message;
    okBtn.textContent = okLabel;
    okBtn.classList.toggle("confirm-danger", isDanger);
    // Both icons are set explicitly every call, not just for "neutral" — the
    // modal's DOM is shared and reused (see module doc comment above), so a
    // prior call's variant would otherwise leak into this one.
    iconDanger?.classList.toggle("hidden", iconVariant !== "danger");
    iconNeutral?.classList.toggle("hidden", iconVariant !== "neutral");
    modal.classList.remove("hidden");

    const cleanup = (result: boolean) => {
      modal.classList.add("hidden");
      okBtn.removeEventListener("click", onOk);
      cancelBtn.removeEventListener("click", onCancel);
      backdrop.removeEventListener("click", onCancel);
      document.removeEventListener("keydown", onKey, true);
      if (previouslyFocused?.isConnected) previouslyFocused.focus();
      resolve(result);
    };

    const onOk     = () => cleanup(true);
    const onCancel = () => cleanup(false);

    // Capture phase, and every key is stopped from reaching the rest of the
    // app: while the dialog is up, Ctrl+Enter, Ctrl+N and the other global
    // shortcuts must not act on the page behind it. Enter and Space on a
    // focused button still activate it, since that is the browser's default
    // action, not a propagated event.
    const onKey = (e: KeyboardEvent) => {
      e.stopPropagation();
      if (e.key === "Escape") { e.preventDefault(); onCancel(); return; }
      if (e.key !== "Tab") return;
      const current = document.activeElement;
      const inside  = modal.contains(current);
      if (e.shiftKey ? (current === cancelBtn || !inside) : (current === okBtn || !inside)) {
        e.preventDefault();
        (e.shiftKey ? okBtn : cancelBtn).focus();
      }
    };

    okBtn.addEventListener("click", onOk);
    cancelBtn.addEventListener("click", onCancel);
    backdrop.addEventListener("click", onCancel);
    document.addEventListener("keydown", onKey, true);

    // A danger prompt opens on Cancel so a stray Enter cannot approve it.
    (isDanger ? cancelBtn : okBtn).focus();
  });
}

// Confirm dialog — replaces window.confirm, which Tauri webview suppresses.
// Returns Promise<boolean>: true on OK, false on Cancel / backdrop click.
export function showConfirm(message: string, isDanger = false, okLabel = "Continue", iconVariant: "danger" | "neutral" = "danger"): Promise<boolean> {
  const result = _queue.then(() => showConfirmNow(message, isDanger, okLabel, iconVariant));
  // Advance the queue regardless of this call's own outcome, so the next
  // queued call always waits for this dialog to close, not for its answer.
  _queue = result.then(() => undefined, () => undefined);
  return result;
}
