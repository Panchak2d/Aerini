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
const out  = node("out", "Output", "output");

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
});
