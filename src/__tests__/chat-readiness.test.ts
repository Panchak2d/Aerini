import { describe, it, expect } from "vitest";
import { checkChatReadiness, type ChatGraphEdge, type ChatGraphNode } from "../chat/readiness";

const node = (id: string, name: string, type: string, config: Record<string, unknown> = {}): ChatGraphNode =>
  ({ id, name, type, config });
const edge = (from: string, to: string, toPort = "input"): ChatGraphEdge =>
  ({ from_node: from, to_node: to, to_port: toPort });

const hook = node("hook", "Webhook", "webhook");
const mem  = node("mem", "AI Memory (read)", "ai_memory", { session_id: "{{Webhook.output.body.session_id}}" });
const ai   = (config: Record<string, unknown> = {}) => node("ai", "AI Prompt", "ai_prompt", {
  prompt: "History: {{AI Memory (read).output.messages}} User: {{Webhook.output.body.message}}",
  ...config,
});
const out  = node("out", "Output", "output", { field: "content" });

describe("checkChatReadiness", () => {
  it("reports nothing for a fully wired chat workflow", () => {
    const issues = checkChatReadiness(
      [hook, mem, ai({ attachments_expr: "{{Webhook.output.files}}" }), out],
      [edge("hook", "mem"), edge("mem", "ai"), edge("hook", "ai", "attachments"), edge("ai", "out")],
      { checkFiles: true },
    );
    expect(issues).toEqual([]);
  });

  it("offers the downstream AI node when files are on but unread, and says nothing when files are off", () => {
    const nodes = [hook, mem, ai(), out];
    const edges = [edge("hook", "mem"), edge("mem", "ai"), edge("ai", "out")];
    expect(checkChatReadiness(nodes, edges, { checkFiles: true })).toEqual([
      { kind: "files_unread", webhook: { id: "hook", name: "Webhook" }, candidates: [{ id: "ai", name: "AI Prompt" }] },
    ]);
    expect(checkChatReadiness(nodes, edges, { checkFiles: false })).toEqual([]);
  });

  it("treats a Files port fed by the wrong node as unread", () => {
    const issues = checkChatReadiness(
      [hook, mem, ai({ attachments_expr: "{{AI Memory (read).output}}" }), out],
      [edge("hook", "mem"), edge("mem", "ai", "attachments"), edge("ai", "out")],
      { checkFiles: true },
    );
    expect(issues.map(i => i.kind)).toEqual(["files_unread"]);
  });

  it("flags a node that reads the Webhook but isn't connected after it, and a node that reads it from an unconnected source", () => {
    const issues = checkChatReadiness(
      [hook, mem, ai({ attachments_expr: "{{Webhook.output.files}}" }), out],
      [edge("hook", "ai", "attachments"), edge("ai", "out")],
      { checkFiles: true },
    );
    expect(issues).toEqual([
      { kind: "not_connected", from: { id: "hook", name: "Webhook" }, to: { id: "mem", name: "AI Memory (read)" }, fixable: true },
      { kind: "not_connected", from: { id: "mem", name: "AI Memory (read)" }, to: { id: "ai", name: "AI Prompt" }, fixable: true },
    ]);
  });

  it("marks a broken reference unfixable when the reader's input is taken or connecting would loop", () => {
    const taken = checkChatReadiness(
      [hook, mem, node("other", "Other", "code"), out],
      [edge("other", "mem")],
      { checkFiles: false },
    );
    expect(taken).toEqual([
      { kind: "not_connected", from: { id: "hook", name: "Webhook" }, to: { id: "mem", name: "AI Memory (read)" }, fixable: false },
    ]);

    const a = node("a", "A", "code", { x: "{{B.output.v}}" });
    const b = node("b", "B", "code");
    const loop = checkChatReadiness([hook, a, b], [edge("a", "b")], { checkFiles: false });
    expect(loop).toEqual([{ kind: "not_connected", from: { id: "b", name: "B" }, to: { id: "a", name: "A" }, fixable: false }]);
  });

  it("does not mistake a node whose name ends a longer node's name for a reference", () => {
    const tail = node("tail", "Memory (read)", "ai_memory");
    const issues = checkChatReadiness(
      [hook, node("full", "AI Memory (read)", "ai_memory"), tail, node("r", "Reader", "code", { x: "{{AI Memory (read).output.messages}}" })],
      [edge("hook", "full"), edge("full", "r")],
      { checkFiles: false },
    );
    expect(issues).toEqual([]);
  });

  it("returns nothing when the workflow has no Webhook", () => {
    expect(checkChatReadiness([ai(), out], [edge("ai", "out")], { checkFiles: true })).toEqual([]);
  });

  const reply = (config: Record<string, unknown> = { prompt: "{{Webhook.output.body.message}}" }) =>
    node("ai", "AI Prompt", "ai_prompt", config);
  const chain = [edge("hook", "ai"), edge("ai", "out")];

  it("flags an AI node after the Webhook with a blank prompt, using the Agent's own field for an Agent", () => {
    const agent = node("agent", "AI Agent", "ai_agent", { goal: "  " });
    expect(checkChatReadiness([hook, reply({ prompt: "" }), out], chain, { checkFiles: false })).toEqual([
      { kind: "prompt_unset", webhook: { id: "hook", name: "Webhook" }, node: { id: "ai", name: "AI Prompt" }, field: "prompt" },
    ]);
    expect(checkChatReadiness([hook, agent, out], [edge("hook", "agent"), edge("agent", "out")], { checkFiles: false })).toEqual([
      { kind: "prompt_unset", webhook: { id: "hook", name: "Webhook" }, node: { id: "agent", name: "AI Agent" }, field: "goal" },
    ]);
  });

  it("does not flag a blank prompt on an AI node the Webhook never reaches", () => {
    expect(checkChatReadiness([hook, reply({}), out], [edge("ai", "out")], { checkFiles: false })).toEqual([]);
  });

  it("flags the Output when it shows an AI node's whole result, and not once a Field or Source Node is chosen", () => {
    const raw = node("out", "Output", "output");
    expect(checkChatReadiness([hook, reply(), raw], chain, { checkFiles: false })).toEqual([
      { kind: "reply_raw", output: { id: "out", name: "Output" }, source: { id: "ai", name: "AI Prompt" }, field: "content" },
    ]);
    for (const config of [{ field: "content" }, { source_node: "ai" }]) {
      expect(checkChatReadiness([hook, reply(), node("out", "Output", "output", config)], chain, { checkFiles: false })).toEqual([]);
    }
  });

  it("does not flag an Output fed by a non-AI node or by an AI node's error port", () => {
    const raw  = node("out", "Output", "output");
    const code = node("code", "Code", "code");
    expect(checkChatReadiness([hook, code, raw], [edge("hook", "code"), edge("code", "out")], { checkFiles: false })).toEqual([]);
    const viaError = [edge("hook", "ai"), { ...edge("ai", "out"), from_port: "on_error" }];
    expect(checkChatReadiness([hook, reply(), raw], viaError, { checkFiles: false })).toEqual([]);
  });
});
