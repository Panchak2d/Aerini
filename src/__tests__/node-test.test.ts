import { describe, it, expect, vi, beforeEach } from "vitest";
import type { CanvasNode } from "../canvas/Node";
import type { Connector } from "../canvas/Connector";
import { PRESET_NODE_OUTPUTS_KEY } from "../ipc/workflow";
import { clearPresetCache, hashGraph, recordOutputs } from "../preset-cache";
import { prepareNodeTest, type NodeTestGraph } from "../node-test";

function testGraph(emailConfig: Record<string, unknown> = {}): NodeTestGraph {
  const node = (id: string, name: string, type: string, config: Record<string, unknown> = {}) =>
    [id, { data: { id, name, node_type_id: type, config } } as unknown as CanvasNode] as const;
  const edge = (id: string, from: string, to: string) =>
    [id, { data: { from_node: from, from_port: "output", to_node: to, to_port: "input" } } as unknown as Connector] as const;
  return {
    nodes: new Map([
      node("m", "Mail", "email_send", emailConfig),
      node("s", "Summarize", "transform"),
      node("t", "Target", "transform"),
      node("x", "Elsewhere", "transform"),
    ]),
    connectors: new Map([edge("e1", "m", "s"), edge("e2", "s", "t")]),
  };
}

beforeEach(() => clearPresetCache());

describe("prepareNodeTest", () => {
  it("asks_before_running_an_earlier_step_that_sends_email_and_stops_when_declined", async () => {
    const confirm = vi.fn().mockResolvedValue(false);
    const prep = await prepareNodeTest({
      graph: testGraph(), targetId: "t", workflowId: "w", approved: new Set(), confirm,
    });
    expect(prep.ok).toBe(false);
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(confirm.mock.calls[0][0]).toContain("Mail: sends an email");
  });

  it("reused_steps_are_not_asked_about_and_only_the_test_chain_is_sent_to_the_engine", async () => {
    const g = testGraph();
    recordOutputs(await hashGraph(g), { m: { sent: true }, s: { content: "hi" } }, new Set(), 1);
    const confirm = vi.fn();
    const prep = await prepareNodeTest({
      graph: g, targetId: "t", workflowId: "w", approved: new Set(), confirm,
    });
    expect(confirm).not.toHaveBeenCalled();
    if (!prep.ok) throw new Error("expected a runnable test");
    expect(prep.vars).toEqual({ [PRESET_NODE_OUTPUTS_KEY]: { m: { sent: true }, s: { content: "hi" } } });
    expect([...prep.nodes.keys()].sort()).toEqual(["m", "s", "t"]);
    expect(prep.connectors.size).toBe(2);
  });
});
