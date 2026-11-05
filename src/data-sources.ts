/**
 * Shared data-source model: which nodes feed a node, which fields they expose
 * (declared schema plus real keys from the last run), and a small read-only
 * cache of last-run outputs. Pure: no DOM, no Tauri.
 *
 * Path syntax matches the engine resolver: `Name.output.a.b` for objects and
 * `Name.output.items[0].title` for arrays.
 */

export interface SourceGraph {
  connectors: Map<string, { data: { from_node: string; to_node: string } }>;
  nodes: Map<string, { data: { id: string; name: string; output_schema?: Record<string, unknown> } }>;
}

export interface UpstreamNode {
  id: string;
  name: string;
  schema: Record<string, unknown>;
}

export interface PathInfo {
  /** Relative to the node's output, e.g. `latest.title` or `items[0].id`. */
  path: string;
  type: string;
  description: string;
}

export interface IndexEntry {
  nodeId: string;
  nodeName: string;
  /** Relative to the node's output; empty when the whole output is a scalar. */
  path: string;
  /** Full reference without braces, e.g. `Feed.output.latest.title`. */
  ref: string;
  display: string;
}

const MAX_DEPTH = 4;
const MAX_PATHS = 200;
const INDEX_MAX_NODES = 30;
const INDEX_MAX_PER_NODE = 300;
const INDEX_MAX_TOTAL = 3000;
const INDEX_MAX_DEPTH = 6;
const INDEX_ARRAY_ITEMS = 10;
const INDEX_MAX_STRING = 200;
const CLOSEST_MAX_CANDIDATES = 500;
const CLOSEST_MAX_LENGTH = 100;

type Obj = Record<string, unknown>;

function isObj(v: unknown): v is Obj {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

// ── Upstream nodes ───────────────────────────────────────────────────────────

/** Every node that can feed `nodeId` through any chain of connectors, nearest first. */
export function getUpstreamNodes(graph: SourceGraph, nodeId: string): UpstreamNode[] {
  const seen = new Set<string>([nodeId]);
  const order: string[] = [];
  const queue: string[] = [nodeId];
  while (queue.length) {
    const current = queue.shift()!;
    for (const conn of graph.connectors.values()) {
      const from = conn.data.from_node;
      if (conn.data.to_node === current && !seen.has(from)) {
        seen.add(from);
        order.push(from);
        queue.push(from);
      }
    }
  }
  const result: UpstreamNode[] = [];
  for (const id of order) {
    const node = graph.nodes.get(id);
    if (node) result.push({ id: node.data.id, name: node.data.name, schema: node.data.output_schema ?? {} });
  }
  return result;
}

// ── Paths ────────────────────────────────────────────────────────────────────

export function isAddressableKey(key: string): boolean {
  return key.length > 0 && key === key.trim() && !/[.[\]{}]/.test(key);
}

function typeOfSample(v: unknown): string | undefined {
  if (v === undefined || v === null) return undefined;
  if (Array.isArray(v)) return "array";
  return typeof v === "object" ? "object" : typeof v;
}

/**
 * Field paths of a node's output. Declared schema comes first; real keys found
 * in `sample` fill in what the schema does not describe. Arrays are walked
 * through their first element only.
 */
export function listPaths(schema: Obj | undefined, sample?: unknown): PathInfo[] {
  const out = new Map<string, PathInfo>();

  const add = (path: string, type: string, description: string): boolean => {
    if (out.has(path)) return true;
    if (out.size >= MAX_PATHS) return false;
    out.set(path, { path, type, description });
    return true;
  };

  const walkObject = (s: Obj | undefined, v: unknown, prefix: string, depth: number): boolean => {
    const props = isObj(s?.properties) ? (s!.properties as Obj) : {};
    const keys = [...Object.keys(props)];
    if (isObj(v)) for (const k of Object.keys(v)) if (!(k in props)) keys.push(k);

    for (const key of keys) {
      if (!isAddressableKey(key)) continue;
      const childSchema = isObj(props[key]) ? (props[key] as Obj) : undefined;
      const childValue = isObj(v) ? v[key] : undefined;
      const type = (childSchema?.type !== undefined ? String(childSchema.type) : undefined)
        ?? typeOfSample(childValue) ?? "any";
      const description = childSchema?.description !== undefined ? String(childSchema.description) : "";
      const path = prefix ? `${prefix}.${key}` : key;
      if (!add(path, type, description)) return false;
      if (depth + 1 >= MAX_DEPTH) continue;

      if (type === "object" || isObj(childValue) || isObj(childSchema?.properties)) {
        if (!walkObject(childSchema, childValue, path, depth + 1)) return false;
      } else if (type === "array" || Array.isArray(childValue)) {
        const itemSchema = isObj(childSchema?.items) ? (childSchema!.items as Obj) : undefined;
        const first = Array.isArray(childValue) ? childValue[0] : undefined;
        const elemPath = `${path}[0]`;
        if (isObj(first) || isObj(itemSchema?.properties)) {
          if (!walkObject(itemSchema, first, elemPath, depth + 1)) return false;
        } else {
          const elemType = (itemSchema?.type !== undefined ? String(itemSchema.type) : undefined) ?? typeOfSample(first);
          if (elemType && !add(elemPath, elemType, "")) return false;
        }
      }
    }
    return true;
  };

  walkObject(schema, sample, "", 0);
  return [...out.values()];
}

// ── Last-run cache ───────────────────────────────────────────────────────────

let _runOutputs: Readonly<Record<string, unknown>> | null = null;
let _indexMemo = new Map<string, IndexEntry[]>();
const _runListeners = new Set<() => void>();

/** Calls `fn` after the run-output cache changes (a run ends, a node is tested, the cache is cleared). Returns the unsubscribe function. */
export function onRunOutputsChange(fn: () => void): () => void {
  _runListeners.add(fn);
  return () => { _runListeners.delete(fn); };
}

function notifyRunOutputs(): void {
  for (const fn of [..._runListeners]) {
    try { fn(); } catch { /* one failing listener must not block the others */ }
  }
}

/** Replace the cache with a run's outputs, or pass null to clear it. */
export function setRunOutputs(outputs: Record<string, unknown> | null | undefined): void {
  _runOutputs = outputs ? Object.freeze({ ...outputs }) : null;
  _indexMemo = new Map();
  notifyRunOutputs();
}

/** Add or replace the outputs of some nodes without dropping the rest of the cache. */
export function mergeRunOutputs(outputs: Record<string, unknown> | null | undefined): void {
  if (!outputs) return;
  _runOutputs = Object.freeze({ ...(_runOutputs ?? {}), ...outputs });
  _indexMemo = new Map();
  notifyRunOutputs();
}

/** Last-run output of a node, or undefined if it has none. Callers must not mutate it. */
export function getRunOutput(nodeId: string): unknown {
  return _runOutputs ? _runOutputs[nodeId] : undefined;
}

// ── Value index ──────────────────────────────────────────────────────────────

function indexNode(node: { id: string; name: string }, budget: number): IndexEntry[] {
  const root = getRunOutput(node.id);
  const limit = Math.min(INDEX_MAX_PER_NODE, budget);
  const entries: IndexEntry[] = [];
  if (root === undefined || limit <= 0) return entries;

  const base = `${node.name}.output`;
  const walk = (v: unknown, path: string, depth: number): void => {
    if (entries.length >= limit) return;
    if (v === null || v === undefined) return;
    if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") {
      const text = String(v);
      entries.push({
        nodeId: node.id,
        nodeName: node.name,
        path,
        ref: path ? `${base}.${path}` : base,
        display: text.length > INDEX_MAX_STRING ? text.slice(0, INDEX_MAX_STRING) : text,
      });
      return;
    }
    if (depth >= INDEX_MAX_DEPTH) return;
    if (Array.isArray(v)) {
      v.slice(0, INDEX_ARRAY_ITEMS).forEach((item, i) => walk(item, `${path}[${i}]`, depth + 1));
    } else if (isObj(v)) {
      for (const [k, child] of Object.entries(v)) {
        if (entries.length >= limit) return;
        if (isAddressableKey(k)) walk(child, path ? `${path}.${k}` : k, depth + 1);
      }
    }
  };
  walk(root, "", 0);
  return entries;
}

/** Flat `path → display string` index over the last run, for the given nodes. Bounded. */
export function buildValueIndex(nodes: Array<{ id: string; name: string }>): IndexEntry[] {
  const all: IndexEntry[] = [];
  for (const node of nodes.slice(0, INDEX_MAX_NODES)) {
    const key = `${node.id}\u0000${node.name}`;
    let entries = _indexMemo.get(key);
    if (!entries) {
      entries = indexNode(node, INDEX_MAX_TOTAL);
      _indexMemo.set(key, entries);
    }
    const room = INDEX_MAX_TOTAL - all.length;
    if (room <= 0) break;
    all.push(...(entries.length > room ? entries.slice(0, room) : entries));
  }
  return all;
}

// ── Closest match ────────────────────────────────────────────────────────────

function editDistance(a: string, b: string, limit: number): number {
  if (Math.abs(a.length - b.length) > limit) return limit + 1;
  const prev2: number[] = new Array(b.length + 1).fill(0);
  let prev: number[] = Array.from({ length: b.length + 1 }, (_, j) => j);
  for (let i = 1; i <= a.length; i++) {
    const cur: number[] = [i];
    for (let j = 1; j <= b.length; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      let d = Math.min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + cost);
      if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) {
        d = Math.min(d, prev2[j - 2] + 1);
      }
      cur[j] = d;
    }
    for (let j = 0; j <= b.length; j++) prev2[j] = prev[j];
    prev = cur;
  }
  return prev[b.length];
}

/** Best candidate within a small edit distance of `typo` (case-insensitive), or null. */
export function closestPath(typo: string, candidates: string[]): string | null {
  const t = typo.trim().toLowerCase();
  if (!t || t.length > CLOSEST_MAX_LENGTH) return null;
  const limit = Math.max(1, Math.floor(t.length / 3));
  let best: string | null = null;
  let bestDist = limit + 1;
  for (const c of candidates.slice(0, CLOSEST_MAX_CANDIDATES)) {
    if (c.length > CLOSEST_MAX_LENGTH) continue;
    const d = editDistance(t, c.toLowerCase(), limit);
    if (d < bestDist) {
      best = c;
      bestDist = d;
      if (d === 0) break;
    }
  }
  return best;
}

// ── Source colors ────────────────────────────────────────────────────────────

export const SOURCE_COLOR_COUNT = 8;
const _colorByNode = new Map<string, number>();

/** Stable palette slot (0 to SOURCE_COLOR_COUNT-1) for a node id; first come, first served. */
export function sourceColorIndex(nodeId: string): number {
  let idx = _colorByNode.get(nodeId);
  if (idx === undefined) {
    idx = _colorByNode.size % SOURCE_COLOR_COUNT;
    _colorByNode.set(nodeId, idx);
  }
  return idx;
}

// ── Sample values and primary output ─────────────────────────────────────────

/** Value at a path relative to a node's output (`a.b`, `items[0].x`). Undefined when absent. */
export function valueAt(root: unknown, path: string): unknown {
  if (path === "") return root;
  let cur: unknown = root;
  for (const part of path.split(".")) {
    const m = /^([^[\]]*)((?:\[\d+\])*)$/.exec(part);
    if (!m) return undefined;
    if (m[1] !== "") {
      if (!isObj(cur)) return undefined;
      cur = cur[m[1]];
    }
    for (const idx of m[2].matchAll(/\[(\d+)\]/g)) {
      if (!Array.isArray(cur)) return undefined;
      cur = cur[Number(idx[1])];
    }
    if (cur === undefined) return undefined;
  }
  return cur;
}

/** Short single-line text for a sample value; objects and arrays are summarised as JSON. */
export function formatSample(v: unknown, max = 80): string {
  if (v === undefined || v === null) return "";
  const text = typeof v === "string" ? v : JSON.stringify(v) ?? "";
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max)}...` : flat;
}

/** First top-level property of `schema` carrying `flag: true`. */
function flaggedKey(schema: Obj | undefined, flag: string): string | null {
  const props = schema && isObj(schema.properties) ? schema.properties : null;
  if (!props) return null;
  for (const [key, val] of Object.entries(props)) {
    if (isObj(val) && val[flag] === true) return key;
  }
  return null;
}

/** Output property flagged `x-aerini-primary-output`; null when the schema declares none. */
export function declaredPrimaryOutputKey(schema: Obj | undefined): string | null {
  return flaggedKey(schema, "x-aerini-primary-output");
}

/** Input property flagged `x-aerini-primary-input`; null when the schema declares none. */
export function declaredPrimaryInputKey(schema: Obj | undefined): string | null {
  return flaggedKey(schema, "x-aerini-primary-input");
}

/** Top-level output property flagged `x-aerini-primary-output`, or the only property when there is just one. */
export function primaryOutputKey(schema: Obj | undefined): string | null {
  const declared = declaredPrimaryOutputKey(schema);
  if (declared !== null) return declared;
  const props = schema && isObj(schema.properties) ? schema.properties : null;
  if (!props) return null;
  const keys = Object.keys(props);
  return keys.length === 1 && isAddressableKey(keys[0]) ? keys[0] : null;
}

/** Nearest upstream node that has a main result, as the reference to offer in a blank field. */
/** The reference to a node's Main result, or null when no single main field exists. */
export function mainResultExpr(up: { name: string; schema?: Obj }): { key: string; expr: string } | null {
  const key = primaryOutputKey(up.schema);
  return key === null ? null : { key, expr: `{{${up.name}.output.${key}}}` };
}

export function suggestMainResult(
  graph: SourceGraph,
  nodeId: string,
): { nodeId: string; nodeName: string; key: string; expr: string } | null {
  for (const up of getUpstreamNodes(graph, nodeId)) {
    const key = primaryOutputKey(up.schema);
    if (key !== null) return { nodeId: up.id, nodeName: up.name, key, expr: `{{${up.name}.output.${key}}}` };
  }
  return null;
}
