import type { Canvas } from "./canvas/Canvas";
import { NODE_IDS } from "./node-ids";

// Inject a dismissable banner into the command palette warning that Node.js is
// not found. Dismissing removes it for the session lifetime.
// Also surfaces a one-time toast if Code nodes are already in the workflow.
export function injectNodejsBanner(canvas: Canvas): void {
  const palette   = document.getElementById("command-palette");
  const inputWrap = document.getElementById("palette-input-wrap");
  if (!palette || !inputWrap) return;

  const BANNER_ID = "nodejs-missing-banner";
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
