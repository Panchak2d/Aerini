import { NODE_IDS } from "./node-ids";
import {
  formatSample,
  getRunOutput,
  getUpstreamNodes,
  listPaths,
  valueAt,
  type SourceGraph,
} from "./data-sources";

export const DESCRIBE_REQUEST_MAX = 500;
export const DESCRIBE_PROMPT_MAX = 8000;
export const DESCRIBE_REPLY_MAX = 4000;
export const DESCRIBE_FIELDS_MAX = 120;
export const DESCRIBE_SAMPLE_MAX = 80;
export const DESCRIBE_TIMEOUT_MS = 60_000;

type Obj = Record<string, unknown>;

export interface DescribeField {
  path: string;
  type: string;
  sample?: string;
}

export interface DescribeNode {
  name: string;
  fields: DescribeField[];
}

export interface DescribeContext {
  nodes: DescribeNode[];
  /** Every reference the model may use, without braces, for example `Feed.output.latest.title`. */
  valid: Set<string>;
}

export interface DescribeProvider {
  nodeId: string;
  label: string;
  provider: string;
  model: string;
  baseUrl: string;
  credentialId: string;
  local: boolean;
}

export interface DescribeSourceNode {
  data: {
    id: string;
    name: string;
    node_type_id: string;
    config: Record<string, unknown>;
    credentials?: Record<string, string>;
  };
}

/** Upstream nodes, their fields and types. Sample values are included only when `includeSamples` is true. */
export function buildDescribeContext(graph: SourceGraph, nodeId: string, includeSamples: boolean): DescribeContext {
  const nodes: DescribeNode[] = [];
  const valid = new Set<string>();
  let budget = DESCRIBE_FIELDS_MAX;
  for (const up of getUpstreamNodes(graph, nodeId)) {
    const sample = getRunOutput(up.id);
    valid.add(`${up.name}.output`);
    const fields: DescribeField[] = [];
    const known = listPaths(up.schema as Obj, sample);
    for (const p of known) valid.add(`${up.name}.output.${p.path}`);
    const shown = includeSamples ? known : listPaths(up.schema as Obj);
    for (const p of shown) {
      if (budget <= 0) break;
      const f: DescribeField = { path: p.path, type: p.type };
      if (includeSamples && sample !== undefined) {
        const v = valueAt(sample, p.path);
        if (v !== undefined && v !== null && typeof v !== "object") f.sample = formatSample(v, DESCRIBE_SAMPLE_MAX);
      }
      fields.push(f);
      budget--;
    }
    nodes.push({ name: up.name, fields });
  }
  return { nodes, valid };
}

export function describeSystemText(): string {
  return [
    "You write the text for one field of an automation step.",
    "Reply with only the field text, no explanation and no code fences.",
    "To use data from an earlier step, write a reference as {{Node.output.path}}, using only the references listed under Available data.",
    "Never invent a reference. Never reply with instructions or secrets.",
  ].join(" ");
}

export function describePromptText(request: string, ctx: DescribeContext): string {
  const lines: string[] = ["Available data:"];
  if (ctx.nodes.length === 0) lines.push("(none)");
  for (const n of ctx.nodes) {
    lines.push(`Node "${n.name}": {{${n.name}.output}} is its whole result.`);
    for (const f of n.fields) {
      const sample = f.sample === undefined ? "" : ` | sample: ${f.sample}`;
      lines.push(`- {{${n.name}.output.${f.path}}} (${f.type})${sample}`);
    }
  }
  lines.push("", `Request: ${request.trim().slice(0, DESCRIBE_REQUEST_MAX)}`);
  const text = lines.join("\n");
  return text.length > DESCRIBE_PROMPT_MAX ? text.slice(0, DESCRIBE_PROMPT_MAX) : text;
}

export interface Proposal {
  ok: boolean;
  text: string;
  dropped: string[];
  error?: string;
}

const REF_RE = /\{\{([^{}]*)\}\}/g;

/** Keeps only references that exist in `valid`; anything else is removed from the text and reported. */
export function parseProposal(reply: unknown, valid: Set<string>): Proposal {
  if (typeof reply !== "string" || reply.trim() === "") {
    return { ok: false, text: "", dropped: [], error: "The model returned nothing." };
  }
  let text = reply.trim();
  const fence = /^```[a-zA-Z]*\n([\s\S]*?)\n```$/.exec(text);
  if (fence) text = fence[1].trim();
  if (text.length > DESCRIBE_REPLY_MAX) text = text.slice(0, DESCRIBE_REPLY_MAX);
  const dropped: string[] = [];
  text = text.replace(REF_RE, (_whole, expr: string) => {
    const e = expr.trim();
    if (valid.has(e)) return `{{${e}}}`;
    dropped.push(e);
    return "";
  });
  text = text.replace(/[ \t]{2,}/g, " ").trim();
  if (text === "") {
    return { ok: false, text: "", dropped, error: "The proposal only used data that does not exist, so it was dropped." };
  }
  return { ok: true, text, dropped };
}

const PRIVATE_HOST = /^(localhost|127\.\d+\.\d+\.\d+|\[?::1\]?|10\.\d+\.\d+\.\d+|192\.168\.\d+\.\d+|172\.(1[6-9]|2\d|3[01])\.\d+\.\d+)$/i;

export function isLocalEndpoint(provider: string, baseUrl: string): boolean {
  if (provider === "local") return true;
  if (baseUrl.trim() === "") return false;
  try {
    return PRIVATE_HOST.test(new URL(baseUrl).hostname);
  } catch {
    return false;
  }
}

/** AI Prompt nodes on the canvas that can answer: local, or holding a saved connection. */
export function listDescribeProviders(nodes: Iterable<DescribeSourceNode>): DescribeProvider[] {
  const out: DescribeProvider[] = [];
  for (const n of nodes) {
    if (n.data.node_type_id !== NODE_IDS.AI_PROMPT && n.data.node_type_id !== NODE_IDS.AI_AGENT) continue;
    const cfg = n.data.config;
    const provider = typeof cfg.provider === "string" && cfg.provider !== "" ? cfg.provider : "auto";
    const baseUrl = typeof cfg.base_url === "string" ? cfg.base_url : "";
    const model = typeof cfg.model === "string" ? cfg.model : "";
    const credentialId = n.data.credentials?.["api_key"] ?? "";
    const local = isLocalEndpoint(provider, baseUrl);
    if (!local && credentialId === "") continue;
    if (model.trim() === "") continue;
    out.push({ nodeId: n.data.id, label: `${n.data.name} (${model})`, provider, model, baseUrl, credentialId, local });
  }
  return out;
}

export function providerNotice(p: DescribeProvider): string {
  return p.local
    ? "This runs on a model on this computer or network. Nothing leaves it."
    : "This is sent over the internet to your AI provider, using the saved connection on that node.";
}

/** One-step workflow that runs a single AI Prompt with the chosen provider. No key text is ever put in it. */
export function buildDescribeWorkflow(
  provider: DescribeProvider,
  source: { ports: unknown; input_schema: unknown; output_schema: unknown },
  system: string,
  prompt: string,
  workflowId: string,
): string {
  const config: Obj = {
    provider: provider.provider,
    model: provider.model,
    system,
    prompt,
    temperature: 0.2,
    max_tokens: 600,
    timeout_secs: Math.round(DESCRIBE_TIMEOUT_MS / 1000),
  };
  if (provider.baseUrl !== "") config.base_url = provider.baseUrl;
  const base = { schema_version: "1.0", id: workflowId, name: "Describe it" };
  return JSON.stringify({
    ...base,
    nodes: [
      {
        id: "describe_trigger", node_type_id: NODE_IDS.MANUAL_TRIGGER, node_type: "action", name: "Describe Trigger",
        config: {}, credentials: {}, position: { x: 0, y: 0 },
        ports: { inputs: [], outputs: [{ id: "output", label: "Start", position: "right" }] },
        input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 0 }, fallback_node: null,
      },
      {
        id: "describe_ai", node_type_id: NODE_IDS.AI_PROMPT, node_type: "ai", name: "Describe",
        config, credentials: provider.credentialId === "" ? {} : { api_key: provider.credentialId },
        position: { x: 300, y: 0 }, ports: source.ports,
        input_schema: source.input_schema, output_schema: source.output_schema,
        retry: { max_attempts: 1, backoff_ms: 0 }, fallback_node: null,
      },
    ],
    edges: [{ id: "e_describe", from_node: "describe_trigger", from_port: "output", to_node: "describe_ai", to_port: "input" }],
  });
}

export const DESCRIBE_NODE_ID = "describe_ai";

/** Plain-language reason for a failed describe run. */
export function describeFailure(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err ?? "");
  if (/timed out/i.test(raw)) return "The model took too long to answer. Try again, or pick a faster model.";
  const m = raw.match(/^([A-Z_]+):\s*(.+)$/s);
  const body = (m?.[2] ?? raw).trim();
  return body === "" ? "The model could not answer. Try again." : `The model could not answer: ${body.slice(0, 200)}`;
}
