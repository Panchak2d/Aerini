/**
 * Which canvas nodes can be wired into a node from inside its data picker.
 * Pure: no DOM, no Tauri. A candidate is offered only when the wire would be
 * legal and useful: not the node itself, not already upstream, not downstream
 * (that wire would close a loop), and with a plain `output` port to read from.
 */

import { getUpstreamNodes, mainResultExpr } from "./data-sources";
import { validateNodeName } from "./node-names";

export const CONNECT_FROM_PORT = "output";
export const CONNECT_TO_PORT = "input";

export interface ConnectNode {
  disabled?: boolean;
  data: { id: string; name: string; node_type_id?: string; output_schema?: Record<string, unknown> };
  ports?: Array<{ id: string; isInput: boolean }>;
}

export interface ConnectGraph {
  connectors: Map<string, { data: { from_node: string; to_node: string } }>;
  nodes: Map<string, ConnectNode>;
}

export interface ConnectCandidate {
  id: string;
  name: string;
  /** Reference to the node's Main result, or the whole output when no single main field exists. */
  expr: string;
}

/** True when `to` can be reached from `from` by following wires forward. */
export function isDownstreamOf(graph: ConnectGraph, from: string, to: string): boolean {
  const seen = new Set<string>([from]);
  const queue = [from];
  while (queue.length) {
    const current = queue.shift()!;
    for (const conn of graph.connectors.values()) {
      const next = conn.data.to_node;
      if (conn.data.from_node !== current || seen.has(next)) continue;
      if (next === to) return true;
      seen.add(next);
      queue.push(next);
    }
  }
  return false;
}

function hasPort(node: ConnectNode, id: string, isInput: boolean): boolean {
  return (node.ports ?? []).some((p) => p.id === id && p.isInput === isInput);
}

/**
 * Nodes that could feed `targetId` but do not yet, in canvas order. `inputFree`
 * says whether a new wire can join the target's input without displacing an
 * existing one; when it cannot, nothing is offered.
 */
/** Longest "Not connected" list offered, so a large canvas cannot flood the picker. */
export const CONNECTABLE_MAX = 40;

export function listConnectable(
  graph: ConnectGraph,
  targetId: string,
  inputFree: (nodeId: string, portId: string) => boolean = () => true,
): ConnectCandidate[] {
  const target = graph.nodes.get(targetId);
  if (!target || !hasPort(target, CONNECT_TO_PORT, true) || !inputFree(targetId, CONNECT_TO_PORT)) return [];
  const upstream = new Set(getUpstreamNodes(graph, targetId).map((u) => u.id));
  const out: ConnectCandidate[] = [];
  for (const [id, node] of graph.nodes) {
    if (out.length >= CONNECTABLE_MAX) break;
    if (id === targetId || upstream.has(id)) continue;
    if (node.disabled || node.data.node_type_id === "note") continue;
    if (validateNodeName(node.data.name) !== null || !hasPort(node, CONNECT_FROM_PORT, false)) continue;
    if (isDownstreamOf(graph, targetId, id)) continue;
    const main = mainResultExpr({ name: node.data.name, schema: node.data.output_schema });
    out.push({ id, name: node.data.name, expr: main ? main.expr : `{{${node.data.name}.output}}` });
  }
  return out;
}
