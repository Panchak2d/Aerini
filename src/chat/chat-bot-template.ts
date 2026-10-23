import { NODE_IDS } from "../node-ids";
import type { NodeDescriptor } from "../ipc/workflow";
import { AI_PROMPT_REPLY_FIELD, chatMessageExpr } from "./chat-defaults";

export const CHAT_BOT_TEMPLATE_NAME = "Chat bot";

export const CHAT_BOT_NEXT_STEPS =
  "Chat bot ready. Double-click AI Prompt to pick a model, then open Chat and press Start.";

const NODE_GAP = 300;

function nodeFrom(d: NodeDescriptor, id: string, column: number, config: Record<string, unknown>): Record<string, unknown> {
  return {
    id,
    node_type_id:  d.type_id,
    node_type:     d.node_type,
    name:          d.display_name,
    config,
    credentials:   {},
    position:      { x: column * NODE_GAP, y: 0 },
    ports:         d.ports,
    input_schema:  d.input_schema,
    output_schema: d.output_schema,
    retry:         { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    disabled:      false,
    dynamic_ports: d.dynamic_ports ?? false,
  };
}

function wire(from: string, to: string): Record<string, unknown> {
  return { id: `e_${from}_${to}`, from_node: from, from_port: "output", to_node: to, to_port: "input", condition: null, on_success: null, on_failure: null };
}

/**
 * A Webhook → AI Prompt → Output workflow that Chat can talk to as soon as a
 * model is chosen. Built from the live node descriptors so its ports and
 * schemas always match the installed nodes. Returns null when a needed node
 * type isn't registered.
 */
export function buildChatBotWorkflow(lookup: (typeId: string) => NodeDescriptor | undefined): Record<string, unknown> | null {
  const hook = lookup(NODE_IDS.WEBHOOK);
  const ai   = lookup(NODE_IDS.AI_PROMPT);
  const out  = lookup(NODE_IDS.OUTPUT);
  if (!hook || !ai || !out) return null;

  return {
    id:          "wf_chat_bot_template",
    name:        CHAT_BOT_TEMPLATE_NAME,
    description: "Answers each message typed in Chat with an AI model.",
    nodes: [
      nodeFrom(hook, "node_chat_in", 0, { method: "POST", path: "/chat", port: 3456 }),
      nodeFrom(ai, "node_chat_ai", 1, {
        prompt: chatMessageExpr(hook.display_name),
        system: "You are a helpful assistant.",
      }),
      nodeFrom(out, "node_chat_out", 2, { field: AI_PROMPT_REPLY_FIELD }),
    ],
    edges: [wire("node_chat_in", "node_chat_ai"), wire("node_chat_ai", "node_chat_out")],
    metadata: { tags: [] },
  };
}
