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
import { getUpstreamNodes, mainResultExpr, sourceColorIndex, suggestMainResult, type SourceGraph } from "../data-sources";
import { hasReference, previewField, referencedNames } from "../field-preview";
import { armPointMode, disarmPointMode } from "../point-mode";

/** Set to false to stop the picker opening by itself when an empty field is focused. */
export const AUTO_OPEN_ON_FOCUS = true;

/** Focus that arrives this soon after the form is built is the popover's own focus move, not the user's. */
const PROGRAMMATIC_FOCUS_MS = 300;
const PREVIEW_MAX = 160;

const ICON_ATTRS = `width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"`;
const PLUS_SVG = `<svg ${ICON_ATTRS}><rect x="3" y="3" width="18" height="18" rx="3"/><line x1="12" y1="8" x2="12" y2="16"/><line x1="8" y1="12" x2="16" y2="12"/></svg>`;

type Field = HTMLInputElement | HTMLTextAreaElement;

interface CanvasLike extends SourceGraph {
  nodeIdAtEvent?: (e: MouseEvent) => string | null;
  setRefHighlights?: (sources: Map<string, number> | null, targetNodeId: string | null) => void;
  revealNode?: (n: unknown) => void;
}

export function mountExpressionUi(
  field: Field,
  host: HTMLElement,
  node: CanvasNode,
  canvasEl: HTMLCanvasElement,
  isPrimary = false,
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
    showExpressionPicker(btn, field, node.data.id, canvasEl);
  });
  bar.appendChild(btn);

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

  const refs = document.createElement("div");
  refs.className = "field-expr-refs";
  refs.hidden = true;
  host.appendChild(refs);

  const canvasObj = () => graph() as CanvasLike | undefined;
  /** Source nodes this field references, by id, with their color slot. */
  function referenced(): Map<string, number> {
    const g = canvasObj();
    const out = new Map<string, number>();
    if (!g) return out;
    const byName = new Map<string, string>();
    for (const n of g.nodes.values()) if (!byName.has(n.data.name)) byName.set(n.data.name, n.data.id);
    for (const name of referencedNames(field.value)) {
      const id = byName.get(name);
      if (id !== undefined && id !== node.data.id) out.set(id, sourceColorIndex(id));
    }
    return out;
  }
  const highlight = (m: Map<string, number> | null) => canvasObj()?.setRefHighlights?.(m, node.data.id);

  function refreshRefs(): void {
    const g = canvasObj();
    const map = referenced();
    refs.replaceChildren();
    refs.hidden = map.size === 0;
    for (const [id, slot] of map) {
      const target = g?.nodes.get(id);
      if (!target) continue;
      const pill = document.createElement("button");
      pill.type = "button";
      pill.className = "ref-pill";
      pill.style.setProperty("--ref", `var(--ref-color-${slot})`);
      pill.textContent = target.data.name;
      pill.title = `Uses data from ${target.data.name}. Click to find it on the canvas.`;
      pill.addEventListener("mouseenter", () => highlight(new Map([[id, slot]])));
      pill.addEventListener("focus", () => highlight(new Map([[id, slot]])));
      pill.addEventListener("mouseleave", () => highlight(document.activeElement === field ? referenced() : null));
      pill.addEventListener("blur", () => highlight(document.activeElement === field ? referenced() : null));
      pill.addEventListener("click", () => g?.revealNode?.(target));
      refs.appendChild(pill);
    }
    if (document.activeElement === field) highlight(map);
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
        if (again || !main) showExpressionPicker(btn, field, node.data.id, canvasEl, { expandNodeId: id });
        else insertAtCursor(field, main.expr);
      },
    });
  }

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
      msg.textContent = p.text.length > PREVIEW_MAX ? `${p.text.slice(0, PREVIEW_MAX)}...` : p.text;
      msg.title = p.text;
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

  const refresh = () => { refreshPreview(); refreshChip(); refreshRefs(); };
  field.addEventListener("input", refresh);
  field.addEventListener("blur", () => {
    disarmPointMode();
    if (!refs.contains(document.activeElement)) highlight(null);
  });
  field.addEventListener("focus", () => {
    refresh();
    if (Date.now() - mountedAt >= PROGRAMMATIC_FOCUS_MS) armPoint();
    if (!AUTO_OPEN_ON_FOCUS || Date.now() - mountedAt < PROGRAMMATIC_FOCUS_MS) return;
    const g = graph();
    if (field.value.trim() === "" && g && getUpstreamNodes(g, node.data.id).length > 0 && !isExpressionPickerOpen()) {
      showExpressionPicker(btn, field, node.data.id, canvasEl, { auto: true });
    }
  });
  refresh();
}
