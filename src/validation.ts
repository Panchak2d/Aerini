import type { Canvas } from "./canvas/Canvas";
import type { CanvasNode } from "./canvas/Node";
import { NODE_IDS, TRIGGER_NODE_IDS, DANGEROUS_NODE_IDS } from "./node-ids";

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
    TRIGGER_NODE_IDS.has(n.data.node_type_id as string)
  );
  if (!hasTrigger) {
    errors.push("No trigger node found. Add a Manual Trigger, Webhook, or Schedule.");
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
    .filter(n => DANGEROUS_NODE_IDS.has(n.data.node_type_id as string));
  if (!dangerousNodes.length) return true;

  const dangerousIds  = dangerousNodes.map(n => n.data.id).sort().join(",");
  const approvalKey   = `${workflowId}::${dangerousIds}`;
  if (approvedForExecution.has(approvalKey)) return true;

  const nodeTypes = dangerousNodes
    .map(n => n.data.name || n.data.node_type_id)
    .join(", ");
  const confirmed = await confirm(
    `This workflow contains nodes that execute code on your computer:\n\n${nodeTypes}\n\nOnly run workflows from sources you trust. Continue?`,
    true,
  );
  if (confirmed) { approvedForExecution.add(approvalKey); return true; }
  return false;
}
