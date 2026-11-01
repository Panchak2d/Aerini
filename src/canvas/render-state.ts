let dirty = true;

/** Requests a repaint on the next animation frame. Safe to call from anywhere, any number of times. */
export function markDirty(): void {
  dirty = true;
}

/** Returns whether a repaint was requested since the last call, and clears the request. */
export function consumeDirty(): boolean {
  const was = dirty;
  dirty = false;
  return was;
}

const watched = new WeakSet<object>();

/**
 * Makes set/delete/clear on `map` request a repaint. Patches the instance in
 * place, so callers holding a reference to the map keep working.
 */
export function watchMap<K, V>(map: Map<K, V>): Map<K, V> {
  if (watched.has(map)) return map;
  watched.add(map);
  const set = map.set, del = map.delete, clear = map.clear;
  map.set = function (this: Map<K, V>, k: K, v: V) { markDirty(); return set.call(this, k, v); };
  map.delete = function (this: Map<K, V>, k: K) { markDirty(); return del.call(this, k); };
  map.clear = function (this: Map<K, V>) { markDirty(); clear.call(this); };
  return map;
}

/** Window events after which something visible may have changed: any user input, focus move or layout-affecting action. */
const REPAINT_EVENTS = ["pointerdown", "pointerup", "mousedown", "mouseup", "click", "dblclick", "keydown", "keyup", "input", "change", "wheel", "focusin", "focusout", "drop", "resize", "transitionend", "animationend"] as const;

let netInstalled = false;

/**
 * Backstop for edits made through paths that never touch the canvas objects
 * directly (popover fields, panels): every such edit starts from a user event
 * or finishes in one, so each of those events requests a repaint.
 */
export function installRepaintNet(target: Window = window): void {
  if (netInstalled) return;
  netInstalled = true;
  for (const type of REPAINT_EVENTS) target.addEventListener(type, markDirty, { capture: true, passive: true });
  target.document.addEventListener("visibilitychange", markDirty);
}
