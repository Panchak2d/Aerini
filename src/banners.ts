import type { Canvas } from "./canvas/Canvas";
import { NODE_IDS } from "./node-ids";
import { checkNodejsAvailable } from "./ipc/workflow";

const BANNER_ID = "nodejs-missing-banner";

// Cached result of the last Node.js availability check this session.
// null = not checked yet, true = confirmed missing, false = confirmed available.
let _nodejsMissing: boolean | null = null;

export function isNodejsMissingCached(): boolean {
  return _nodejsMissing === true;
}

// Single place that updates the cache and syncs the banner DOM to match —
// every check path (startup, focus regain, settings button) routes through
// this so the cache and the banner can never drift apart.
export function setNodejsAvailability(available: boolean, canvas: Canvas): void {
  _nodejsMissing = !available;
  if (available) document.getElementById(BANNER_ID)?.remove();
  else injectNodejsBanner(canvas);
}

// Re-runs check_nodejs_available and applies the result. Used at startup and
// on window-focus regain.
export async function refreshNodejsAvailability(canvas: Canvas): Promise<boolean> {
  const available = await checkNodejsAvailable().catch(() => false);
  setNodejsAvailability(available, canvas);
  return available;
}

// Cheap reshow using only the cached state — no IPC call. Closes the
// "switched workflows after dismissing the startup banner" gap without
// rechecking Node.js on every workflow navigation.
export function reshowNodejsBannerIfStillMissing(canvas: Canvas): void {
  if (_nodejsMissing === true) injectNodejsBanner(canvas);
}

// Only recheck while still in the "missing" state — early-exits once
// resolved so window-focus events stop spawning subprocesses after that.
export function recheckNodejsOnFocusRegain(canvas: Canvas): void {
  if (_nodejsMissing === true) refreshNodejsAvailability(canvas).catch(() => {});
}

// Inject a dismissable banner into the command palette warning that Node.js is
// not found. Dismissing removes it for the session lifetime.
// Also surfaces a one-time toast if Code nodes are already in the workflow.
export function injectNodejsBanner(canvas: Canvas): void {
  const palette   = document.getElementById("command-palette");
  const inputWrap = document.getElementById("palette-input-wrap");
  if (!palette || !inputWrap) return;

  if (document.getElementById(BANNER_ID)) return;

  const banner = document.createElement("div");
  banner.id = BANNER_ID;
  banner.className = "nodejs-banner";
  banner.setAttribute("role", "alert");

  const msg = document.createElement("span");
  msg.textContent = "Code node requires Node.js — ";

  const link = document.createElement("a");
  link.textContent = "Download";
  link.href = "https://nodejs.org";
  link.target = "_blank";
  link.rel = "noopener noreferrer";

  const dismiss = document.createElement("button");
  dismiss.textContent = "×";
  dismiss.setAttribute("aria-label", "Dismiss");
  dismiss.className = "nodejs-banner-dismiss";
  dismiss.addEventListener("click", () => banner.remove());

  banner.appendChild(msg);
  banner.appendChild(link);
  banner.appendChild(dismiss);
  palette.insertBefore(banner, inputWrap);

  const hasCodeNode = Array.from(canvas.nodes.values()).some(
    n => n.data.node_type_id === NODE_IDS.CODE
  );
  if (hasCodeNode) {
    setTimeout(() => {
      const toastEl = document.createElement("div");
      toastEl.className = "toast toast--warning";
      toastEl.textContent = "This workflow uses Code nodes but Node.js was not found. Download it at nodejs.org.";
      document.body.appendChild(toastEl);
      setTimeout(() => toastEl.remove(), 8000);
    }, 1500);
  }
}
