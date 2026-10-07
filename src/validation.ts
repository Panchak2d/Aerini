import type { Canvas } from "./canvas/Canvas";
import type { CanvasNode } from "./canvas/Node";
import { NODE_IDS } from "./node-ids";
import { getNodeDescriptor, isDangerousNodeType, isTriggerNodeType } from "./canvas/node-registry";
import { canonicalJson } from "./canonical-json";

// `database` is deliberately absent here — its required fields depend on
// `db_type` (sqlite needs db_path+query; postgres/mysql need
// connection_url+query; redis needs connection_url+operation+key) and can't
// be expressed as one flat list. See the NODE_IDS.DATABASE branch in
// validateWorkflow below, which mirrors sqlite.rs/postgres.rs/redis.rs's own
// MISSING_PATH/MISSING_URL/MISSING_QUERY/MISSING_OPERATION/MISSING_KEY checks.
export const REQUIRED_FIELDS: Record<string, string[]> = {
  [NODE_IDS.HTTP_REQUEST]: ["url", "method"],
  email_send:              ["smtp_host", "from", "to", "subject", "body"],
  [NODE_IDS.SHELL_EXEC]:   ["command"],
  [NODE_IDS.CODE]:         ["code"],
  [NODE_IDS.AI_PROMPT]:    ["prompt"],
  [NODE_IDS.AI_AGENT]:     ["goal", "provider"],
  [NODE_IDS.SCHEDULE]:     ["mode"],
};

export function validateWorkflow(canvas: Canvas): string[] {
  const errors: string[] = [];
  const nodes = canvas.nodes;

  if (nodes.size === 0) {
    errors.push("Canvas is empty — add some nodes first.");
    return errors;
  }

  const hasTrigger = [...nodes.values()].some(n =>
    isTriggerNodeType(n.data.node_type_id as string)
  );
  if (!hasTrigger) {
    errors.push("No trigger node found. Add a Manual Trigger, Webhook, Schedule, or a trigger plugin.");
  }

  for (const node of nodes.values()) {
    if (node.data.node_type_id === NODE_IDS.DATABASE) {
      const dbType = String(node.data.config["db_type"] ?? "sqlite");
      const dbRequired =
        dbType === "postgres" || dbType === "mysql" ? ["connection_url", "query"]
        : dbType === "redis"                        ? ["connection_url", "operation", "key"]
        :                                              ["db_path", "query"]; // sqlite (default)
      for (const field of dbRequired) {
        const val = node.data.config[field];
        if (!val || String(val).trim() === "") {
          errors.push(`"${node.data.name}" — ${field.replace(/_/g, " ")} is required.`);
        }
      }
      continue;
    }

    const required = REQUIRED_FIELDS[node.data.node_type_id];
    if (!required) continue;
    for (const field of required) {
      const val = node.data.config[field];
      if (!val || String(val).trim() === "") {
        errors.push(`"${node.data.name}" — ${field.replace(/_/g, " ")} is required.`);
      }
    }
  }

  return errors;
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
