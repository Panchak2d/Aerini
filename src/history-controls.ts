import type { Canvas } from "./canvas/Canvas";
import type { HistoryState } from "./canvas/UndoManager";
import { buildIconBtn, type Toast } from "./toolbar-helpers";

const ICON_ATTRS = `width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"`;

/** Undo / Redo buttons plus status-bar and screen-reader feedback for every history change. */
export function bindHistoryControls(canvas: Canvas, toast: Toast): void {
  const zoomGroup = document.querySelector<HTMLElement>(".toolbar-zoom-group");
  if (!zoomGroup?.parentElement) return;

  const group = document.createElement("div");
  group.className = "toolbar-zoom-group";
  group.setAttribute("role", "group");
  group.setAttribute("aria-label", "Undo and redo");
  const undoBtn = buildIconBtn("btn-undo", "Undo (Ctrl+Z)",
    `<svg ${ICON_ATTRS}><path d="M3 7v6h6"/><path d="M21 17a9 9 0 0 0-9-9 9 9 0 0 0-6 2.3L3 13"/></svg>`);
  const redoBtn = buildIconBtn("btn-redo", "Redo (Ctrl+Shift+Z)",
    `<svg ${ICON_ATTRS}><path d="M21 7v6h-6"/><path d="M3 17a9 9 0 0 1 9-9 9 9 0 0 1 6 2.3L21 13"/></svg>`);
  undoBtn.addEventListener("click", () => canvas.undo());
  redoBtn.addEventListener("click", () => canvas.redo());
  group.append(undoBtn, redoBtn);
  zoomGroup.insertAdjacentElement("beforebegin", group);
  const divider = document.getElementById("toolbar-divider-new-workflow");
  if (divider) {
    const sep = divider.cloneNode(true) as HTMLElement;
    sep.removeAttribute("id");
    group.insertAdjacentElement("afterend", sep);
  }

  const setBtn = (btn: HTMLButtonElement, verb: string, keys: string, enabled: boolean, label: string | null) => {
    const text = enabled && label ? `${verb}: ${label} (${keys})` : `${verb} (${keys})`;
    btn.disabled = !enabled;
    btn.title = text;
    btn.setAttribute("data-tooltip", text);
    btn.setAttribute("aria-label", text);
  };
  const STATUS_HOLD_MS = 4000;
  const ANNOUNCE_GAP_MS = 50;
  let heldStatus: { msg: string; prior: string } | null = null;
  let statusTimer: ReturnType<typeof setTimeout> | undefined;
  let announceTimer: ReturnType<typeof setTimeout> | undefined;
  const say = (msg: string) => {
    const status = document.getElementById("status-text");
    if (status) {
      const prior = heldStatus && status.textContent === heldStatus.msg ? heldStatus.prior : (status.textContent ?? "");
      status.textContent = msg;
      heldStatus = { msg, prior };
      clearTimeout(statusTimer);
      statusTimer = setTimeout(() => {
        if (heldStatus && status.textContent === heldStatus.msg) status.textContent = heldStatus.prior;
        heldStatus = null;
      }, STATUS_HOLD_MS);
    }
    const ann = document.getElementById("a11y-announcer");
    if (ann) {
      // A live region only speaks when its text changes, so an identical repeat is cleared first.
      ann.textContent = "";
      clearTimeout(announceTimer);
      announceTimer = setTimeout(() => { ann.textContent = msg; }, ANNOUNCE_GAP_MS);
    }
  };

  const render = (s: HistoryState) => {
    setBtn(undoBtn, "Undo", "Ctrl+Z", s.canUndo, s.undoLabel);
    setBtn(redoBtn, "Redo", "Ctrl+Shift+Z", s.canRedo, s.redoLabel);
    switch (s.kind) {
      case "undo":       say(`Undid: ${s.label}`); break;
      case "redo":       say(`Redid: ${s.label}`); break;
      case "empty-undo": toast("Nothing to undo", "info"); break;
      case "empty-redo": toast("Nothing to redo", "info"); break;
      case "busy":       toast("Finish the current action first, then undo or redo", "info"); break;
      case "failed":     toast(`Couldn't ${s.label ? `undo or redo "${s.label}"` : "undo or redo"}. That step was dropped.`, "error"); break;
    }
  };
  const prev = canvas.onHistoryChange;
  canvas.onHistoryChange = (s) => { prev?.(s); render(s); };
  render({ ...canvas.historyState(), kind: "push" });
}
