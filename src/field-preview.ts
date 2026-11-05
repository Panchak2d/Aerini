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
/** One function call whose first argument is a plain reference: `upper(Name.output.x)` or `join(Name.output.x, ", ")`. */
const WRAPPED_RE = /^[A-Za-z_]\w*\(\s*(.+?)\.output((?:\.[^\s.[\]()),]+|\[\d+\])*)\s*(?:,[\s\S]*)?\)$/;

interface ParsedRef {
  name: string;
  /** Text after `output`, with its leading dot. */
  rest: string;
  wrapped: boolean;
}

/** Splits `Name.output.path`, or a single function call around it, into node name and path. Null for anything else. */
function parseRef(expr: string): ParsedRef | null {
  const w = WRAPPED_RE.exec(expr);
  if (w) return { name: w[1], rest: w[2], wrapped: true };
  const s = SIMPLE_RE.exec(expr);
  return s ? { name: s[1], rest: s[2], wrapped: false } : null;
}

export function hasReference(text: string): boolean {
  return /\{\{[\s\S]*?\}\}/.test(text);
}

/** Distinct node names used by `{{Name.output...}}` references (plain or inside one function call), in order of appearance. */
export function referencedNames(text: string): string[] {
  const seen: string[] = [];
  for (const m of text.matchAll(REF_RE)) {
    const r = parseRef(m[1].trim());
    if (r && !seen.includes(r.name)) seen.push(r.name);
  }
  return seen;
}

export interface FieldReference {
  /** Offsets of the whole `{{...}}` text inside the field value. */
  start: number;
  end: number;
  nodeName: string;
  /** Output path segments after `output`, for example `["items[0]", "title"]`; empty for the whole output. */
  path: string[];
  /** True when the reference sits inside a function call; removing it removes the whole expression. */
  wrapped: boolean;
}

/** Every `{{Name.output.path}}` reference in the text (plain or inside one function call), in order, with its position. Other expressions are skipped. */
export function listReferences(text: string): FieldReference[] {
  const out: FieldReference[] = [];
  for (const m of text.matchAll(REF_RE)) {
    const r = parseRef(m[1].trim());
    if (!r) continue;
    const start = m.index ?? 0;
    const rel = r.rest.startsWith(".") ? r.rest.slice(1) : r.rest;
    out.push({ start, end: start + m[0].length, nodeName: r.name, path: rel === "" ? [] : rel.split("."), wrapped: r.wrapped });
  }
  return out;
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
    const s = WRAPPED_RE.test(expr) ? null : SIMPLE_RE.exec(expr);
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
