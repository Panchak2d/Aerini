export type ToastType = "success" | "error" | "info" | "warning";

export const TOAST_MAX_VISIBLE = 3;
export const TOAST_SHOW_MS = 2800;
const TOAST_FADE_MS = 300;

const COLORS: Record<ToastType, string> = {
  success: "var(--green)",
  error: "var(--red)",
  info: "var(--blue)",
  warning: "var(--amber)",
};

const waiting: Array<{ msg: string; type: ToastType }> = [];

function stack(): HTMLElement {
  let el = document.getElementById("toast-stack");
  if (!el) {
    el = document.createElement("div");
    el.id = "toast-stack";
    el.className = "toast-stack";
    // Announced politely by screen readers without taking focus.
    el.setAttribute("role", "status");
    el.setAttribute("aria-live", "polite");
    document.body.appendChild(el);
  }
  return el;
}

function present(msg: string, type: ToastType): void {
  const host = stack();
  const t = document.createElement("div");
  t.className = "toast";
  t.style.background = COLORS[type];
  t.textContent = msg;
  host.appendChild(t);
  setTimeout(() => {
    t.style.opacity = "0";
    setTimeout(() => {
      t.remove();
      const next = waiting.shift();
      if (next) present(next.msg, next.type);
    }, TOAST_FADE_MS);
  }, TOAST_SHOW_MS);
}

/** Shows a toast. Up to three are stacked at once, oldest at the bottom; any more wait for a place. */
export function showToast(msg: string, type: ToastType = "info"): void {
  if (stack().childElementCount >= TOAST_MAX_VISIBLE) waiting.push({ msg, type });
  else present(msg, type);
}
