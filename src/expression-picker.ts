/**
 * Data picker: floating panel with a Data tab (upstream nodes, their fields and
 * sample values from the last run) and a Functions tab. Choosing an item inserts
 * a `{{...}}` reference or function at the caret of the target field.
 *
 *   showExpressionPicker(anchorEl, targetInput, currentNodeId, canvasEl, opts?)
 *   closeExpressionPicker()
 */

import {
  buildValueIndex,
  formatSample,
  getRunOutput,
  getUpstreamNodes,
  listPaths,
  primaryOutputKey,
  valueAt,
  type SourceGraph,
} from "./data-sources";
import { EXPRESSION_FUNCTIONS } from "./expression-functions";
import { getIconSvg } from "./icon-cache";
import { mentionQuery } from "./mention";
import type { ConnectCandidate } from "./connect-insert";

type Field = HTMLInputElement | HTMLTextAreaElement;
type Tab = "data" | "functions";

export interface PickerOptions {
  /** Opened because the field gained focus: keep focus in the field and close on typing. */
  auto?: boolean;
  /** Node to show expanded first (used when a canvas node is picked twice). */
  expandNodeId?: string;
  /**
   * Opened by typing `@` at `start` in the field: the text after it is the search,
   * focus stays in the field, and picking replaces `@` and that text.
   */
  mention?: { start: number };
  /**
   * Config key of the field being edited. Needed to offer nodes that are not
   * connected yet: choosing one wires it in and writes this key as one undo step.
   */
  fieldKey?: string;
}

interface ConnectingCanvas extends SourceGraph {
  connectableTo?: (nodeId: string) => ConnectCandidate[];
  connectAndInsert?: (srcId: string, targetId: string, apply: () => void) => boolean;
}

let _pickerEl: HTMLElement | null = null;
let _pickerId = 0;

const VALUE_MATCH_MAX = 20;

const SVG_ATTRS = `width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"`;
const CHEVRON_SVG = `<svg ${SVG_ATTRS} class="expr-picker-chev"><polyline points="9 6 15 12 9 18"/></svg>`;
const CHECK_SVG = `<svg ${SVG_ATTRS}><polyline points="5 13 10 18 19 7"/></svg>`;

export function closeExpressionPicker(): void {
  if (_pickerEl) {
    _pickerEl.remove();
    _pickerEl = null;
  }
}

export function isExpressionPickerOpen(): boolean {
  return _pickerEl !== null;
}

export function showExpressionPicker(
  anchorEl: HTMLElement,
  targetInput: Field,
  currentNodeId: string,
  canvasEl: HTMLCanvasElement,
  opts: PickerOptions = {},
): void {
  closeExpressionPicker();
  const myId = ++_pickerId;

  const canvas = (canvasEl as unknown as { __canvas: ConnectingCanvas }).__canvas;
  if (!canvas) return;

  const upstream = getUpstreamNodes(canvas, currentNodeId);
  const expanded = new Set<string>(
    opts.expandNodeId && upstream.some((u) => u.id === opts.expandNodeId) ? [opts.expandNodeId] : upstream[0] ? [upstream[0].id] : [],
  );
  const allFields = new Set<string>();
  const connectable: ConnectCandidate[] =
    opts.fieldKey && canvas.connectableTo && canvas.connectAndInsert ? canvas.connectableTo(currentNodeId) : [];
  const mention = opts.mention ?? null;
  const initialQuery = mention ? mentionQuery(targetInput.value, mention.start, targetInput.selectionStart ?? targetInput.value.length) : "";
  if (initialQuery === null) return;
  let tab: Tab = "data";
  let query = initialQuery.trim().toLowerCase();
  let active = 0;

  const picker = document.createElement("div");
  picker.className = "expr-picker";
  picker.setAttribute("role", "dialog");
  picker.setAttribute("aria-label", "Insert data");

  const tabs = document.createElement("div");
  tabs.className = "expr-picker-tabs";
  tabs.setAttribute("role", "tablist");
  const tabBtns = new Map<Tab, HTMLButtonElement>();
  for (const [id, label] of [["data", "Data"], ["functions", "Functions"]] as const) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "expr-picker-tab";
    b.setAttribute("role", "tab");
    b.textContent = label;
    b.addEventListener("mousedown", (e) => e.preventDefault());
    b.addEventListener("click", () => { tab = id; render(); });
    tabs.appendChild(b);
    tabBtns.set(id, b);
  }
  tabs.hidden = mention !== null;
  picker.appendChild(tabs);

  const search = document.createElement("input");
  search.type = "search";
  search.className = "expr-picker-search";
  search.placeholder = "Search";
  search.setAttribute("aria-label", "Search data and functions");
  search.autocomplete = "off";
  search.spellcheck = false;
  search.addEventListener("input", () => { query = search.value.trim().toLowerCase(); renderBody(); });
  search.hidden = mention !== null;
  picker.appendChild(search);

  const body = document.createElement("div");
  body.className = "expr-picker-body";
  picker.appendChild(body);

  const pick = (text: string) => {
    if (mention) targetInput.setSelectionRange(mention.start, targetInput.selectionStart ?? targetInput.value.length);
    insertAtCursor(targetInput, text);
    closeExpressionPicker();
  };
  const connectAndPick = (c: ConnectCandidate) => {
    const target = canvas.nodes.get(currentNodeId)?.data as { config?: Record<string, unknown> } | undefined;
    const key = opts.fieldKey;
    if (!target?.config || !key || !canvas.connectAndInsert) { closeExpressionPicker(); return; }
    if (mention) targetInput.setSelectionRange(mention.start, targetInput.selectionStart ?? targetInput.value.length);
    const { next, caret } = spliceAtSelection(targetInput, c.expr);
    const config = target.config;
    const done = canvas.connectAndInsert(c.id, currentNodeId, () => { config[key] = next; });
    closeExpressionPicker();
    if (!done) return;
    targetInput.value = next;
    targetInput.selectionStart = targetInput.selectionEnd = caret;
    targetInput.dispatchEvent(new Event("input", { bubbles: true }));
    targetInput.focus();
  };
  const matches = (...parts: string[]) => query === "" || parts.some((p) => p.toLowerCase().includes(query));

  function render(): void {
    for (const [id, b] of tabBtns) {
      b.setAttribute("aria-selected", String(id === tab));
      b.classList.toggle("expr-picker-tab--on", id === tab);
    }
    renderBody();
  }

  function renderBody(): void {
    body.replaceChildren();
    if (tab === "functions") renderFunctions(); else renderData();
    if (mention) markActive();
  }

  function markActive(): void {
    const items = [...body.querySelectorAll<HTMLElement>(".expr-picker-item")];
    active = Math.max(0, Math.min(active, items.length - 1));
    items.forEach((el, i) => el.classList.toggle("expr-picker-item--active", i === active));
    items[active]?.scrollIntoView?.({ block: "nearest" });
  }

  function renderFunctions(): void {
    let shown = 0;
    for (const f of EXPRESSION_FUNCTIONS) {
      if (!matches(f.sig, f.desc)) continue;
      shown++;
      const d = mkRow("expr-picker-item expr-picker-item--fn", () => {
        insertFunction(targetInput, f.template);
        closeExpressionPicker();
      });
      d.appendChild(mkSpan("expr-picker-expr", f.sig));
      d.appendChild(mkSpan("expr-picker-label", f.desc));
      body.appendChild(d);
    }
    if (shown === 0) body.appendChild(mkHint("No functions match."));
  }

  function renderData(): void {
    if (upstream.length === 0) {
      body.appendChild(mkHint(connectable.length > 0
        ? "No node feeds this one yet. Pick one below to connect it and use its data."
        : "Connect a node to this one first, then its data appears here."));
    }
    const shown = new Set<string>();
    for (const up of upstream) {
      const group = document.createElement("div");
      group.className = "expr-picker-group";
      const open = query !== "" || expanded.has(up.id);
      const sample = getRunOutput(up.id);
      const hasRun = sample !== undefined;
      const primary = primaryOutputKey(up.schema);
      const paths = listPaths(up.schema, sample);

      const visible = paths.filter((p) => p.path !== primary && matches(up.name, p.path, p.description, scalarText(valueAt(sample, p.path))));
      const showPrimary = primary !== null && matches(up.name, "main result", primary, scalarText(valueAt(sample, primary)));
      if (query !== "" && !showPrimary && visible.length === 0 && !matches(up.name)) continue;

      const head = document.createElement("button");
      head.type = "button";
      head.className = "expr-picker-node";
      head.setAttribute("aria-expanded", String(open));
      head.addEventListener("mousedown", (e) => e.preventDefault());
      head.addEventListener("click", () => {
        if (expanded.has(up.id)) expanded.delete(up.id); else expanded.add(up.id);
        renderBody();
      });
      const typeId = (canvas.nodes.get(up.id)?.data as { node_type_id?: string } | undefined)?.node_type_id;
      const icon = mkSpan("expr-picker-node-icon", "");
      if (typeId) icon.innerHTML = getIconSvg(typeId);
      head.appendChild(icon);
      head.appendChild(mkSpan("expr-picker-node-name", up.name));
      if (hasRun) {
        const ran = mkSpan("expr-picker-node-ran", "");
        ran.innerHTML = CHECK_SVG;
        ran.title = "Has run";
        head.appendChild(ran);
      }
      const chev = mkSpan("expr-picker-node-chev", "");
      chev.innerHTML = CHEVRON_SVG;
      head.appendChild(chev);
      group.appendChild(head);

      if (open) {
        const content = document.createElement("div");
        content.className = "expr-picker-node-body";
        if (!hasRun) {
          content.appendChild(mkHint(paths.length > 0 || primary !== null
            ? `Run once to see values: run the workflow, or use Test on ${up.name}.`
            : `Run the workflow once, or use Test on ${up.name}, to see its data here.`));
        }
        if (showPrimary && primary !== null) {
          const expr = `{{${up.name}.output.${primary}}}`;
          const desc = paths.find((p) => p.path === primary)?.description ?? "";
          shown.add(expr);
          content.appendChild(mkDataItem(expr, "Main result", formatSample(valueAt(sample, primary)), desc, () => pick(expr), true));
        }
        const collapse = showPrimary && query === "" && visible.length > 0 && !allFields.has(up.id);
        if (!collapse) {
          for (const p of visible) {
            const expr = `{{${up.name}.output.${p.path}}}`;
            shown.add(expr);
            content.appendChild(mkDataItem(expr, p.path, formatSample(valueAt(sample, p.path)), p.description, () => pick(expr), false));
          }
        }
        if (showPrimary && query === "" && visible.length > 0) {
          const more = mkRow("expr-picker-more", () => {
            if (allFields.has(up.id)) allFields.delete(up.id); else allFields.add(up.id);
            renderBody();
          });
          more.textContent = allFields.has(up.id) ? "Show fewer fields" : `Show all fields (${visible.length})`;
          content.appendChild(more);
        }
        if (paths.length === 0 && primary === null && hasRun) {
          content.appendChild(mkHint(`${up.name} returned no named fields. Use Entire output below.`));
        }
        if (query === "") {
          const whole = `{{${up.name}.output}}`;
          const w = mkRow("expr-picker-item expr-picker-item--whole", () => pick(whole));
          w.dataset.expr = whole;
          w.title = whole;
          w.appendChild(mkSpan("expr-picker-label", "Entire output (JSON)"));
          content.appendChild(w);
        }
        group.appendChild(content);
      }
      body.appendChild(group);
    }

    if (query !== "") appendValueMatches(shown);
    appendConnectable();

    if (query === "" || matches("run variables", "$run")) {
      body.appendChild(mkSection("Run variables"));
      for (const v of [
        { expr: "$run.id", desc: "Execution ID" },
        { expr: "$run.timestamp", desc: "ISO timestamp of this run" },
        { expr: "$run.workflow_name", desc: "Workflow name" },
      ]) {
        if (!matches(v.expr, v.desc)) continue;
        body.appendChild(mkDataItem(`{{${v.expr}}}`, v.expr, "", v.desc, () => pick(`{{${v.expr}}}`), false));
      }
    }
  }

  /** Canvas nodes that are not wired to this one: choosing one connects it and inserts its main result. */
  function appendConnectable(): void {
    const rows = connectable.filter((c) => matches(c.name));
    if (rows.length === 0) return;
    body.appendChild(mkSection("Not connected"));
    for (const c of rows) {
      const row = mkRow("expr-picker-item expr-picker-item--unconnected", () => connectAndPick(c));
      row.dataset.expr = c.expr;
      row.dataset.connectNodeId = c.id;
      row.title = `Connect ${c.name} to this node and use ${c.expr}`;
      const typeId = (canvas.nodes.get(c.id)?.data as { node_type_id?: string } | undefined)?.node_type_id;
      const icon = mkSpan("expr-picker-node-icon", "");
      if (typeId) icon.innerHTML = getIconSvg(typeId);
      row.appendChild(icon);
      row.appendChild(mkSpan("expr-picker-name", c.name));
      row.appendChild(mkSpan("expr-picker-label", "Connect and use"));
      body.appendChild(row);
    }
  }

  /** Last-run values or deep paths that match the search and are not already listed above. */
  function appendValueMatches(shown: Set<string>): void {
    const rows = buildValueIndex(upstream)
      .filter((e) => !shown.has(`{{${e.ref}}}`) && (e.display.toLowerCase().includes(query) || e.path.toLowerCase().includes(query)))
      .slice(0, VALUE_MATCH_MAX);
    if (rows.length === 0) return;
    body.appendChild(mkSection("Matching values"));
    for (const e of rows) {
      const expr = `{{${e.ref}}}`;
      body.appendChild(mkDataItem(expr, e.path === "" ? e.nodeName : `${e.nodeName}.${e.path}`, formatSample(e.display), "", () => pick(expr), false));
    }
  }

  render();
  document.body.appendChild(picker);
  _pickerEl = picker;
  positionPicker(picker, anchorEl);
  if (!opts.auto && !mention) search.focus({ preventScroll: true });

  const cleanups: Array<() => void> = [];
  const stillMine = () => myId === _pickerId;
  const detach = () => { for (const c of cleanups.splice(0)) c(); };

  const onOutside = (e: MouseEvent) => {
    if (!stillMine()) { detach(); return; }
    const t = e.target as Node;
    if (!picker.contains(t) && t !== anchorEl && !anchorEl.contains(t) && !((opts.auto || mention) && t === targetInput)) {
      detach();
      closeExpressionPicker();
    }
  };
  const armTimer = setTimeout(() => {
    if (!stillMine()) return;
    document.addEventListener("mousedown", onOutside, true);
    cleanups.push(() => document.removeEventListener("mousedown", onOutside, true));
  }, 80);
  cleanups.push(() => clearTimeout(armTimer));

  const onKey = (e: KeyboardEvent) => {
    if (!stillMine()) { detach(); return; }
    if (mention && e.target === targetInput && !e.isComposing && !e.shiftKey && (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter" || e.key === "Tab")) {
      const items = [...body.querySelectorAll<HTMLElement>(".expr-picker-item")];
      if (items.length === 0) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Enter" || e.key === "Tab") {
        items[active]?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
      } else {
        active += e.key === "ArrowDown" ? 1 : -1;
        markActive();
      }
      return;
    }
    if (e.key === "Escape") {
      detach();
      closeExpressionPicker();
      targetInput.focus();
    } else if ((e.key === "ArrowDown" || e.key === "ArrowUp") && picker.contains(document.activeElement)) {
      const items = [...picker.querySelectorAll<HTMLElement>('input, [tabindex="0"]')];
      const i = items.indexOf(document.activeElement as HTMLElement);
      const next = items[i + (e.key === "ArrowDown" ? 1 : -1)];
      if (next) { e.preventDefault(); next.focus(); }
    }
  };
  document.addEventListener("keydown", onKey, true);
  cleanups.push(() => document.removeEventListener("keydown", onKey, true));

  if (mention) {
    const sync = () => {
      if (!stillMine()) { detach(); return; }
      const caret = targetInput.selectionStart ?? targetInput.value.length;
      const q = mentionQuery(targetInput.value, mention.start, caret);
      if (q === null) { detach(); closeExpressionPicker(); return; }
      const next = q.trim().toLowerCase();
      if (next === query) return;
      query = next;
      active = 0;
      renderBody();
      if (query !== "" && !body.querySelector(".expr-picker-item")) { detach(); closeExpressionPicker(); }
    };
    const onBlur = (e: Event) => {
      if (stillMine() && !picker.contains((e as FocusEvent).relatedTarget as Node | null)) { detach(); closeExpressionPicker(); }
    };
    targetInput.addEventListener("input", sync);
    targetInput.addEventListener("keyup", sync);
    targetInput.addEventListener("click", sync);
    targetInput.addEventListener("focusout", onBlur);
    cleanups.push(() => {
      targetInput.removeEventListener("input", sync);
      targetInput.removeEventListener("keyup", sync);
      targetInput.removeEventListener("click", sync);
      targetInput.removeEventListener("focusout", onBlur);
    });
  }

  if (opts.auto) {
    const onType = () => { if (stillMine()) { detach(); closeExpressionPicker(); } };
    const onBlur = (e: Event) => {
      if (stillMine() && !picker.contains((e as FocusEvent).relatedTarget as Node | null)) { detach(); closeExpressionPicker(); }
    };
    targetInput.addEventListener("input", onType, { once: true });
    targetInput.addEventListener("focusout", onBlur);
    cleanups.push(() => {
      targetInput.removeEventListener("input", onType);
      targetInput.removeEventListener("focusout", onBlur);
    });
  }
}

/** Insert `text` at the caret of the field and notify the field's input handler. */
export function insertAtCursor(el: Field, text: string): void {
  const { next, caret } = spliceAtSelection(el, text);
  el.value = next;
  el.selectionStart = el.selectionEnd = caret;
  el.dispatchEvent(new Event("input", { bubbles: true }));
  el.focus();
}

/** The field's value with `text` replacing the selection, and where the caret lands. */
function spliceAtSelection(el: Field, text: string): { next: string; caret: number } {
  const start = el.selectionStart ?? el.value.length;
  const end = el.selectionEnd ?? el.value.length;
  return { next: el.value.slice(0, start) + text + el.value.slice(end), caret: start + text.length };
}

/** Insert a function template and place the caret at its `|` marker. */
function insertFunction(el: Field, template: string): void {
  const marker = template.indexOf("|");
  const text = marker >= 0 ? template.slice(0, marker) + template.slice(marker + 1) : template;
  const start = el.selectionStart ?? el.value.length;
  const end = el.selectionEnd ?? el.value.length;
  el.value = el.value.slice(0, start) + text + el.value.slice(end);
  el.selectionStart = el.selectionEnd = start + (marker >= 0 ? marker : text.length);
  el.dispatchEvent(new Event("input", { bubbles: true }));
  el.focus();
}

function positionPicker(picker: HTMLElement, anchor: HTMLElement): void {
  const r = anchor.getBoundingClientRect();
  const W = 320;
  const H = 380;
  const M = 8;
  let left = r.left;
  let top = r.bottom + 4;
  if (left + W > window.innerWidth - M) left = window.innerWidth - W - M;
  if (left < M) left = M;
  if (top + H > window.innerHeight - M) top = r.top - H - 4;
  if (top < M) top = M;
  picker.style.left = `${left}px`;
  picker.style.top = `${top}px`;
}

/** Text of a string, number or boolean sample; objects and arrays are found through their own fields instead. */
function scalarText(v: unknown): string {
  return typeof v === "string" || typeof v === "number" || typeof v === "boolean" ? formatSample(v) : "";
}

function mkSpan(cls: string, text: string): HTMLSpanElement {
  const s = document.createElement("span");
  s.className = cls;
  if (text) s.textContent = text;
  return s;
}

function mkSection(title: string): HTMLElement {
  const d = document.createElement("div");
  d.className = "expr-picker-section";
  d.textContent = title;
  return d;
}

function mkHint(text: string): HTMLElement {
  const d = document.createElement("div");
  d.className = "expr-picker-hint";
  d.textContent = text;
  return d;
}

function mkRow(cls: string, onClick: () => void): HTMLElement {
  const d = document.createElement("div");
  d.className = cls;
  d.setAttribute("role", "button");
  d.setAttribute("tabindex", "0");
  d.addEventListener("mousedown", (e) => { e.preventDefault(); onClick(); });
  d.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onClick(); }
  });
  return d;
}

/** A pickable data item: readable name (path segments joined by a drawn chevron), sample, description. */
function mkDataItem(
  expr: string, name: string, sample: string, desc: string, onClick: () => void, pinned: boolean,
): HTMLElement {
  const d = mkRow(`expr-picker-item${pinned ? " expr-picker-item--main" : ""}`, onClick);
  d.dataset.expr = expr;
  d.title = expr;
  const label = mkSpan("expr-picker-name", "");
  const segs = name.startsWith("$") ? [name] : name.split(".");
  segs.forEach((seg, i) => {
    if (i > 0) { const sep = mkSpan("expr-picker-sep", ""); sep.setAttribute("aria-hidden", "true"); label.appendChild(sep); }
    label.appendChild(document.createTextNode(seg));
  });
  d.appendChild(label);
  if (sample) d.appendChild(mkSpan("expr-picker-sample", sample));
  if (desc) d.appendChild(mkSpan("expr-picker-label", desc));
  return d;
}
