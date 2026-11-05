/**
 * Problems found without running anything (and warnings read back from a run's
 * log): broken `{{Node.output.field}}` references, ambiguous or unusable node
 * names, blank required fields. Pure: no DOM, no canvas import.
 *
 * Reference problems are warnings and never block a run; blank required
 * fields are errors. A reference problem may carry a one-click fix that
 * rewrites the reference text; nothing is ever applied automatically.
 */

import { closestPath, getRunOutput, getUpstreamNodes, listPaths } from "./data-sources";
import type { SourceGraph } from "./data-sources";
import { validateNodeName } from "./node-names";

export type ProblemSeverity = "error" | "warning";
export type ProblemKind =
  | "required"
  | "unknown-node"
  | "not-upstream"
  | "self-reference"
  | "bad-segment"
  | "missing-field"
  | "duplicate-name"
  | "bad-name"
  | "empty-field"
  | "no-output";

export interface ProblemFix {
  label: string;
  /** Exact reference text as stored, including braces. */
  before: string;
  after: string;
}

export interface Problem {
  /** Stable across refreshes while the underlying issue is unchanged. */
  key: string;
  nodeId: string;
  nodeName: string;
  severity: ProblemSeverity;
  kind: ProblemKind;
  /** Top-level config key the problem sits in, when there is one. */
  field?: string;
  message: string;
  fix?: ProblemFix;
}

export interface ProblemNode {
  disabled?: boolean;
  data: {
    id: string;
    name: string;
    node_type_id: string;
    config: Record<string, unknown>;
    output_schema?: Record<string, unknown>;
  };
}

export interface ProblemGraph {
  nodes: Map<string, ProblemNode>;
  connectors: SourceGraph["connectors"];
}

const REF_RE = /\{\{([\s\S]*?)\}\}/g;
const MAX_PROBLEMS = 200;

function isObj(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function walkStrings(value: unknown, visit: (s: string) => void, depth = 0): void {
  if (depth > 8) return;
  if (typeof value === "string") visit(value);
  else if (Array.isArray(value)) for (const v of value) walkStrings(v, visit, depth + 1);
  else if (isObj(value)) for (const v of Object.values(value)) walkStrings(v, visit, depth + 1);
}

/** Function calls, `$run`/`$vars`/`$env` and literals are not node references. */
function isNodeReference(expr: string): boolean {
  if (!expr || /^["'\d$]/.test(expr)) return false;
  const paren = expr.indexOf("(");
  if (paren > 0 && /^[A-Za-z0-9_]+$/.test(expr.slice(0, paren))) return false;
  return expr.includes(".");
}

function topLevelKey(path: string): string {
  return path.replace(/\[.*$/, "");
}

function schemaKeys(schema: Record<string, unknown> | undefined): string[] {
  return schema && isObj(schema.properties) ? Object.keys(schema.properties) : [];
}

/** Top-level field names a node's output is known to have, from schema and the last run. */
function knownTopLevel(nodeId: string, schema: Record<string, unknown> | undefined): string[] {
  const out = new Set<string>(schemaKeys(schema));
  for (const p of listPaths(schema, getRunOutput(nodeId))) out.add(topLevelKey(p.path));
  return [...out];
}

function replaceFirst(text: string, find: string, repl: string): string {
  const i = text.indexOf(find);
  return i < 0 ? text : text.slice(0, i) + repl + text.slice(i + find.length);
}

function replaceLast(text: string, find: string, repl: string): string {
  const i = text.lastIndexOf(find);
  return i < 0 ? text : text.slice(0, i) + repl + text.slice(i + find.length);
}

/** Reference and name problems for every enabled node. Required-field errors are added by the caller. */
export function collectReferenceProblems(graph: ProblemGraph): Problem[] {
  const problems: Problem[] = [];
  const byName = new Map<string, ProblemNode[]>();
  for (const n of graph.nodes.values()) {
    const list = byName.get(n.data.name);
    if (list) list.push(n);
    else byName.set(n.data.name, [n]);
  }

  for (const n of graph.nodes.values()) {
    if (n.data.node_type_id === "note") continue;
    const list = byName.get(n.data.name) ?? [];
    if (list.length > 1) {
      problems.push({
        key: `dup:${n.data.id}`, nodeId: n.data.id, nodeName: n.data.name,
        severity: "warning", kind: "duplicate-name",
        message: `Another node is also named "${n.data.name}". References to that name are ambiguous: rename one of them.`,
      });
    } else {
      const bad = validateNodeName(n.data.name);
      if (bad) {
        problems.push({
          key: `badname:${n.data.id}`, nodeId: n.data.id, nodeName: n.data.name,
          severity: "warning", kind: "bad-name",
          message: `This name can't be used in references. ${bad}`,
        });
      }
    }
  }

  for (const node of graph.nodes.values()) {
    if (node.disabled || node.data.node_type_id === "note" || problems.length >= MAX_PROBLEMS) continue;
    const upstream = getUpstreamNodes(graph, node.data.id);
    const upstreamIds = new Set(upstream.map(u => u.id));
    const nameOrder = [...upstream.map(u => u.name), ...[...graph.nodes.values()].filter(o => o.data.id !== node.data.id).map(o => o.data.name)];
    const seen = new Set<string>();

    for (const [field, value] of Object.entries(node.data.config ?? {})) {
      walkStrings(value, (s) => {
        for (const m of s.matchAll(REF_RE)) {
          const expr = m[1].trim();
          if (!isNodeReference(expr)) continue;
          const dedupe = `${field}\u0000${m[0]}`;
          if (seen.has(dedupe)) continue;
          seen.add(dedupe);
          const p = checkReference(node, field, m[0], expr, graph, upstreamIds, nameOrder);
          if (p) problems.push(p);
        }
      });
    }
  }
  return problems.slice(0, MAX_PROBLEMS);
}

function checkReference(
  node: ProblemNode,
  field: string,
  raw: string,
  expr: string,
  graph: ProblemGraph,
  upstreamIds: Set<string>,
  nameOrder: string[],
): Problem | null {
  const segs = expr.split(".");
  const name = segs[0];
  const base = { nodeId: node.data.id, nodeName: node.data.name, field, severity: "warning" as const };
  const key = `ref:${node.data.id}:${field}:${raw}`;

  const sources = [...graph.nodes.values()].filter(n => n.data.name === name);
  if (sources.length === 0) {
    const best = closestPath(name, nameOrder);
    return {
      ...base, key, kind: "unknown-node",
      message: `No node is named "${name}", so this reference will be empty.`,
      fix: best ? { label: `Use ${best}`, before: raw, after: replaceFirst(raw, name, best) } : undefined,
    };
  }
  const source = sources[0];
  if (source.data.id === node.data.id) {
    return { ...base, key, kind: "self-reference", message: `This field refers to its own node, "${name}". A node can't use its own result.` };
  }
  if (!upstreamIds.has(source.data.id)) {
    return { ...base, key, kind: "not-upstream", message: `"${name}" isn't connected before this node, so its data isn't available here. Connect it first.` };
  }
  if (segs[1] !== "output") {
    const best = closestPath(segs[1] ?? "", ["output"]);
    return {
      ...base, key, kind: "bad-segment",
      message: `After the node name this should be "output" (${name}.output.field), not "${segs[1]}".`,
      fix: best ? { label: "Use output", before: raw, after: replaceFirst(raw, `${name}.${segs[1]}`, `${name}.output`) } : undefined,
    };
  }
  const first = segs[2];
  if (first === undefined) return null;
  const keyName = topLevelKey(first);
  if (schemaKeys(source.data.output_schema).length === 0 || keyName === "") return null;
  const known = knownTopLevel(source.data.id, source.data.output_schema);
  if (known.includes(keyName)) return null;
  const best = closestPath(keyName, known);
  return {
    ...base, key, kind: "missing-field",
    message: `${name} has no "${keyName}".`,
    fix: best
      ? { label: `Use ${best}`, before: raw, after: replaceLast(raw, keyName, best) }
      : undefined,
  };
}

export interface RunLogLike {
  level: string;
  node_id?: string;
  message: string;
}

let runLogs: RunLogLike[] = [];
const listeners = new Set<() => void>();

/** Feed the last run's log (or null to clear). Notifies subscribers. */
export function setRunLogs(logs: RunLogLike[] | null | undefined): void {
  runLogs = logs ? logs.filter(l => l.level === "warn" && !!l.node_id).slice(0, 500) : [];
  for (const fn of listeners) fn();
}

export function onProblemsInvalidated(fn: () => void): () => void {
  listeners.add(fn);
  return () => { listeners.delete(fn); };
}

const FIELD_NOT_FOUND = /^Expression '\{\{(.*)\}\}': field path '(.*)' not found in node '(.*)' output\./s;
const NO_OUTPUT_YET = /^Expression '\{\{(.*)\}\}': node '(.*)' has not produced output yet\./s;

/** Locate the stored `{{expr}}` text (whatever its inner spacing) in a node's config. */
export function findReference(
  config: Record<string, unknown>,
  expr: string,
): { field: string; before: string } | null {
  for (const [field, value] of Object.entries(config ?? {})) {
    let hit: string | null = null;
    walkStrings(value, (s) => {
      if (hit) return;
      for (const m of s.matchAll(REF_RE)) {
        if (m[1].trim() === expr.trim()) { hit = m[0]; return; }
      }
    });
    if (hit) return { field, before: hit };
  }
  return null;
}

/** Warnings from the last run that still apply to the current configs. */
export function collectRuntimeProblems(graph: ProblemGraph): Problem[] {
  const out: Problem[] = [];
  const seen = new Set<string>();
  for (const log of runLogs) {
    const node = graph.nodes.get(log.node_id!);
    if (!node || node.disabled) continue;
    const empty = FIELD_NOT_FOUND.exec(log.message);
    const pending = empty ? null : NO_OUTPUT_YET.exec(log.message);
    if (!empty && !pending) continue;
    const expr = (empty ?? pending)![1];
    const loc = findReference(node.data.config, expr);
    if (!loc) continue;
    const key = `run:${node.data.id}:${loc.before}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const base = { key, nodeId: node.data.id, nodeName: node.data.name, field: loc.field, severity: "warning" as const };
    if (empty) {
      const path = empty[2];
      const srcName = empty[3];
      const src = [...graph.nodes.values()].find(n => n.data.name === srcName);
      const candidates = src ? listPaths(src.data.output_schema, getRunOutput(src.data.id)).map(p => p.path) : [];
      const best = closestPath(path, candidates);
      out.push({
        ...base, kind: "empty-field",
        message: `${srcName} returned nothing for "${path}", so this field came out empty.`,
        fix: best ? { label: `Use ${best}`, before: loc.before, after: replaceLast(loc.before, path, best) } : undefined,
      });
    } else {
      out.push({
        ...base, kind: "no-output",
        message: `${pending![2]} hadn't produced a result yet when this field was filled in, so it came out empty.`,
      });
    }
  }
  return out;
}

function replaceInValue(value: unknown, before: string, after: string, depth = 0): unknown {
  if (depth > 8) return value;
  if (typeof value === "string") return value.includes(before) ? value.split(before).join(after) : value;
  if (Array.isArray(value)) return value.map(v => replaceInValue(v, before, after, depth + 1));
  if (isObj(value)) {
    const o: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) o[k] = replaceInValue(v, before, after, depth + 1);
    return o;
  }
  return value;
}

/** New value for `config[field]` with the fix applied, or null when the text is no longer there. */
export function applyFixToValue(value: unknown, fix: ProblemFix): unknown | null {
  const next = replaceInValue(value, fix.before, fix.after);
  return JSON.stringify(next) === JSON.stringify(value) ? null : next;
}
