/// <reference types="vite/client" />
import { firstSentence } from "./node-descriptions";
import { NODE_IDS } from "./node-ids";

export type Difficulty = "Beginner" | "Intermediate" | "Advanced";

export interface TemplateInfo {
  id: string;
  title: string;
  summary: string;
  difficulty: Difficulty;
  needs: string[];
  workflow: Record<string, unknown>;
}

interface RawNode { node_type_id?: unknown; config?: { provider?: unknown } }

const ADVANCED_TYPES: ReadonlySet<string> = new Set([
  NODE_IDS.AI_AGENT, NODE_IDS.LOOP, NODE_IDS.SWITCH, NODE_IDS.CODE, NODE_IDS.SHELL_EXEC, NODE_IDS.DATABASE,
]);
const AI_TYPES: ReadonlySet<string> = new Set([NODE_IDS.AI_PROMPT, NODE_IDS.AI_AGENT, NODE_IDS.IMAGE_GEN]);
const ACCOUNT_TYPES: ReadonlySet<string> = new Set([NODE_IDS.EMAIL_SEND, NODE_IDS.SOCIAL_UPLOAD]);
const KEYED_PROVIDERS: ReadonlySet<string> = new Set(["openai", "anthropic", "gemini"]);
const DIFFICULTY_ORDER: Record<Difficulty, number> = { Beginner: 0, Intermediate: 1, Advanced: 2 };

function nodesOf(workflow: Record<string, unknown>): RawNode[] {
  return Array.isArray(workflow.nodes) ? (workflow.nodes as RawNode[]) : [];
}

export function templateDifficulty(workflow: Record<string, unknown>): Difficulty {
  const nodes = nodesOf(workflow);
  if (nodes.length >= 10 || nodes.some(n => ADVANCED_TYPES.has(String(n.node_type_id)))) return "Advanced";
  return nodes.length <= 4 ? "Beginner" : "Intermediate";
}

export function templateNeeds(workflow: Record<string, unknown>): string[] {
  const needs = new Set<string>();
  for (const n of nodesOf(workflow)) {
    const type = String(n.node_type_id);
    const provider = String(n.config?.provider ?? "");
    if (AI_TYPES.has(type) && provider === "local") needs.add("Ollama");
    else if (AI_TYPES.has(type) && KEYED_PROVIDERS.has(provider)) needs.add("API key");
    else if (ACCOUNT_TYPES.has(type)) needs.add("Account credentials");
  }
  return [...needs];
}

export function describeTemplate(id: string, workflow: Record<string, unknown>): TemplateInfo | null {
  if (typeof workflow.name !== "string" || !Array.isArray(workflow.nodes) || !Array.isArray(workflow.edges)) return null;
  const description = typeof workflow.description === "string" ? workflow.description : "";
  return {
    id,
    title: workflow.name,
    summary: firstSentence(description),
    difficulty: templateDifficulty(workflow),
    needs: templateNeeds(workflow),
    workflow,
  };
}

export function buildGettingStartedWorkflow(): Record<string, unknown> {
  const now = new Date().toISOString();
  return {
    schema_version: "1.0",
    id: "example_getting_started",
    name: "Getting Started: Fetch and Show",
    description: "Fetch a test web page and show what came back.",
    nodes: [
      {
        id: "n1", node_type_id: NODE_IDS.MANUAL_TRIGGER, node_type: "action",
        name: "Start", position: { x: 120, y: 200 }, config: {},
        credentials: {}, retry: { max_attempts: 1, backoff_ms: 500 },
        fallback_node: null,
        input_schema:  { type: "object", properties: { mock_payload: { type: "string", description: "Optional JSON payload to inject when running manually" } } },
        output_schema: { type: "object" },
        ports: { inputs: [], outputs: [{ id: "output", label: "Start", position: "right" }] },
      },
      {
        id: "n2", node_type_id: NODE_IDS.HTTP_REQUEST, node_type: "action",
        name: "Fetch Data", position: { x: 380, y: 200 },
        config: { url: "https://httpbin.org/get", method: "GET" },
        credentials: {}, retry: { max_attempts: 3, backoff_ms: 500 },
        fallback_node: null,
        input_schema:  { type: "object", properties: { url: { type: "string" }, method: { type: "string", enum: ["GET", "POST", "PUT", "PATCH", "DELETE"] } } },
        output_schema: { type: "object" },
        ports: { inputs: [{ id: "input", label: "In", position: "left" }], outputs: [{ id: "output", label: "Success", position: "right" }, { id: "on_error", label: "Error", position: "right" }] },
      },
      {
        id: "n3", node_type_id: NODE_IDS.OUTPUT, node_type: "utility",
        name: "Show Result", position: { x: 640, y: 200 },
        config: { label: "HTTP Result" },
        credentials: {}, retry: { max_attempts: 1, backoff_ms: 500 },
        fallback_node: null,
        input_schema:  { type: "object", properties: { label: { type: "string" }, source_node: { type: "string" }, field: { type: "string" } } },
        output_schema: { type: "object" },
        ports: { inputs: [{ id: "input", label: "In", position: "left" }], outputs: [{ id: "output", label: "Out", position: "right" }] },
      },
    ],
    edges: [
      { id: "e1", from_node: "n1", from_port: "output", to_node: "n2", to_port: "input", condition: null, on_success: null, on_failure: null },
      { id: "e2", from_node: "n2", from_port: "output", to_node: "n3", to_port: "input", condition: null, on_success: null, on_failure: null },
    ],
    metadata: { author: "user", created_at: now, updated_at: now, version: "1.0.0", tags: [] },
  };
}

const BUNDLED_RAW = import.meta.glob("../examples/*.aerini", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

export function bundledTemplates(): TemplateInfo[] {
  const found: TemplateInfo[] = [];
  for (const [path, raw] of Object.entries(BUNDLED_RAW)) {
    try {
      const info = describeTemplate(`tpl_${path.split("/").pop() ?? path}`, JSON.parse(raw) as Record<string, unknown>);
      if (info) found.push(info);
    } catch {
      continue;
    }
  }
  return found;
}

export function getTemplates(): TemplateInfo[] {
  const builtin = describeTemplate("tpl_getting_started", buildGettingStartedWorkflow());
  const all = [...(builtin ? [builtin] : []), ...bundledTemplates()];
  return all.sort((a, b) => DIFFICULTY_ORDER[a.difficulty] - DIFFICULTY_ORDER[b.difficulty] || a.title.localeCompare(b.title));
}
