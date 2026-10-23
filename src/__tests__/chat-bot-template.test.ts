import { describe, it, expect } from "vitest";
import { buildChatBotWorkflow } from "../chat/chat-bot-template";
import { checkChatReadiness } from "../chat/readiness";
import type { NodeDescriptor } from "../ipc/workflow";

const descriptor = (type_id: string, display_name: string, node_type: NodeDescriptor["node_type"]): NodeDescriptor => ({
  type_id, display_name, node_type, version: "1",
  input_schema: { type: "object" }, output_schema: { type: "object" },
  ports: { inputs: [{ id: "input", label: "In", position: "left" }], outputs: [{ id: "output", label: "Out", position: "right" }] },
});

const registry = new Map([
  ["webhook",   descriptor("webhook",   "Chat In",   "action")],
  ["ai_prompt", descriptor("ai_prompt", "AI Prompt", "ai")],
  ["output",    descriptor("output",    "Output",    "utility")],
]);

describe("buildChatBotWorkflow", () => {
  it("builds a Webhook → AI Prompt → Output chain that Chat reports no problems with", () => {
    const wf = buildChatBotWorkflow(id => registry.get(id))!;
    const nodes = wf.nodes as Array<{ id: string; name: string; node_type_id: string; config: Record<string, unknown> }>;
    const edges = wf.edges as Array<{ from_node: string; to_node: string; from_port: string; to_port: string }>;

    expect(nodes.map(n => n.node_type_id)).toEqual(["webhook", "ai_prompt", "output"]);
    expect(nodes[1].config.prompt).toBe("{{Chat In.output.body.message}}");
    expect(edges.map(e => `${e.from_node}.${e.from_port}->${e.to_node}.${e.to_port}`))
      .toEqual(["node_chat_in.output->node_chat_ai.input", "node_chat_ai.output->node_chat_out.input"]);
    expect(checkChatReadiness(
      nodes.map(n => ({ id: n.id, name: n.name, type: n.node_type_id, config: n.config })),
      edges,
      { checkFiles: false },
    )).toEqual([]);
  });

  it("returns null when a needed node type is not registered", () => {
    expect(buildChatBotWorkflow(id => (id === "output" ? undefined : registry.get(id)))).toBeNull();
  });
});
