/**
 * Smart paste: a field whose whole text equals one value from the last run can be turned
 * into a live reference to that value. This module only finds the match; the field UI
 * asks the user before changing anything.
 */

import { buildValueIndex, getRunOutput, valueAt } from "./data-sources";

export const SMART_PASTE_MIN_LENGTH = 4;
export const SMART_PASTE_DISMISS_MAX = 50;

export interface LiveCandidate {
  nodeId: string;
  nodeName: string;
  /** Relative to the node's output; empty when the whole output is the value. */
  path: string;
  /** The reference to insert, braces included. */
  expr: string;
  /** The text that matched, trimmed. */
  value: string;
}

/**
 * The single last-run value that `text` equals as a whole, or null. Needs at least
 * SMART_PASTE_MIN_LENGTH characters, no `{{`, a value from exactly one node of `upstream`,
 * and not a boolean. Within that node, paths through an array index (`items[0].title`) are
 * positional copies and are ignored when a named path (`latest.title`) holds the same
 * value; if more than one candidate path is left, the match is ambiguous and null.
 */
export function findLiveCandidate(
  text: string,
  upstream: Array<{ id: string; name: string }>,
  dismissed?: ReadonlySet<string>,
): LiveCandidate | null {
  const value = text.trim();
  if (value.length < SMART_PASTE_MIN_LENGTH || value.includes("{{") || dismissed?.has(value)) return null;

  const hits = buildValueIndex(upstream).filter((e) => e.display === value);
  if (hits.length === 0 || new Set(hits.map((h) => h.nodeId)).size !== 1) return null;

  const named = hits.filter((h) => !h.path.includes("["));
  const candidates = named.length > 0 ? named : hits;
  if (candidates.length !== 1) return null;
  const best = candidates[0];

  const raw = valueAt(getRunOutput(best.nodeId), best.path);
  if (typeof raw === "boolean" || String(raw) !== value) return null;
  return { nodeId: best.nodeId, nodeName: best.nodeName, path: best.path, expr: `{{${best.ref}}}`, value };
}
