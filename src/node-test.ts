/**
 * Shared preparation for testing one node together with its earlier steps:
 * decide which earlier steps can reuse recorded results, ask before any
 * step that acts outside the app runs for real, and record what the run
 * produced. Used by the node popover's Test button and the canvas "Run node".
 */

import type { CanvasNode } from "./canvas/Node";
import type { Connector } from "./canvas/Connector";
import { PRESET_NODE_OUTPUTS_KEY } from "./ipc/workflow";
import { mergeRunOutputs } from "./data-sources";
import { checkDangerousNodes } from "./validation";
import { sideEffectOf } from "./side-effects";
import {
  buildTestPlan, hashGraph, recordOutputs, useCachedResults,
  type CacheGraph, type GraphHashes, type TestPlan,
} from "./preset-cache";

export type ConfirmFn = (
  message: string,
  isDanger: boolean,
  okLabel?: string,
  iconVariant?: "danger" | "neutral",
) => Promise<boolean>;

export interface NodeTestGraph extends CacheGraph {
  nodes: Map<string, CanvasNode>;
  connectors: Map<string, Connector>;
}

export type NodeTestPrep =
  | { ok: false }
  | {
      ok: true;
      plan: TestPlan;
      hashes: GraphHashes;
      /** The target plus its ancestors, ready to serialize. */
      nodes: Map<string, CanvasNode>;
      connectors: NodeTestGraph["connectors"];
      /** Variables to pass to the run. */
      vars: Record<string, unknown>;
    };

export async function planNodeTest(
  graph: NodeTestGraph,
  targetId: string,
  useCache: boolean = useCachedResults(),
): Promise<{ plan: TestPlan; hashes: GraphHashes }> {
  const hashes = await hashGraph(graph);
  return { plan: buildTestPlan(graph, targetId, hashes, useCache), hashes };
}

function sideEffectLines(plan: TestPlan, graph: NodeTestGraph): string[] {
  const lines: string[] = [];
  for (const step of plan.willRun) {
    const node = graph.nodes.get(step.id);
    const effect = node ? sideEffectOf(step.typeId, node.data.config as Record<string, unknown> | undefined) : null;
    if (effect) lines.push(`${step.name}: ${effect.label}`);
  }
  return lines;
}

export async function prepareNodeTest(args: {
  graph: NodeTestGraph;
  targetId: string;
  workflowId: string;
  approved: Set<string>;
  confirm: ConfirmFn;
  useCache?: boolean;
}): Promise<NodeTestPrep> {
  const { graph, targetId, workflowId, approved, confirm } = args;
  const target = graph.nodes.get(targetId);
  if (!target) return { ok: false };

  const { plan, hashes } = await planNodeTest(graph, targetId, args.useCache);

  const toRun = [target, ...plan.willRun.map(s => graph.nodes.get(s.id)!)];
  if (!await checkDangerousNodes(workflowId, toRun, approved, confirm)) return { ok: false };

  const lines = sideEffectLines(plan, graph);
  if (lines.length) {
    const message =
      `Testing "${target.data.name}" will run these earlier steps for real:\n\n${lines.join("\n")}\n\nContinue?`;
    if (!await confirm(message, false, "Run them", "neutral")) return { ok: false };
  }

  const keep = new Set<string>([targetId, ...plan.ancestors.map(s => s.id)]);
  const nodes = new Map([...graph.nodes].filter(([id]) => keep.has(id)));
  const connectors = new Map([...graph.connectors].filter(([, c]) =>
    keep.has(c.data.from_node) && keep.has(c.data.to_node)));
  const vars = plan.cachedSteps.length ? { [PRESET_NODE_OUTPUTS_KEY]: plan.presets } : {};
  return { ok: true, plan, hashes, nodes, connectors, vars };
}

/** Store what a finished run produced. Best effort: a failure never affects the run. */
export function recordFinishedRun(
  hashes: GraphHashes,
  outputs: Record<string, unknown> | null | undefined,
  reused: Iterable<string> = [],
): void {
  try {
    recordOutputs(hashes, outputs, new Set(reused));
    mergeRunOutputs(outputs);
  } catch (e) {
    console.warn("Could not record run results for reuse:", e);
  }
}
