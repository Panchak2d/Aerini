export const isTauri = (): boolean => "__TAURI_INTERNALS__" in window;

export function escapeHtml(s: string): string {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

const COPY_FEEDBACK_MS = 2000;

export async function copyWithButtonFeedback(btn: HTMLElement, text: string): Promise<void> {
  const original = btn.dataset.copyLabel ?? btn.textContent ?? "";
  btn.dataset.copyLabel = original;
  const ok = await copyText(text);
  btn.textContent = ok ? "Copied" : "Copy failed";
  setTimeout(() => {
    btn.textContent = original;
    delete btn.dataset.copyLabel;
  }, COPY_FEEDBACK_MS);
}
