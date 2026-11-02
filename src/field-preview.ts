/**
 * Client-side preview of a field's text from the last run. Resolves only plain
 * `{{Node Name.output.path}}` references; anything else (functions, `$run`,
 * nested braces) is reported as unsupported rather than re-implementing the engine.
 */

import { getRunOutput, valueAt } from "./data-sources";

export type PreviewStatus = "none" | "ok" | "empty" | "unrun" | "unsupported";

export interface FieldPreview {
  status: PreviewStatus;
  /** Resolved text for `ok`; empty otherwise. */
  text: string;
  /** Node the status refers to, for `empty` and `unrun`. */
  nodeName?: string;
  /** Path the status refers to, for `empty`. */
  path?: string;
  /** Id of the first referenced node, for Reveal. */
  nodeId?: string;
}

const REF_RE = /\{\{([\s\S]*?)\}\}/g;
const SIMPLE_RE = /^(.+?)\.output((?:\.(?:[^\s.[\]]+)|\[\d+\])*)$/;

export function hasReference(text: string): boolean {
  return /\{\{[\s\S]*?\}\}/.test(text);
}

/** Distinct node names used as the first segment of plain `{{Name.output...}}` references, in order of appearance. */
export function referencedNames(text: string): string[] {
  const seen: string[] = [];
  for (const m of text.matchAll(REF_RE)) {
    const s = SIMPLE_RE.exec(m[1].trim());
    if (s && !seen.includes(s[1])) seen.push(s[1]);
  }
  return seen;
}

export function previewField(
  text: string,
  nodes: Iterable<{ id: string; name: string }>,
): FieldPreview {
  if (!hasReference(text)) return { status: "none", text: "" };

  const byName = new Map<string, string>();
  for (const n of nodes) if (!byName.has(n.name)) byName.set(n.name, n.id);

  let out = "";
  let last = 0;
  let first: { id: string } | undefined;
  for (const m of text.matchAll(REF_RE)) {
    out += text.slice(last, m.index);
    last = (m.index ?? 0) + m[0].length;
    const expr = m[1].trim();
    const s = SIMPLE_RE.exec(expr);
    const id = s ? byName.get(s[1]) : undefined;
    if (!s || id === undefined) return { status: "unsupported", text: "" };
    first ??= { id };
    const raw = getRunOutput(id);
    if (raw === undefined) return { status: "unrun", text: "", nodeName: s[1], nodeId: first.id };
    const rel = s[2].startsWith(".") ? s[2].slice(1) : s[2];
    const v = valueAt(raw, rel);
    if (v === undefined || v === null || v === "") {
      return { status: "empty", text: "", nodeName: s[1], path: rel, nodeId: first.id };
    }
    out += typeof v === "string" ? v : JSON.stringify(v);
  }
  out += text.slice(last);
  return { status: "ok", text: out, nodeId: first?.id };
}
