import { NODE_IDS } from "../node-ids";
import { AI_CHAT_FIELDS, isBlank } from "./chat-defaults";

export interface ChatGraphNode {
  id:     string;
  name:   string;
  type:   string;
  config: Record<string, unknown>;
}

export interface ChatGraphEdge {
  from_node:  string;
  from_port?: string;
  to_node:    string;
  to_port:    string;
}

export interface NodeRef { id: string; name: string; }

export type ReadinessIssue =
  /** Files can be sent, but nothing in the workflow reads the Webhook's files. */
  | { kind: "files_unread"; webhook: NodeRef; candidates: NodeRef[] }
  /** `to` reads `from`'s output but `from` does not run before it. */
  | { kind: "not_connected"; from: NodeRef; to: NodeRef; fixable: boolean }
  /** An AI node after the Webhook has no prompt, so it never sees the typed message. */
  | { kind: "prompt_unset"; webhook: NodeRef; node: NodeRef; field: string }
  /** The Output Chat reads shows an AI node's whole result, which renders as raw JSON. */
  | { kind: "reply_raw"; output: NodeRef; source: NodeRef; field: string };

const FILE_READER_TYPES = new Set<string>([NODE_IDS.AI_PROMPT, NODE_IDS.AI_AGENT]);
const MAX_SCANNED_STRING = 20_000;
const MAX_SCAN_DEPTH = 6;

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** Matches `Name.output<suffix>` where the name is not the tail of a longer one ("AI Memory (read)" must not match "Memory (read)"). */
function outputRef(name: string, suffix: string): RegExp {
  return new RegExp(`(?:^|[^A-Za-z0-9_\\s])\\s*${escapeRegExp(name)}\\.output${suffix}(?![A-Za-z0-9_])`);
}

function collectStrings(value: unknown, out: string[], depth = 0): void {
  if (typeof value === "string") {
    if (value.length <= MAX_SCANNED_STRING) out.push(value);
    return;
  }
  if (depth >= MAX_SCAN_DEPTH || !value || typeof value !== "object") return;
  for (const v of Object.values(value as Record<string, unknown>)) collectStrings(v, out, depth + 1);
}

function downstreamOf(startId: string, adjacency: Map<string, string[]>): Set<string> {
  const seen = new Set<string>();
  const stack = [startId];
  while (stack.length > 0) {
    const id = stack.pop()!;
    for (const next of adjacency.get(id) ?? []) {
      if (!seen.has(next)) { seen.add(next); stack.push(next); }
    }
  }
  return seen;
}

/**
 * Static check of what the Chat panel depends on: the Webhook's files reaching
 * a node that reads them (when `checkFiles`), and every node that reads another
 * node's output actually running after it, an AI node after the Webhook with no
 * prompt, and an Output that would render an AI node's result as raw JSON.
 * Never throws; an unidentifiable Webhook yields no file issue.
 */
export function checkChatReadiness(
  nodes: ChatGraphNode[],
  edges: ChatGraphEdge[],
  opts: { checkFiles: boolean },
): ReadinessIssue[] {
  const webhook = nodes.find(n => n.type === NODE_IDS.WEBHOOK);
  if (!webhook) return [];

  const adjacency = new Map<string, string[]>();
  const incoming  = new Set<string>();
  for (const e of edges) {
    adjacency.set(e.from_node, [...(adjacency.get(e.from_node) ?? []), e.to_node]);
    if (e.to_port === "input") incoming.add(e.to_node);
  }

  const strings = new Map<string, string[]>();
  for (const n of nodes) {
    const list: string[] = [];
    collectStrings(n.config, list);
    strings.set(n.id, list);
  }
  const reads = (n: ChatGraphNode, re: RegExp) => (strings.get(n.id) ?? []).some(s => re.test(s));

  const issues: ReadinessIssue[] = [];
  const named = nodes.filter(n => n.name.trim() !== "");
  const downstream = downstreamOf(webhook.id, adjacency);

  for (const source of named) {
    const re = outputRef(source.name.trim(), "");
    const sourceDownstream = downstreamOf(source.id, adjacency);
    const wouldCycle = (reader: ChatGraphNode) => downstreamOf(reader.id, adjacency).has(source.id);
    for (const reader of nodes) {
      if (reader.id === source.id || sourceDownstream.has(reader.id) || !reads(reader, re)) continue;
      issues.push({
        kind:    "not_connected",
        from:    { id: source.id, name: source.name },
        to:      { id: reader.id, name: reader.name },
        fixable: !incoming.has(reader.id) && !wouldCycle(reader),
      });
    }
  }

  if (opts.checkFiles && webhook.name.trim() !== "") {
    const filesRef = outputRef(webhook.name.trim(), "\\.(?:files|body\\.attachments)");
    const consumed = nodes.some(n => n.id !== webhook.id && reads(n, filesRef))
      || edges.some(e => e.from_node === webhook.id && e.to_port === "attachments");
    if (!consumed) {
      issues.push({
        kind:       "files_unread",
        webhook:    { id: webhook.id, name: webhook.name },
        candidates: nodes
          .filter(n => FILE_READER_TYPES.has(n.type) && downstream.has(n.id))
          .map(n => ({ id: n.id, name: n.name })),
      });
    }
  }

  if (webhook.name.trim() !== "") {
    for (const n of nodes) {
      const fields = AI_CHAT_FIELDS[n.type];
      if (!fields || !downstream.has(n.id) || !isBlank(n.config[fields.input])) continue;
      issues.push({
        kind:    "prompt_unset",
        webhook: { id: webhook.id, name: webhook.name },
        node:    { id: n.id, name: n.name },
        field:   fields.input,
      });
    }
  }

  // Chat reads the first Output node it finds, so only that one can show raw JSON.
  const output = nodes.find(n => n.type === NODE_IDS.OUTPUT);
  if (output && downstream.has(output.id) && isBlank(output.config["field"]) && isBlank(output.config["source_node"])) {
    for (const e of edges) {
      if (e.to_node !== output.id || e.to_port !== "input" || (e.from_port ?? "output") !== "output") continue;
      const source = nodes.find(n => n.id === e.from_node);
      const fields = source && AI_CHAT_FIELDS[source.type];
      if (!source || !fields) continue;
      issues.push({
        kind:   "reply_raw",
        output: { id: output.id, name: output.name },
        source: { id: source.id, name: source.name },
        field:  fields.reply,
      });
      break;
    }
  }

  return issues;
}
