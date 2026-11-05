/**
 * Point mode: while a text field is focused, clicking an upstream node on the
 * canvas inserts that node's data into the field. This module owns only the
 * armed state; the canvas, the popover and the field each call into it.
 */

import { markDirty } from "./canvas/render-state";

export interface PointModeConfig {
  /** Nodes that can be picked. Everything else stays inert. */
  eligible: ReadonlySet<string>;
  /** Id of the node under the mouse event, or null. */
  hit: (e: MouseEvent) => string | null;
  /** `again` is true when the same node is picked twice in a row. */
  onPick: (nodeId: string, again: boolean) => void;
  /** Inserts a reference chosen elsewhere (a value in the data tree) into the focused field. */
  insert: (expr: string) => void;
}

export const POINT_MODE_HINT = "Pick mode: click a node to use its result · Esc to cancel";

let cfg: PointModeConfig | null = null;
let lastPicked: string | null = null;
let savedHint: { text: string; title: string } | null = null;

function hintEl(): HTMLElement | null {
  return typeof document === "undefined" ? null : document.getElementById("status-hint");
}

export function armPointMode(c: PointModeConfig): void {
  if (c.eligible.size === 0) { disarmPointMode(); return; }
  const wasArmed = cfg !== null;
  cfg = c;
  if (!wasArmed) lastPicked = null;
  const el = hintEl();
  if (el) {
    if (!savedHint) savedHint = { text: el.textContent ?? "", title: el.title };
    el.textContent = POINT_MODE_HINT;
    el.title = POINT_MODE_HINT;
  }
  markDirty();
}

export function disarmPointMode(): void {
  if (!cfg) return;
  cfg = null;
  lastPicked = null;
  const el = hintEl();
  if (el && savedHint) { el.textContent = savedHint.text; el.title = savedHint.title; }
  savedHint = null;
  markDirty();
}

/** While armed, the status line shows the pick-mode text; a caller with a new normal hint hands it here and it is shown on disarm. Returns true when it was taken. */
export function deferStatusHint(text: string): boolean {
  if (!cfg) return false;
  savedHint = { text, title: text };
  return true;
}

export function isPointModeArmed(): boolean {
  return cfg !== null;
}

export function pointEligibleIds(): ReadonlySet<string> | null {
  return cfg ? cfg.eligible : null;
}

function pickedAt(e: MouseEvent): string | null {
  if (!cfg) return null;
  const id = cfg.hit(e);
  return id !== null && cfg.eligible.has(id) ? id : null;
}

function leafNodeId(e: MouseEvent): string | null {
  const t = e.target;
  if (!cfg || !(t instanceof Element)) return null;
  const id = t.closest("[data-json-node]")?.getAttribute("data-json-node") ?? null;
  return id !== null && cfg.eligible.has(id) ? id : null;
}

/** True when this mouse event lands on a pickable node or a pickable data value, so the popover must not treat it as an outside click. */
export function pointModeOwns(e: MouseEvent): boolean {
  return pickedAt(e) !== null || leafNodeId(e) !== null;
}

/** Inserts `expr`, a reference into `nodeId`'s output, into the focused field. False when no field is waiting or the node is not upstream of it. */
export function insertFromTree(nodeId: string, expr: string): boolean {
  if (!cfg || !cfg.eligible.has(nodeId)) return false;
  cfg.insert(expr);
  markDirty();
  return true;
}

/** Handles a canvas mousedown. Returns true when the click was a pick and normal canvas handling must stop. */
export function consumePointClick(e: MouseEvent): boolean {
  const id = pickedAt(e);
  if (id === null || !cfg) return false;
  const again = id === lastPicked;
  lastPicked = id;
  cfg.onPick(id, again);
  markDirty();
  return true;
}

/** Esc while armed leaves point mode only. Returns true when it did, so the popover stays open. */
export function exitPointModeOnEscape(): boolean {
  if (!cfg) return false;
  disarmPointMode();
  return true;
}
