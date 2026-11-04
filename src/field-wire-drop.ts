/**
 * Drag a wire from a node's result dot onto a field of the open popover.
 * Fields register as drop targets; the canvas input handler reports the
 * start, the release and a cancel. Nothing here touches the canvas model.
 */

type Field = HTMLInputElement | HTMLTextAreaElement;

export interface FieldDropTarget {
  field: Field;
  /** Element whose area counts as the drop zone (the field and the controls around it). */
  host: HTMLElement;
  /** Id of the node whose result dot is under the event, or null. */
  sourceAt: (e: MouseEvent) => string | null;
  /** True when this node's data can be used in the field. */
  accepts: (sourceId: string) => boolean;
  /** `openList` is true when Shift was held: show the field list instead of inserting. */
  drop: (sourceId: string, openList: boolean) => void;
}

const SOURCE_PORT = "output";
const TARGET_CLASS = "field-wire-target";
const HOVER_CLASS = "field-wire-target--hover";

const targets = new Set<FieldDropTarget>();
let dragSource: string | null = null;
let hovered: FieldDropTarget | null = null;

function liveTargets(): FieldDropTarget[] {
  for (const t of targets) if (!t.field.isConnected) targets.delete(t);
  return [...targets];
}

export function registerFieldDropTarget(t: FieldDropTarget): () => void {
  targets.add(t);
  return () => { targets.delete(t); };
}

/** True when a press lands on a result dot whose node can feed an open field, so the popover must stay open. */
export function fieldDropOwnsPress(e: MouseEvent): boolean {
  return liveTargets().some((t) => {
    const id = t.sourceAt(e);
    return id !== null && t.accepts(id);
  });
}

function targetAt(x: number, y: number): FieldDropTarget | null {
  if (dragSource === null || typeof document.elementFromPoint !== "function") return null;
  const el = document.elementFromPoint(x, y);
  if (!el) return null;
  const src = dragSource;
  return liveTargets().find((t) => t.host.contains(el) && t.accepts(src)) ?? null;
}

function setHover(next: FieldDropTarget | null): void {
  if (next === hovered) return;
  hovered?.host.classList.remove(HOVER_CLASS);
  next?.host.classList.add(HOVER_CLASS);
  hovered = next;
}

const onMove = (e: MouseEvent): void => setHover(targetAt(e.clientX, e.clientY));

function clear(): void {
  window.removeEventListener("mousemove", onMove, true);
  for (const t of targets) t.host.classList.remove(TARGET_CLASS, HOVER_CLASS);
  hovered = null;
  dragSource = null;
}

/** A wire drag started at `sourceId`'s port. Highlights every field that can take the node's data. */
export function beginFieldWireDrag(sourceId: string, sourcePort: string): void {
  clear();
  if (sourcePort !== SOURCE_PORT) return;
  const fit = liveTargets().filter((t) => t.accepts(sourceId));
  if (fit.length === 0) return;
  dragSource = sourceId;
  for (const t of fit) t.host.classList.add(TARGET_CLASS);
  window.addEventListener("mousemove", onMove, true);
}

export function isFieldWireDragActive(): boolean {
  return dragSource !== null;
}

/** Ends the drag. True when the release landed on a field and the data was used, so no new node or wire may be made. */
export function endFieldWireDrag(e: MouseEvent): boolean {
  const src = dragSource;
  const target = src === null ? null : targetAt(e.clientX, e.clientY);
  clear();
  if (src === null || !target) return false;
  target.drop(src, e.shiftKey);
  return true;
}

/** Esc while dragging cancels the drag only. True when a drag was active, so the popover stays open. */
export function cancelFieldWireDrag(): boolean {
  if (dragSource === null) return false;
  clear();
  return true;
}
