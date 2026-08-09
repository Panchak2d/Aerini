import { DEFAULT_CHAT_SETTINGS, type ChatSettings, type WorkflowDocument } from "./CanvasSerializer";

export interface FieldChange {
  field: string;
  from: unknown;
  to: unknown;
}

export type DiffStatus = "added" | "removed" | "changed";

export interface NodeDiffEntry {
  id: string;
  name: string;
  status: DiffStatus;
  changes: FieldChange[];
}

export interface EdgeDiffEntry {
  id: string;
  from_node: string;
  to_node: string;
  status: DiffStatus;
  changes: FieldChange[];
}

export interface WorkflowDiffResult {
  metadata: FieldChange[];
  nodes: NodeDiffEntry[];
  edges: EdgeDiffEntry[];
  isEmpty: boolean;
}

type RawRecord = Record<string, unknown>;

function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== typeof b) return false;
  if (a === null || b === null || typeof a !== "object") return false;
  const aArr = Array.isArray(a), bArr = Array.isArray(b);
  if (aArr !== bArr) return false;
  if (aArr && bArr) {
    const al = a as unknown[], bl = b as unknown[];
    return al.length === bl.length && al.every((v, i) => deepEqual(v, bl[i]));
  }
  const ao = a as RawRecord, bo = b as RawRecord;
  const ak = Object.keys(ao);
  return ak.length === Object.keys(bo).length
    && ak.every(k => Object.prototype.hasOwnProperty.call(bo, k) && deepEqual(ao[k], bo[k]));
}

// Captured on every saved node but sourced from the live node-type registry
// at load time rather than anything the user edits on this workflow
// (CanvasSerializer.deserialize prefers the registry value over saved JSON
// for these) — diffing them would surface node-registry/app-version noise
// as if it were a workflow change.
const NODE_DIFF_FIELDS = [
  "node_type_id", "node_type", "name", "config", "credentials",
  "retry", "fallback_node", "disabled", "position",
] as const;

const EDGE_DIFF_FIELDS = [
  "from_node", "from_port", "to_node", "to_port",
  "condition", "on_success", "on_failure",
] as const;

function diffFields(fields: readonly string[], a: RawRecord | undefined, b: RawRecord | undefined): FieldChange[] {
  const changes: FieldChange[] = [];
  for (const field of fields) {
    const av = a?.[field];
    const bv = b?.[field];
    if (!deepEqual(av, bv)) changes.push({ field, from: av, to: bv });
  }
  return changes;
}

interface Identified { id: unknown; [k: string]: unknown }

function diffById(
  oldItems: Identified[],
  newItems: Identified[],
  fields: readonly string[],
): Array<{ id: string; old?: Identified; new?: Identified; status: DiffStatus; changes: FieldChange[] }> {
  const oldMap = new Map(oldItems.map(i => [String(i.id), i]));
  const newMap = new Map(newItems.map(i => [String(i.id), i]));
  const out: Array<{ id: string; old?: Identified; new?: Identified; status: DiffStatus; changes: FieldChange[] }> = [];
  for (const [id, a] of oldMap) {
    const b = newMap.get(id);
    if (!b) { out.push({ id, old: a, status: "removed", changes: [] }); continue; }
    const changes = diffFields(fields, a, b);
    if (changes.length) out.push({ id, old: a, new: b, status: "changed", changes });
  }
  for (const [id, b] of newMap) {
    if (!oldMap.has(id)) out.push({ id, new: b, status: "added", changes: [] });
  }
  return out;
}

/**
 * Compares two serialized workflow JSON snapshots (CanvasSerializer's
 * WorkflowDocument shape) and returns an id-keyed diff of nodes and edges
 * plus a flat list of workflow-level setting changes.
 *
 * Deliberately excludes metadata.author/version (always constant in this
 * app) and metadata.created_at/updated_at (CanvasSerializer restamps both
 * on every serialize() call, so comparing them would report "changed" on
 * every single diff regardless of any real edit).
 *
 * Throws if either argument is not valid JSON — callers already wrap the
 * snapshot fetch/compare path in a try/catch (same convention as
 * WorkflowManager.restoreVersion).
 */
export function diffWorkflowJson(oldJson: string, newJson: string): WorkflowDiffResult {
  const oldDoc = JSON.parse(oldJson) as Partial<WorkflowDocument>;
  const newDoc = JSON.parse(newJson) as Partial<WorkflowDocument>;

  const metadata: FieldChange[] = [];
  const pushIfChanged = (field: string, from: unknown, to: unknown) => {
    if (!deepEqual(from, to)) metadata.push({ field, from, to });
  };
  pushIfChanged("Name", oldDoc.name, newDoc.name);
  pushIfChanged("Description", oldDoc.description ?? "", newDoc.description ?? "");
  pushIfChanged("Tags", oldDoc.metadata?.tags ?? [], newDoc.metadata?.tags ?? []);
  pushIfChanged("Parallel execution", !!oldDoc.parallel_execution, !!newDoc.parallel_execution);
  pushIfChanged("Max concurrent nodes", oldDoc.max_concurrent_nodes ?? 8, newDoc.max_concurrent_nodes ?? 8);

  const oldChat: ChatSettings = { ...DEFAULT_CHAT_SETTINGS, ...(oldDoc.settings?.chat ?? {}) };
  const newChat: ChatSettings = { ...DEFAULT_CHAT_SETTINGS, ...(newDoc.settings?.chat ?? {}) };
  pushIfChanged("Chat: allow attachments",     oldChat.allow_attachments,      newChat.allow_attachments);
  pushIfChanged("Chat: allow image responses", oldChat.allow_image_responses,  newChat.allow_image_responses);
  pushIfChanged("Chat: max message length",    oldChat.max_message_length,     newChat.max_message_length);
  pushIfChanged("Chat: session persistence",   oldChat.session_persistence,    newChat.session_persistence);
  pushIfChanged("Chat: show branding",         oldChat.show_branding,          newChat.show_branding);

  const oldNodes = (oldDoc.nodes ?? []) as Identified[];
  const newNodes = (newDoc.nodes ?? []) as Identified[];
  const nodes: NodeDiffEntry[] = diffById(oldNodes, newNodes, NODE_DIFF_FIELDS).map(d => ({
    id: d.id,
    name: String((d.new ?? d.old)?.name ?? d.id),
    status: d.status,
    changes: d.changes,
  }));

  const oldEdges = (oldDoc.edges ?? []) as Identified[];
  const newEdges = (newDoc.edges ?? []) as Identified[];
  const edges: EdgeDiffEntry[] = diffById(oldEdges, newEdges, EDGE_DIFF_FIELDS).map(d => ({
    id: d.id,
    from_node: String((d.new ?? d.old)?.from_node ?? ""),
    to_node:   String((d.new ?? d.old)?.to_node ?? ""),
    status: d.status,
    changes: d.changes,
  }));

  return { metadata, nodes, edges, isEmpty: metadata.length === 0 && nodes.length === 0 && edges.length === 0 };
}
