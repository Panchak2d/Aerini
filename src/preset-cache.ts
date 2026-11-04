/**
 * In-memory record of what each node produced in an earlier run, so a Test can
 * reuse it instead of running earlier steps again. Pure apart from SHA-256.
 *
 * An entry is reusable only while the node and everything upstream of it is
 * unchanged: each node gets a chain hash over its own settings and its
 * parents' chain hashes. Nothing here is written to a workflow file or to run
 * history, and the store is dropped when the workflow is switched.
 */

import { canonicalJson } from "./canonical-json";

export const MAX_PRESET_BYTES = 4 * 1024 * 1024;
export const MAX_CACHED_NODES = 500;

const LOOP_TYPE_ID = "loop";
const LOOP_BODY_PORT = "loop_body";

export interface CacheGraph {
  nodes: Map<string, {
    data: {
      id: string;
      name: string;
      node_type_id: string;
      disabled?: boolean;
      config?: unknown;
      credentials?: unknown;
    };
  }>;
  connectors: Map<string, {
    data: { from_node: string; from_port?: string; to_node: string; to_port?: string };
  }>;
}

export interface GraphHashes {
  self: Map<string, string>;
  chain: Map<string, string>;
}

interface Entry {
  output: unknown;
  self: string;
  chain: string;
  at: number;
  bytes: number;
}

export interface PlanStep {
  id: string;
  name: string;
  typeId: string;
}

export interface TestPlan {
  /** Every node upstream of the target, nearest first. */
  ancestors: PlanStep[];
  /** Recorded outputs to hand to the engine, keyed by node id. */
  presets: Record<string, unknown>;
  /** Ancestors answered from the record. */
  cachedSteps: PlanStep[];
  /** Enabled ancestors that will really execute. */
  willRun: PlanStep[];
  /** Ancestors whose own settings changed since they were recorded. */
  changed: PlanStep[];
  /** Ancestors that have a record that no longer matches. */
  stale: PlanStep[];
  /** Time the oldest reused record was made, or null when none is reused. */
  oldestAt: number | null;
}

let _entries = new Map<string, Entry>();
let _useCached = true;

export function clearPresetCache(): void {
  _entries = new Map();
}

export function useCachedResults(): boolean {
  return _useCached;
}

export function setUseCachedResults(value: boolean): void {
  _useCached = value;
}

async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(digest), b => b.toString(16).padStart(2, "0")).join("");
}

interface ParentLink {
  from: string;
  fromPort: string;
  toPort: string;
}

function parentLinks(graph: CacheGraph): Map<string, ParentLink[]> {
  const parents = new Map<string, ParentLink[]>();
  for (const c of graph.connectors.values()) {
    const list = parents.get(c.data.to_node) ?? [];
    list.push({ from: c.data.from_node, fromPort: c.data.from_port ?? "output", toPort: c.data.to_port ?? "input" });
    parents.set(c.data.to_node, list);
  }
  return parents;
}

/**
 * `salt` carries anything outside the graph that changes what nodes produce,
 * such as the run's input variables; a record made under another salt is stale.
 */
export async function hashGraph(graph: CacheGraph, salt: string = ""): Promise<GraphHashes> {
  const parents = parentLinks(graph);
  const self = new Map<string, string>();
  for (const [id, n] of graph.nodes) {
    self.set(id, await sha256Hex(canonicalJson({
      type:        n.data.node_type_id,
      name:        n.data.name,
      disabled:    n.data.disabled ?? false,
      config:      n.data.config ?? null,
      credentials: n.data.credentials ?? null,
    })));
  }

  const order: string[] = [];
  const state = new Map<string, 1 | 2>();
  const visit = (id: string): void => {
    if (!graph.nodes.has(id) || state.get(id) === 2 || state.get(id) === 1) return;
    state.set(id, 1);
    for (const p of parents.get(id) ?? []) visit(p.from);
    state.set(id, 2);
    order.push(id);
  };
  for (const id of graph.nodes.keys()) visit(id);

  const chain = new Map<string, string>();
  for (const id of order) {
    const parts = (parents.get(id) ?? [])
      .map(p => `${p.fromPort}>${p.toPort}:${chain.get(p.from) ?? "unresolved"}`)
      .sort();
    chain.set(id, await sha256Hex(`${self.get(id)}|${parts.join(",")}|${salt}`));
  }
  return { self, chain };
}

function byteSize(value: unknown): number {
  try {
    return JSON.stringify(value)?.length ?? 0;
  } catch {
    return Infinity;
  }
}

/**
 * Record the outputs of a finished run. `hashes` must come from the graph as it
 * was when the run started. Nodes in `skip` (answered from the record) keep
 * their existing entry and timestamp.
 */
export function recordOutputs(
  hashes: GraphHashes,
  outputs: Record<string, unknown> | null | undefined,
  skip: ReadonlySet<string> = new Set(),
  now: number = Date.now(),
): void {
  if (!outputs) return;
  for (const [id, output] of Object.entries(outputs)) {
    if (skip.has(id) || output === undefined) continue;
    const self = hashes.self.get(id);
    const chain = hashes.chain.get(id);
    if (!self || !chain) continue;
    const bytes = byteSize(output);
    if (bytes > MAX_PRESET_BYTES) { _entries.delete(id); continue; }
    if (!_entries.has(id) && _entries.size >= MAX_CACHED_NODES) continue;
    _entries.set(id, { output, self, chain, at: now, bytes });
  }
}

function ancestorSteps(graph: CacheGraph, targetId: string): PlanStep[] {
  const parents = parentLinks(graph);
  const seen = new Set<string>([targetId]);
  const out: PlanStep[] = [];
  let frontier = [targetId];
  while (frontier.length) {
    const next: string[] = [];
    for (const id of frontier) {
      for (const p of parents.get(id) ?? []) {
        const node = graph.nodes.get(p.from);
        if (!node || seen.has(p.from)) continue;
        seen.add(p.from);
        out.push({ id: p.from, name: node.data.name, typeId: node.data.node_type_id });
        next.push(p.from);
      }
    }
    frontier = next;
  }
  return out;
}

function loopOwnedIds(graph: CacheGraph): Set<string> {
  const children = new Map<string, string[]>();
  for (const c of graph.connectors.values()) {
    const list = children.get(c.data.from_node) ?? [];
    list.push(c.data.to_node);
    children.set(c.data.from_node, list);
  }
  const owned = new Set<string>();
  const stack: string[] = [];
  for (const n of graph.nodes.values()) {
    if (n.data.node_type_id === LOOP_TYPE_ID) owned.add(n.data.id);
  }
  for (const c of graph.connectors.values()) {
    if (c.data.from_port === LOOP_BODY_PORT) stack.push(c.data.to_node);
  }
  while (stack.length) {
    const id = stack.pop()!;
    if (owned.has(id)) continue;
    owned.add(id);
    for (const child of children.get(id) ?? []) stack.push(child);
  }
  return owned;
}

export function buildTestPlan(
  graph: CacheGraph,
  targetId: string,
  hashes: GraphHashes,
  useCache: boolean,
): TestPlan {
  const ancestors = ancestorSteps(graph, targetId);
  const loopOwned = loopOwnedIds(graph);
  const presets: Record<string, unknown> = {};
  const cachedSteps: PlanStep[] = [];
  const willRun: PlanStep[] = [];
  const changed: PlanStep[] = [];
  const stale: PlanStep[] = [];
  let oldestAt: number | null = null;
  let budget = MAX_PRESET_BYTES;

  for (const step of ancestors) {
    const node = graph.nodes.get(step.id)!;
    if (node.data.disabled) continue;
    const entry = _entries.get(step.id);
    const fresh = entry !== undefined && entry.chain === hashes.chain.get(step.id);
    if (entry && !fresh) {
      stale.push(step);
      if (entry.self !== hashes.self.get(step.id)) changed.push(step);
    }
    if (useCache && entry && fresh && !loopOwned.has(step.id) && entry.bytes <= budget) {
      budget -= entry.bytes;
      presets[step.id] = entry.output;
      cachedSteps.push(step);
      oldestAt = oldestAt === null ? entry.at : Math.min(oldestAt, entry.at);
    } else {
      willRun.push(step);
    }
  }
  return { ancestors, presets, cachedSteps, willRun, changed, stale, oldestAt };
}

function ageText(at: number, now: number): string {
  const minutes = Math.floor((now - at) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours} h ago` : `${Math.floor(hours / 24)} d ago`;
}

function nameList(steps: PlanStep[]): string {
  const names = steps.map(s => s.name);
  if (names.length <= 3) return names.join(", ");
  return `${names.slice(0, 3).join(", ")} and ${names.length - 3} more`;
}

/** One plain sentence (or two) saying what Test will do with earlier steps. */
export function describeTestPlan(plan: TestPlan, now: number = Date.now()): string {
  if (plan.ancestors.length === 0) return "";
  const parts: string[] = [];
  if (plan.cachedSteps.length) {
    parts.push(`Using results from ${nameList(plan.cachedSteps)} (${ageText(plan.oldestAt ?? now, now)}).`);
  }
  if (plan.changed.length) {
    parts.push(`Earlier results are out of date: ${nameList(plan.changed)} changed.`);
  } else if (plan.stale.length && !plan.cachedSteps.length) {
    parts.push("Earlier results are out of date: a step before them changed.");
  }
  if (plan.willRun.length) {
    parts.push(`Test will run ${nameList(plan.willRun)} first.`);
  }
  return parts.join(" ");
}
