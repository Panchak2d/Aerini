import type { Canvas } from "./Canvas";
import type { CanvasNode } from "./Node";
import type { Connector } from "./Connector";

export type MoveEntry = { nodeId: string; from: { x: number; y: number }; to: { x: number; y: number } };

export type ConfigPatch = { nodeId: string; keys: Record<string, { before: string | null; after: string | null }> };
type WirePort = { node: string; port: string };

export type NodeProps = { name: string; disabled: boolean; retry: { max_attempts: number; backoff_ms: number }; credentials: Record<string, string> };
export type PropChange = { [K in keyof NodeProps]?: { before: NodeProps[K]; after: NodeProps[K] } };

export type UndoAction =
  | { type: "add_node";    node: CanvasNode }
  | { type: "delete_node"; node: CanvasNode; connectors: Connector[] }
  | { type: "add_edge";    connector: Connector }
  | { type: "delete_edge"; connector: Connector }
  | { type: "reroute_edge"; connector: Connector; from: WirePort; to: WirePort; configs: ConfigPatch[]; replaced?: Connector[] }
  | { type: "move_sources"; connectors: Connector[]; from: WirePort; to: WirePort; configs: ConfigPatch[] }
  | { type: "batch";       actions: UndoAction[] }
  | { type: "split_edge";  removed: Connector; added1: Connector; added2: Connector; node: CanvasNode; configs: ConfigPatch[] }
  | { type: "move_node";   nodeId: string; from: { x: number; y: number }; to: { x: number; y: number } }
  | { type: "move_nodes";  moves: MoveEntry[] }
  | { type: "cut_edges";   connectors: Connector[] }
  | { type: "config_patch"; configs: ConfigPatch[] }
  | { type: "node_props";  nodeId: string; props: PropChange; configs: ConfigPatch[]; wires: Connector[] };

export type HistoryEventKind = "push" | "undo" | "redo" | "clear" | "empty-undo" | "empty-redo" | "busy" | "failed";

export interface HistoryState {
  kind: HistoryEventKind;
  /** Label of the step that was just undone/redone (undo/redo events only). */
  label?: string;
  canUndo: boolean;
  canRedo: boolean;
  undoLabel: string | null;
  redoLabel: string | null;
}

export type HistoryResult =
  | { ok: true; label: string }
  | { ok: false; reason: "empty" | "busy" | "failed" };

export const MAX_HISTORY = 200;
export const COALESCE_MS = 1500;

type Entry = { action: UndoAction; label: string; key?: string; at: number };
type NodeSnapshot = { name: string; disabled: boolean; retry: NodeProps["retry"]; credentials: NodeProps["credentials"]; config: Map<string, string> };

/** Ctrl/Cmd+Z, Ctrl/Cmd+Shift+Z and Ctrl/Cmd+Y, independent of Caps Lock, Shift-cased `key` and non-Latin layouts. */
export function historyShortcut(
  e: Pick<KeyboardEvent, "key" | "code" | "shiftKey" | "altKey" | "ctrlKey" | "metaKey">,
): "undo" | "redo" | null {
  if (!(e.ctrlKey || e.metaKey) || e.altKey) return null;
  const letter = /^[a-z]$/i.test(e.key)
    ? e.key.toLowerCase()
    : e.code === "KeyZ" ? "z" : e.code === "KeyY" ? "y" : "";
  if (letter === "z") return e.shiftKey ? "redo" : "undo";
  if (letter === "y" && !e.shiftKey) return "redo";
  return null;
}

export function snapshotConfig(node: CanvasNode): Map<string, string> {
  const out = new Map<string, string>();
  for (const [k, v] of Object.entries(node.data.config)) {
    const json = JSON.stringify(v);
    if (json !== undefined) out.set(k, json);
  }
  return out;
}

export function diffConfig(nodeId: string, before: Map<string, string>, after: Map<string, string>): ConfigPatch | null {
  const keys: ConfigPatch["keys"] = {};
  for (const k of new Set([...before.keys(), ...after.keys()])) {
    const b = before.get(k) ?? null, a = after.get(k) ?? null;
    if (b !== a) keys[k] = { before: b, after: a };
  }
  return Object.keys(keys).length ? { nodeId, keys } : null;
}

function snapshotNode(n: CanvasNode): NodeSnapshot {
  return {
    name: n.data.name,
    disabled: n.disabled,
    retry: { ...n.data.retry },
    credentials: { ...n.data.credentials },
    config: snapshotConfig(n),
  };
}

const retryEq = (a: NodeProps["retry"], b: NodeProps["retry"]) =>
  a.max_attempts === b.max_attempts && a.backoff_ms === b.backoff_ms;

const credentialsEq = (a: NodeProps["credentials"], b: NodeProps["credentials"]) => {
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length && keys.every(k => a[k] === b[k]);
};

function diffProps(before: NodeSnapshot, after: NodeSnapshot): PropChange {
  const p: PropChange = {};
  if (before.name !== after.name) p.name = { before: before.name, after: after.name };
  if (before.disabled !== after.disabled) p.disabled = { before: before.disabled, after: after.disabled };
  if (!retryEq(before.retry, after.retry)) p.retry = { before: { ...before.retry }, after: { ...after.retry } };
  if (!credentialsEq(before.credentials, after.credentials)) p.credentials = { before: { ...before.credentials }, after: { ...after.credentials } };
  return p;
}

function isNoopProps(a: Extract<UndoAction, { type: "node_props" }>): boolean {
  const p = a.props;
  if (p.name && p.name.before !== p.name.after) return false;
  if (p.disabled && p.disabled.before !== p.disabled.after) return false;
  if (p.retry && !retryEq(p.retry.before, p.retry.after)) return false;
  if (p.credentials && !credentialsEq(p.credentials.before, p.credentials.after)) return false;
  if (a.wires.length) return false;
  return a.configs.every(c => Object.values(c.keys).every(k => k.before === k.after));
}

function mergeProps(
  prev: Extract<UndoAction, { type: "node_props" }>,
  next: Extract<UndoAction, { type: "node_props" }>,
): Extract<UndoAction, { type: "node_props" }> {
  const props: PropChange = { ...prev.props };
  for (const k of Object.keys(next.props) as (keyof NodeProps)[]) {
    const n = next.props[k] as { before: unknown; after: unknown };
    const p = prev.props[k] as { before: unknown; after: unknown } | undefined;
    (props as Record<string, unknown>)[k] = { before: p ? p.before : n.before, after: n.after };
  }
  const byNode = new Map<string, ConfigPatch>();
  for (const patch of prev.configs) byNode.set(patch.nodeId, { nodeId: patch.nodeId, keys: { ...patch.keys } });
  for (const patch of next.configs) {
    const cur = byNode.get(patch.nodeId) ?? { nodeId: patch.nodeId, keys: {} };
    for (const [k, v] of Object.entries(patch.keys)) cur.keys[k] = { before: k in cur.keys ? cur.keys[k].before : v.before, after: v.after };
    byNode.set(patch.nodeId, cur);
  }
  return { ...prev, props, configs: [...byNode.values()], wires: [...prev.wires, ...next.wires] };
}

const clip = (s: string) => (s.length > 40 ? `${s.slice(0, 39)}…` : s);

export class UndoManager {
  private canvas: Canvas;
  private stack: Entry[] = [];
  private redos: Entry[] = [];
  private txn: { label: string; actions: UndoAction[] } | null = null;
  private applying = false;
  private baselines = new Map<string, NodeSnapshot>();

  constructor(canvas: Canvas) {
    this.canvas = canvas;
  }

  // ── State ────────────────────────────────────────────────────────────────

  canUndo(): boolean { return this.stack.length > 0; }
  canRedo(): boolean { return this.redos.length > 0; }

  state(kind: HistoryEventKind = "push", label?: string): HistoryState {
    return {
      kind, label,
      canUndo: this.stack.length > 0,
      canRedo: this.redos.length > 0,
      undoLabel: this.stack.length ? this.stack[this.stack.length - 1].label : null,
      redoLabel: this.redos.length ? this.redos[this.redos.length - 1].label : null,
    };
  }

  /** Drops all history. The canvas calls this whenever its node or connector map is replaced (another workflow was loaded). */
  clear(): void {
    const had = this.stack.length > 0 || this.redos.length > 0;
    this.stack = []; this.redos = []; this.txn = null; this.baselines.clear();
    if (had) this.notify("clear");
  }

  private notify(kind: HistoryEventKind, label?: string): void {
    this.canvas.onHistoryChange?.(this.state(kind, label));
  }

  // ── Recording ────────────────────────────────────────────────────────────

  push(a: UndoAction, opts: { label?: string; key?: string } = {}): void {
    if (this.applying) return;
    if (this.txn) { this.txn.actions.push(a); return; }

    const now = Date.now();
    const top = this.stack[this.stack.length - 1];
    if (opts.key && top && top.key === opts.key && now - top.at <= COALESCE_MS
        && top.action.type === "node_props" && a.type === "node_props") {
      const merged = mergeProps(top.action, a);
      this.redos = [];
      if (isNoopProps(merged)) this.stack.pop();
      else { top.action = merged; top.label = opts.label ?? this.describe(merged); top.at = now; }
      this.notify("push");
      return;
    }

    this.stack.push({ action: a, label: opts.label ?? this.describe(a), key: opts.key, at: now });
    if (this.stack.length > MAX_HISTORY) this.stack.shift();
    this.redos = [];
    this.notify("push");
  }

  /**
   * Runs `fn` and records everything it pushes as ONE undo step. Nested calls join
   * the outer step. The step is recorded even when `fn` throws, since whatever it
   * mutated before throwing is already live.
   */
  transact<T>(label: string, fn: () => T): T {
    if (this.txn || this.applying) return fn();
    const txn = { label, actions: [] as UndoAction[] };
    this.txn = txn;
    try {
      return fn();
    } finally {
      this.txn = null;
      if (txn.actions.length) {
        this.push(txn.actions.length === 1 ? txn.actions[0] : { type: "batch", actions: txn.actions }, { label });
      }
    }
  }

  // ── Node property edits (name, enabled, retry, config) ───────────────────

  /** Remember the node's current state as the "before" for the next commitNode(). */
  trackNode(n: CanvasNode): void {
    this.baselines.set(n.data.id, snapshotNode(n));
  }

  /**
   * Records whatever changed on `n` since trackNode()/the previous commit. Rapid
   * edits to the same fields merge into one step; `wires` are connectors the edit
   * removed from the graph (already deleted by the caller).
   */
  commitNode(n: CanvasNode, opts: { wires?: Connector[]; coalesce?: boolean } = {}): void {
    const id = n.data.id;
    const before = this.baselines.get(id);
    const after = snapshotNode(n);
    this.baselines.set(id, after);
    if (!before) return;
    const props = diffProps(before, after);
    const patch = diffConfig(id, before.config, after.config);
    const wires = opts.wires ?? [];
    if (!Object.keys(props).length && !patch && !wires.length) return;
    this.push(
      { type: "node_props", nodeId: id, props, configs: patch ? [patch] : [], wires },
      { key: opts.coalesce === false ? undefined : `edit:${id}` },
    );
  }

  /** One-shot property change (context-menu rename, enable/disable): never merged with neighbours. */
  editNode(n: CanvasNode, mutate: () => void): void {
    this.trackNode(n);
    mutate();
    this.commitNode(n, { coalesce: false });
  }

  // ── Undo / redo ──────────────────────────────────────────────────────────

  undo(): HistoryResult { return this.step("undo"); }
  redo(): HistoryResult { return this.step("redo"); }

  private step(dir: "undo" | "redo"): HistoryResult {
    const from = dir === "undo" ? this.stack : this.redos;
    const to   = dir === "undo" ? this.redos : this.stack;
    if (this.applying || this.canvas.isGestureActive?.()) { this.notify("busy"); return { ok: false, reason: "busy" }; }
    const entry = from.pop();
    if (!entry) { this.notify(dir === "undo" ? "empty-undo" : "empty-redo"); return { ok: false, reason: "empty" }; }

    this.txn = null;
    this.applying = true;
    try {
      if (dir === "undo") this.revert(entry.action); else this.reapply(entry.action);
    } catch (err) {
      // The step is dropped: replaying a half-applied step could only compound the damage.
      console.error(`Aerini: ${dir} failed:`, err);
      this.sweepDangling();
      this.applying = false;
      this.canvas.afterHistoryApplied?.(this.affected(entry.action));
      this.canvas.onCanvasChanged?.();
      this.notify("failed", entry.label);
      return { ok: false, reason: "failed" };
    }
    this.applying = false;
    this.sweepDangling();
    entry.key = undefined;
    to.push(entry);
    const next = this.stack[this.stack.length - 1];
    if (next) next.key = undefined;
    const ids = this.affected(entry.action);
    for (const id of ids) if (this.baselines.has(id)) { const n = this.canvas.nodes.get(id); if (n) this.baselines.set(id, snapshotNode(n)); else this.baselines.delete(id); }
    this.canvas.afterHistoryApplied?.(ids);
    this.canvas.onCanvasChanged?.();
    this.notify(dir, entry.label);
    return { ok: true, label: entry.label };
  }

  /** Wires whose endpoint node no longer exists can never be valid; drop them rather than save a broken graph. */
  private sweepDangling(): void {
    const { nodes, connectors } = this.canvas;
    for (const [id, c] of connectors) {
      if (!nodes.has(c.data.from_node) || !nodes.has(c.data.to_node)) connectors.delete(id);
    }
  }

  // ── Labels / affected nodes ──────────────────────────────────────────────

  private nodeName(id: string): string {
    return clip(this.canvas.nodes.get(id)?.data.name ?? "node");
  }

  private describe(a: UndoAction): string {
    switch (a.type) {
      case "add_node":      return `Add "${clip(a.node.data.name)}"`;
      case "delete_node":   return `Delete "${clip(a.node.data.name)}"`;
      case "add_edge":      return "Connect nodes";
      case "delete_edge":   return "Delete wire";
      case "reroute_edge":  return "Move wire";
      case "move_sources":  return "Move wires";
      case "split_edge":    return `Insert "${clip(a.node.data.name)}" into wire`;
      case "move_node":     return `Move "${this.nodeName(a.nodeId)}"`;
      case "move_nodes":    return `Move ${a.moves.length} nodes`;
      case "cut_edges":     return a.connectors.length === 1 ? "Cut wire" : `Cut ${a.connectors.length} wires`;
      case "config_patch":  return "Change settings";
      case "node_props": {
        const p = a.props;
        if (p.name && Object.keys(p).length === 1 && !a.configs.length) return `Rename "${clip(p.name.after)}"`;
        if (p.disabled && Object.keys(p).length === 1 && !a.configs.length) return `${p.disabled.after ? "Disable" : "Enable"} "${this.nodeName(a.nodeId)}"`;
        if (p.credentials && Object.keys(p).length === 1 && !a.configs.length) return `Change credential on "${this.nodeName(a.nodeId)}"`;
        return `Edit "${this.nodeName(a.nodeId)}"`;
      }
      case "batch":         return a.actions.length ? this.describe(a.actions[0]) : "Edit";
    }
  }

  private affected(a: UndoAction, out = new Set<string>()): string[] {
    const wire = (c: Connector) => { out.add(c.data.from_node); out.add(c.data.to_node); };
    switch (a.type) {
      case "add_node":     out.add(a.node.data.id); break;
      case "delete_node":  out.add(a.node.data.id); a.connectors.forEach(wire); break;
      case "add_edge":
      case "delete_edge":  wire(a.connector); break;
      case "reroute_edge": wire(a.connector); out.add(a.from.node); out.add(a.to.node); (a.replaced ?? []).forEach(wire); break;
      case "move_sources": a.connectors.forEach(wire); out.add(a.from.node); out.add(a.to.node); break;
      case "split_edge":   out.add(a.node.data.id); wire(a.removed); a.configs.forEach(p => out.add(p.nodeId)); break;
      case "move_node":    out.add(a.nodeId); break;
      case "move_nodes":   a.moves.forEach(m => out.add(m.nodeId)); break;
      case "cut_edges":    a.connectors.forEach(wire); break;
      case "config_patch": a.configs.forEach(p => out.add(p.nodeId)); break;
      case "node_props":   out.add(a.nodeId); a.wires.forEach(wire); break;
      case "batch":        a.actions.forEach(s => this.affected(s, out)); break;
    }
    return [...out];
  }

  // ── Pure state transitions ───────────────────────────────────────────────
  // Neither direction recomputes anything from live canvas state: every side
  // effect of the original edit (injected expressions, defaulted config, pruned
  // ports) is carried by a ConfigPatch recorded when the edit happened, so undo
  // restores exactly what was there and redo reproduces exactly what was done.

  private setProps(n: CanvasNode, p: PropChange, side: "before" | "after"): void {
    if (p.name) n.data.name = p.name[side];
    if (p.disabled) n.disabled = p.disabled[side];
    if (p.retry) n.data.retry = { ...p.retry[side] };
    if (p.credentials) {
      const target = n.data.credentials;
      for (const k of Object.keys(target)) delete target[k];
      Object.assign(target, p.credentials[side]);
    }
  }

  private revert(a: UndoAction): void {
    const c = this.canvas;
    switch (a.type) {
      case "add_node":
        c.nodes.delete(a.node.data.id);
        break;
      case "delete_node":
        c.nodes.set(a.node.data.id, a.node);
        for (const conn of a.connectors) c.connectors.set(conn.data.id, conn);
        break;
      case "add_edge":
        c.connectors.delete(a.connector.data.id);
        break;
      case "delete_edge":
        c.connectors.set(a.connector.data.id, a.connector);
        break;
      case "reroute_edge":
        a.connector.data.to_node = a.from.node;
        a.connector.data.to_port = a.from.port;
        for (const r of a.replaced ?? []) c.connectors.set(r.data.id, r);
        c.applyConfigPatches(a.configs, "before");
        break;
      case "move_sources":
        for (const conn of a.connectors) {
          conn.data.from_node = a.from.node;
          conn.data.from_port = a.from.port;
        }
        c.applyConfigPatches(a.configs, "before");
        break;
      case "move_node": {
        const n = c.nodes.get(a.nodeId);
        if (n) { n.data.position = { ...a.from }; n.updatePortPositions(); }
        break;
      }
      case "move_nodes":
        for (const m of a.moves) {
          const n = c.nodes.get(m.nodeId);
          if (n) { n.data.position = { ...m.from }; n.updatePortPositions(); }
        }
        break;
      case "cut_edges":
        for (const conn of a.connectors) c.connectors.set(conn.data.id, conn);
        break;
      case "split_edge":
        c.connectors.delete(a.added1.data.id);
        c.connectors.delete(a.added2.data.id);
        c.connectors.set(a.removed.data.id, a.removed);
        c.applyConfigPatches(a.configs, "before");
        break;
      case "config_patch":
        c.applyConfigPatches(a.configs, "before");
        break;
      case "node_props": {
        const n = c.nodes.get(a.nodeId);
        if (!n) break;
        this.setProps(n, a.props, "before");
        c.applyConfigPatches(a.configs, "before");
        for (const w of a.wires) c.connectors.set(w.data.id, w);
        break;
      }
      case "batch":
        for (let i = a.actions.length - 1; i >= 0; i--) this.revert(a.actions[i]);
        break;
    }
  }

  private reapply(a: UndoAction): void {
    const c = this.canvas;
    switch (a.type) {
      case "add_node":
        c.nodes.set(a.node.data.id, a.node);
        break;
      case "delete_node":
        c.nodes.delete(a.node.data.id);
        for (const conn of a.connectors) c.connectors.delete(conn.data.id);
        break;
      case "add_edge":
        c.connectors.set(a.connector.data.id, a.connector);
        break;
      case "delete_edge":
        c.connectors.delete(a.connector.data.id);
        break;
      case "reroute_edge":
        a.connector.data.to_node = a.to.node;
        a.connector.data.to_port = a.to.port;
        for (const r of a.replaced ?? []) c.connectors.delete(r.data.id);
        c.applyConfigPatches(a.configs, "after");
        break;
      case "move_sources":
        for (const conn of a.connectors) {
          conn.data.from_node = a.to.node;
          conn.data.from_port = a.to.port;
        }
        c.applyConfigPatches(a.configs, "after");
        break;
      case "move_node": {
        const n = c.nodes.get(a.nodeId);
        if (n) { n.data.position = { ...a.to }; n.updatePortPositions(); }
        break;
      }
      case "move_nodes":
        for (const m of a.moves) {
          const n = c.nodes.get(m.nodeId);
          if (n) { n.data.position = { ...m.to }; n.updatePortPositions(); }
        }
        break;
      case "cut_edges":
        for (const conn of a.connectors) c.connectors.delete(conn.data.id);
        break;
      case "split_edge":
        c.connectors.delete(a.removed.data.id);
        c.connectors.set(a.added1.data.id, a.added1);
        c.connectors.set(a.added2.data.id, a.added2);
        c.applyConfigPatches(a.configs, "after");
        break;
      case "config_patch":
        c.applyConfigPatches(a.configs, "after");
        break;
      case "node_props": {
        const n = c.nodes.get(a.nodeId);
        if (!n) break;
        this.setProps(n, a.props, "after");
        c.applyConfigPatches(a.configs, "after");
        for (const w of a.wires) c.connectors.delete(w.data.id);
        break;
      }
      case "batch":
        for (const sub of a.actions) this.reapply(sub);
        break;
    }
  }
}
