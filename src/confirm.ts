// Confirm dialog — replaces window.confirm, which Tauri webview suppresses.
// Returns Promise<boolean>: true on OK, false on Cancel / backdrop click.
export function showConfirm(message: string, isDanger = false, okLabel = "Continue"): Promise<boolean> {
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
