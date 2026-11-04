/**
 * Adds the data-insertion UI to a text field: a labelled "Insert data" button,
 * expression styling, a live preview line from the last run, a one-click
 * suggestion for a blank main field, and auto-open of the picker on focus.
 */

import type { CanvasNode } from "../canvas/Node";
import {
  closeExpressionPicker,
  insertAtCursor,
  isExpressionPickerOpen,
  showExpressionPicker,
} from "../expression-picker";
import { getUpstreamNodes, mainResultExpr, onRunOutputsChange, sourceColorIndex, suggestMainResult, type SourceGraph } from "../data-sources";
import { hasReference, listReferences, previewField, type FieldReference } from "../field-preview";
import { getIconSvg } from "../icon-cache";
import { isMentionStart } from "../mention";
import { findLiveCandidate, SMART_PASTE_DISMISS_MAX, type LiveCandidate } from "../smart-paste";
import { armPointMode, disarmPointMode } from "../point-mode";
import { registerFieldDropTarget } from "../field-wire-drop";
import { mountDescribeIt, type DescribeGraph } from "./describe-panel";

/** Set to false to stop the picker opening by itself when an empty field is focused. */
export const AUTO_OPEN_ON_FOCUS = true;

/** Focus that arrives this soon after the form is built is the popover's own focus move, not the user's. */
const PROGRAMMATIC_FOCUS_MS = 300;
const PREVIEW_MAX = 160;
const PREVIEW_EXPANDED_MAX = 2000;
/** Typed text is checked for a live-value match after this much idle time; pasted text at once. */
const SMART_PASTE_IDLE_MS = 600;

const dismissedValues = new Map<string, Set<string>>();

const ICON_ATTRS = `width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"`;
const CLOSE_SVG = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" aria-hidden="true"><line x1="6" y1="6" x2="18" y2="18"/><line x1="18" y1="6" x2="6" y2="18"/></svg>`;
/** More chips than this are folded into one summary chip so a long template does not flood the popover. */
const CHIPS_MAX = 20;
const PLUS_SVG = `<svg ${ICON_ATTRS}><rect x="3" y="3" width="18" height="18" rx="3"/><line x1="12" y1="8" x2="12" y2="16"/><line x1="8" y1="12" x2="16" y2="12"/></svg>`;

type Field = HTMLInputElement | HTMLTextAreaElement;

interface CanvasLike extends SourceGraph {
  nodeIdAtEvent?: (e: MouseEvent) => string | null;
  outputPortNodeAtEvent?: (e: MouseEvent) => string | null;
  setRefHighlights?: (sources: Map<string, number> | null, targetNodeId: string | null) => void;
  revealNode?: (n: unknown) => void;
  editNode?: (n: CanvasNode, mutate: () => void) => void;
}

export function mountExpressionUi(
  field: Field,
  host: HTMLElement,
  node: CanvasNode,
  canvasEl: HTMLCanvasElement,
  isPrimary = false,
  fieldKey = "",
): void {
  const mountedAt = Date.now();
  const graph = () => (canvasEl as unknown as { __canvas?: SourceGraph }).__canvas;

  const bar = document.createElement("div");
  bar.className = "field-expr-bar";

  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "field-insert-data";
  btn.setAttribute("aria-label", "Insert data from a previous node");
  btn.title = "Insert data from a previous node. You can also type {{";
  btn.innerHTML = `${PLUS_SVG}<span>Insert data</span>`;
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    if (isExpressionPickerOpen()) { closeExpressionPicker(); return; }
    showExpressionPicker(btn, field, node.data.id, canvasEl, { fieldKey: fieldKey || undefined });
  });
  bar.appendChild(btn);

  mountDescribeIt(field, host, bar, node.data.id, () => graph() as unknown as DescribeGraph | undefined);

  const chip = document.createElement("button");
  chip.type = "button";
  chip.className = "field-expr-suggest";
  chip.hidden = true;
  bar.appendChild(chip);
  host.appendChild(bar);

  const preview = document.createElement("div");
  preview.className = "field-expr-preview";
  preview.setAttribute("aria-live", "polite");
  preview.hidden = true;
  host.appendChild(preview);

  const live = document.createElement("div");
  live.className = "field-expr-live";
  live.setAttribute("aria-live", "polite");
  live.hidden = true;
  host.appendChild(live);

  const refs = document.createElement("div");
  refs.className = "field-expr-refs";
  refs.hidden = true;
  host.appendChild(refs);

  const canvasObj = () => graph() as CanvasLike | undefined;
  /** Chips for plain references that point at another node on the canvas, in field order. */
  function chipRefs(): { ref: FieldReference; id: string; slot: number }[] {
    const g = canvasObj();
    const out: { ref: FieldReference; id: string; slot: number }[] = [];
    if (!g) return out;
    const byName = new Map<string, string>();
    for (const n of g.nodes.values()) if (!byName.has(n.data.name)) byName.set(n.data.name, n.data.id);
    for (const ref of listReferences(field.value)) {
      const id = byName.get(ref.nodeName);
      if (id !== undefined && id !== node.data.id) out.push({ ref, id, slot: sourceColorIndex(id) });
    }
    return out;
  }
  /** Source nodes this field references, by id, with their color slot. */
  function referenced(): Map<string, number> {
    return new Map(chipRefs().map((c) => [c.id, c.slot] as const));
  }
  const highlight = (m: Map<string, number> | null) => canvasObj()?.setRefHighlights?.(m, node.data.id);

  /** Drops one reference from the text. Goes through the same value, selection and `input` event as a typed edit. */
  function removeReference(ref: FieldReference): void {
    const text = field.value;
    if (!text.slice(ref.start, ref.end).startsWith("{{")) return;
    field.value = text.slice(0, ref.start) + text.slice(ref.end);
    field.selectionStart = field.selectionEnd = ref.start;
    field.dispatchEvent(new Event("input", { bubbles: true }));
    field.focus();
  }

  function refreshRefs(): void {
    const g = canvasObj();
    const chips = chipRefs();
    refs.replaceChildren();
    refs.hidden = chips.length === 0;
    for (const { ref, id, slot } of chips.slice(0, CHIPS_MAX)) {
      const target = g?.nodes.get(id);
      if (!target) continue;
      const label = [target.data.name, ...ref.path].join(", ");
      const pill = document.createElement("span");
      pill.className = "ref-pill";
      pill.style.setProperty("--ref", `var(--ref-color-${slot})`);

      const main = document.createElement("button");
      main.type = "button";
      main.className = "ref-pill-main";
      main.title = ref.wrapped
        ? `Uses ${label} inside a function. Click to find the node and see its fields.`
        : `Uses ${label}. Click to find the node and see its fields. Backspace removes it.`;
      const typeId = (target.data as { node_type_id?: string }).node_type_id;
      if (typeId) {
        const icon = document.createElement("span");
        icon.className = "ref-pill-icon";
        icon.innerHTML = getIconSvg(typeId);
        main.appendChild(icon);
      }
      const name = document.createElement("span");
      name.className = "ref-pill-name";
      name.textContent = target.data.name;
      main.appendChild(name);
      for (const seg of ref.path) {
        const sep = document.createElement("span");
        sep.className = "expr-picker-sep";
        sep.setAttribute("aria-hidden", "true");
        const segEl = document.createElement("span");
        segEl.className = "ref-pill-seg";
        segEl.textContent = seg;
        main.append(sep, segEl);
      }
      main.addEventListener("click", () => {
        g?.revealNode?.(target);
        showExpressionPicker(btn, field, node.data.id, canvasEl, { expandNodeId: id, fieldKey: fieldKey || undefined });
      });
      main.addEventListener("keydown", (e) => {
        if (ref.wrapped || (e.key !== "Backspace" && e.key !== "Delete")) return;
        e.preventDefault();
        removeReference(ref);
      });

      const close = document.createElement("button");
      close.type = "button";
      close.className = "ref-pill-close";
      close.setAttribute("aria-label", `Remove ${label} from this field`);
      close.title = "Remove";
      close.innerHTML = CLOSE_SVG;
      close.addEventListener("click", () => removeReference(ref));

      if (ref.wrapped) pill.append(main);
      else pill.append(main, close);
      const on = () => highlight(new Map([[id, slot]]));
      const off = () => highlight(document.activeElement === field ? referenced() : null);
      pill.addEventListener("mouseenter", on);
      pill.addEventListener("focusin", on);
      pill.addEventListener("mouseleave", off);
      pill.addEventListener("focusout", off);
      refs.appendChild(pill);
    }
    if (chips.length > CHIPS_MAX) {
      const more = document.createElement("span");
      more.className = "ref-pill-more";
      more.textContent = `+${chips.length - CHIPS_MAX} more`;
      refs.appendChild(more);
    }
    if (document.activeElement === field) highlight(referenced());
  }

  function armPoint(): void {
    const g = canvasObj();
    if (!g?.nodeIdAtEvent) return;
    const ups = getUpstreamNodes(g, node.data.id);
    armPointMode({
      eligible: new Set(ups.map((u) => u.id)),
      hit: (e) => g.nodeIdAtEvent!(e),
      insert: (expr) => insertAtCursor(field, expr),
      onPick: (id, again) => {
        const up = getUpstreamNodes(g, node.data.id).find((u) => u.id === id);
        const main = up ? mainResultExpr(up) : null;
        if (again || !main) showExpressionPicker(btn, field, node.data.id, canvasEl, { expandNodeId: id, fieldKey: fieldKey || undefined });
        else insertAtCursor(field, main.expr);
      },
    });
  }

  registerFieldDropTarget({
    field,
    host,
    sourceAt: (e) => canvasObj()?.outputPortNodeAtEvent?.(e) ?? null,
    accepts: (id) => {
      const g = canvasObj();
      return !!g && getUpstreamNodes(g, node.data.id).some((u) => u.id === id);
    },
    drop: (id, openList) => {
      const g = canvasObj();
      const up = g ? getUpstreamNodes(g, node.data.id).find((u) => u.id === id) : undefined;
      if (!up) return;
      const main = mainResultExpr(up);
      if (openList || !main) {
        showExpressionPicker(btn, field, node.data.id, canvasEl, { expandNodeId: id, fieldKey: fieldKey || undefined });
        return;
      }
      if (document.activeElement !== field) field.selectionStart = field.selectionEnd = field.value.length;
      insertAtCursor(field, main.expr);
    },
  });

  let suggestion: ReturnType<typeof suggestMainResult> = null;
  chip.addEventListener("click", () => {
    if (suggestion) insertAtCursor(field, suggestion.expr);
  });

  function refreshChip(): void {
    const g = graph();
    suggestion = isPrimary && g && field.value.trim() === "" ? suggestMainResult(g, node.data.id) : null;
    chip.hidden = suggestion === null;
    if (!suggestion) return;
    chip.replaceChildren(
      document.createTextNode(`Use ${suggestion.nodeName}`),
      Object.assign(document.createElement("span"), { className: "expr-picker-sep" }),
      document.createTextNode("Main result"),
    );
    chip.title = suggestion.expr;
  }

  let previewExpanded = false;
  function refreshPreview(): void {
    const g = graph();
    field.classList.toggle("field-expr", hasReference(field.value));
    preview.replaceChildren();
    preview.className = "field-expr-preview";
    const p = g ? previewField(field.value, Array.from(g.nodes.values(), (n) => n.data)) : { status: "none" as const, text: "" };
    if (p.status === "none") { preview.hidden = true; return; }
    preview.hidden = false;
    const msg = document.createElement("span");
    msg.className = "field-expr-preview-text";
    if (p.status === "ok") {
      const limit = previewExpanded ? PREVIEW_EXPANDED_MAX : PREVIEW_MAX;
      msg.textContent = p.text.length > limit ? `${p.text.slice(0, limit)}...` : p.text;
      msg.title = p.text.slice(0, PREVIEW_EXPANDED_MAX);
    } else if (p.status === "empty") {
      preview.classList.add("field-expr-preview--warn");
      msg.textContent = `Empty: ${p.nodeName} has no value for ${p.path || "its output"} yet.`;
    } else if (p.status === "unrun") {
      msg.textContent = `Run the workflow once, or use Test on ${p.nodeName}, to preview this.`;
    } else {
      msg.textContent = "Run to preview.";
    }
    const label = document.createElement("span");
    label.className = "field-expr-preview-label";
    label.textContent = p.status === "ok" ? "Preview" : "";
    if (p.status === "ok") preview.appendChild(label);
    preview.appendChild(msg);
    if (p.status === "ok" && p.text.length > PREVIEW_MAX) {
      const more = document.createElement("button");
      more.type = "button";
      more.className = "field-expr-reveal field-expr-more";
      more.textContent = previewExpanded ? "Show less" : "Show all";
      more.setAttribute("aria-expanded", String(previewExpanded));
      more.addEventListener("click", () => { previewExpanded = !previewExpanded; refreshPreview(); });
      preview.appendChild(more);
    }
    const target = p.nodeId ? g?.nodes.get(p.nodeId) : undefined;
    const canvas = g as unknown as { revealNode?: (n: unknown) => void } | undefined;
    if (target && canvas?.revealNode) {
      const reveal = document.createElement("button");
      reveal.type = "button";
      reveal.className = "field-expr-reveal";
      reveal.textContent = "Reveal node";
      reveal.addEventListener("click", () => canvas.revealNode!(target));
      preview.appendChild(reveal);
    }
  }

  const dismissKey = `${node.data.id}\u0000${fieldKey}`;
  let candidate: LiveCandidate | null = null;
  let idleTimer: ReturnType<typeof setTimeout> | undefined;

  function renderLive(): void {
    live.replaceChildren();
    live.hidden = candidate === null;
    if (!candidate) return;
    live.appendChild(document.createTextNode(`That's from ${candidate.nodeName}`));
    if (candidate.path !== "") {
      for (const seg of candidate.path.split(".")) {
        const sep = document.createElement("span");
        sep.className = "expr-picker-sep";
        sep.setAttribute("aria-hidden", "true");
        live.append(sep, document.createTextNode(seg));
      }
    }
    live.appendChild(document.createTextNode(". Make it live?"));
    const convertBtn = document.createElement("button");
    convertBtn.type = "button";
    convertBtn.className = "field-expr-live-convert";
    convertBtn.textContent = "Convert";
    convertBtn.addEventListener("click", convertToLive);
    const dismissBtn = document.createElement("button");
    dismissBtn.type = "button";
    dismissBtn.className = "field-expr-live-dismiss";
    dismissBtn.textContent = "Dismiss";
    dismissBtn.addEventListener("click", dismissLive);
    live.append(convertBtn, dismissBtn);
  }

  function refreshLive(): void {
    if (!field.isConnected) return;
    const g = graph();
    const seen = dismissedValues.get(dismissKey);
    candidate = g ? findLiveCandidate(field.value, getUpstreamNodes(g, node.data.id), seen) : null;
    renderLive();
  }

  function convertToLive(): void {
    if (!candidate) return;
    const c = candidate;
    const next = field.value.replace(c.value, () => c.expr);
    const owner = canvasObj();
    if (fieldKey && owner?.editNode) {
      owner.editNode(node, () => { (node.data.config as Record<string, unknown>)[fieldKey] = next; });
    }
    candidate = null;
    renderLive();
    field.value = next;
    field.selectionStart = field.selectionEnd = next.length;
    field.dispatchEvent(new Event("input", { bubbles: true }));
    field.focus();
  }

  function dismissLive(): void {
    if (!candidate) return;
    let seen = dismissedValues.get(dismissKey);
    if (!seen) dismissedValues.set(dismissKey, (seen = new Set()));
    if (seen.size >= SMART_PASTE_DISMISS_MAX) seen.clear();
    seen.add(candidate.value);
    candidate = null;
    renderLive();
  }

  function onFieldInput(e: Event): void {
    clearTimeout(idleTimer);
    if (candidate && field.value.trim() !== candidate.value) { candidate = null; renderLive(); }
    const input = e as InputEvent;
    if (input.data === "@") {
      const g = graph();
      const caret = field.selectionStart;
      if (g && caret !== null && isMentionStart(field.value, caret) && getUpstreamNodes(g, node.data.id).length > 0) {
        showExpressionPicker(field, field, node.data.id, canvasEl, { mention: { start: caret - 1 }, fieldKey: fieldKey || undefined });
      }
    }
    if (input.inputType === "insertFromPaste" || input.inputType === "insertFromDrop") refreshLive();
    else idleTimer = setTimeout(refreshLive, SMART_PASTE_IDLE_MS);
  }

  const refresh = () => { refreshPreview(); refreshChip(); refreshRefs(); };
  const stopRunWatch = onRunOutputsChange(() => {
    if (!field.isConnected) { stopRunWatch(); return; }
    refreshPreview();
  });
  field.addEventListener("input", refresh);
  field.addEventListener("input", onFieldInput);
  field.addEventListener("blur", () => {
    clearTimeout(idleTimer);
    disarmPointMode();
    if (!refs.contains(document.activeElement)) highlight(null);
  });
  field.addEventListener("focus", () => {
    refresh();
    if (Date.now() - mountedAt >= PROGRAMMATIC_FOCUS_MS) armPoint();
    if (!AUTO_OPEN_ON_FOCUS || Date.now() - mountedAt < PROGRAMMATIC_FOCUS_MS) return;
    const g = graph();
    if (field.value.trim() === "" && g && getUpstreamNodes(g, node.data.id).length > 0 && !isExpressionPickerOpen()) {
      showExpressionPicker(btn, field, node.data.id, canvasEl, { auto: true, fieldKey: fieldKey || undefined });
    }
  });
  refresh();
}
