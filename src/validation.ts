import type { Canvas } from "./canvas/Canvas";
import type { CanvasNode, CanvasNodeData } from "./canvas/Node";
import { NODE_IDS } from "./node-ids";
import { getNodeDescriptor, isDangerousNodeType, isTriggerNodeType, isUnregisteredNodeType } from "./canvas/node-registry";
import { canonicalJson } from "./canonical-json";
import { collectReferenceProblems, collectRuntimeProblems } from "./problems";
import type { Problem, ProblemGraph } from "./problems";

// Fallback for nodes whose schema declares no `required` list. `database` is
// deliberately absent: its required fields depend on `db_type` (sqlite needs
// db_path+query; postgres/mysql need connection_url+query; redis needs
// connection_url+operation+key), so requiredFieldsFor special-cases it to
// mirror sqlite.rs/postgres.rs/redis.rs's own MISSING_* checks.
export const REQUIRED_FIELDS: Record<string, string[]> = {
  [NODE_IDS.HTTP_REQUEST]: ["url", "method"],
  email_send:              ["smtp_host", "from", "to", "subject", "body"],
  [NODE_IDS.SHELL_EXEC]:   ["command"],
  [NODE_IDS.CODE]:         ["code"],
  [NODE_IDS.AI_PROMPT]:    ["prompt"],
  [NODE_IDS.AI_AGENT]:     ["goal", "provider"],
  [NODE_IDS.SCHEDULE]:     ["mode"],
};

function isBlankValue(v: unknown): boolean {
  return v === undefined || v === null || (typeof v === "string" && v.trim() === "");
}

/** Config keys a node must have filled in before it can run: the node's own schema `required` list, or the fallback list when the schema declares none. */
export function requiredFieldsFor(data: CanvasNodeData): string[] {
  if (data.node_type_id === NODE_IDS.DATABASE) {
    const dbType = String(data.config["db_type"] ?? "sqlite");
    return dbType === "postgres" || dbType === "mysql" ? ["connection_url", "query"]
      : dbType === "redis"                              ? ["connection_url", "operation", "key"]
      :                                                    ["db_path", "query"];
  }
  const declared = (data.input_schema as { required?: unknown } | undefined)?.required;
  if (Array.isArray(declared)) {
    const keys = declared.filter((k): k is string => typeof k === "string");
    if (keys.length) return keys;
  }
  return REQUIRED_FIELDS[data.node_type_id] ?? [];
}

// A required key that the node also accepts from another config key.
const ALTERNATIVE_FIELD: Record<string, Record<string, string>> = {
  text_splitter: { text: "source_field" },
};

/** Required config keys that are blank and not supplied by a saved credential or an accepted alternative key. */
export function blankRequiredFields(data: CanvasNodeData): string[] {
  const creds = data.credentials ?? {};
  const alternatives = ALTERNATIVE_FIELD[data.node_type_id] ?? {};
  return requiredFieldsFor(data).filter(f =>
    !(f in creds)
    && isBlankValue(data.config[f])
    && !(alternatives[f] && !isBlankValue(data.config[alternatives[f]])));
}

export function validateWorkflow(canvas: Canvas): string[] {
  const errors: string[] = [];
  const nodes = canvas.nodes;

  if (nodes.size === 0) {
    errors.push("Canvas is empty \u2014 add some nodes first.");
    return errors;
  }

  const hasTrigger = [...nodes.values()].some(n =>
    isTriggerNodeType(n.data.node_type_id as string)
  );
  if (!hasTrigger) {
    errors.push("No trigger node found. Add a Manual Trigger, Webhook, Schedule, or a trigger plugin.");
  }

  for (const node of nodes.values()) {
    if (node.disabled || node.data.node_type_id === NODE_IDS.NOTE) continue;
    if (isUnregisteredNodeType(node.data.node_type_id as string)) {
      errors.push(`"${node.data.name}" (${node.data.node_type_id}) isn't available in this version of Aerini. Update the app, or remove the node.`);
      continue;
    }
    for (const field of blankRequiredFields(node.data)) {
      errors.push(requiredMessage(node.data.name, field));
    }
  }

  return errors;
}

function requiredMessage(nodeName: string, field: string): string {
  return `"${nodeName}" \u2014 ${field.replace(/_/g, " ")} is required.`;
}

/** Every problem the canvas shows before a run: blank required fields (errors), then reference and name problems and last-run warnings (warnings). */
export function collectProblems(canvas: Canvas): Problem[] {
  const graph: ProblemGraph = { nodes: canvas.nodes, connectors: canvas.connectors };
  const required: Problem[] = [];
  for (const node of canvas.nodes.values()) {
    if (node.disabled || node.data.node_type_id === NODE_IDS.NOTE) continue;
    if (isUnregisteredNodeType(node.data.node_type_id as string)) continue;
    for (const field of blankRequiredFields(node.data)) {
      required.push({
        key: `req:${node.data.id}:${field}`, nodeId: node.data.id, nodeName: node.data.name,
        severity: "error", kind: "required", field,
        message: `${field.replace(/_/g, " ")} is required.`,
      });
    }
  }
  return [...required, ...collectReferenceProblems(graph), ...collectRuntimeProblems(graph)];
}

async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(digest), b => b.toString(16).padStart(2, "0")).join("");
}

// An approval covers each dangerous node's id, type, enabled state, config
// and credential references: they decide what runs, so any change to one
// yields a different key and the user is asked again. Config can hold
// secrets (connection URLs, API keys), so only a digest is kept as the key,
// never the values. SHA-256 because a workflow file's author controls the
// new config and must not be able to craft one that collides with an
// approved one.
async function approvalKeyFor(workflowId: string, dangerousNodes: CanvasNode[]): Promise<string> {
  const entries = dangerousNodes
    .map(n => canonicalJson({
      id:          n.data.id,
      type:        n.data.node_type_id,
      disabled:    n.data.disabled ?? false,
      config:      n.data.config,
      credentials: n.data.credentials,
    }))
    .sort();
  return `${workflowId}::${await sha256Hex(JSON.stringify(entries))}`;
}

// approvedForExecution is session-scoped and owned by the caller (app.ts /
// RunManager). Passing it in avoids hidden module-level state while keeping
// the Set encapsulated per-caller.
//
// `nodes` is the exact set of nodes about to be executed — the whole canvas
// for a normal Run, or just the ancestor subgraph for a single-node test run
// (see run-manager/stream-handler.ts::handleRunSingleNode). Scoping to the
// actual execution set means a dangerous node elsewhere in the workflow,
// outside what's about to run, never triggers this warning.
export async function checkDangerousNodes(
  workflowId: string,
  nodes: Iterable<CanvasNode>,
  approvedForExecution: Set<string>,
  confirm: (msg: string, isDanger: boolean) => Promise<boolean>,
): Promise<boolean> {
  const dangerousNodes = [...nodes]
    .filter(n => isDangerousNodeType(n.data.node_type_id as string));
  if (!dangerousNodes.length) return true;

  const approvalKey = await approvalKeyFor(workflowId, dangerousNodes);
  if (approvedForExecution.has(approvalKey)) return true;

  // A node's name is user- or import-controlled, so the type is always shown
  // alongside it; a name alone could disguise a Shell node as "Weather lookup".
  const nodeTypes = dangerousNodes
    .map(n => {
      const typeId = n.data.node_type_id as string;
      const kind   = getNodeDescriptor(typeId)?.display_name ?? typeId;
      return n.data.name && n.data.name !== kind ? `${n.data.name} (${kind})` : kind;
    })
    .join(", ");
  const confirmed = await confirm(
    `This workflow contains nodes that execute code on your computer:\n\n${nodeTypes}\n\nOnly run workflows from sources you trust. Continue?`,
    true,
  );
  if (confirmed) { approvedForExecution.add(approvalKey); return true; }
  return false;
}
