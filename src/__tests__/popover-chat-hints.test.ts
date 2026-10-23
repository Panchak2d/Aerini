/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { renderAiNodeNotices } from "../popover/extensions/ai-prompt";
import { renderOutputHint } from "../popover/extensions/output";
import type { ExtensionContext } from "../node-configs/popover-utils";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), convertFileSrc: vi.fn((p: string) => p) }));

function render(fn: (ctx: ExtensionContext) => void, typeId: string, config: Record<string, unknown>): string[] {
  const body = document.createElement("div");
  fn({ node: { data: { node_type_id: typeId, config } }, body } as unknown as ExtensionContext);
  return Array.from(body.children).map(c => c.textContent ?? "");
}

describe("chat hints in node popovers", () => {
  it("tells a blank AI Prompt or Agent how to read the Chat message, and stays quiet once filled", () => {
    expect(render(renderAiNodeNotices, "ai_prompt", {}).some(t => t.includes("{{Webhook.output.body.message}}"))).toBe(true);
    expect(render(renderAiNodeNotices, "ai_agent", { goal: " " }).some(t => t.includes("{{Webhook.output.body.message}}"))).toBe(true);
    expect(render(renderAiNodeNotices, "ai_prompt", { prompt: "Hi" }).some(t => t.includes("Answering Chat messages"))).toBe(false);
  });

  it("tells a blank Output to use content, and stays quiet once a Field is set", () => {
    expect(render(renderOutputHint, "output", {}).join(" ")).toContain("content");
    expect(render(renderOutputHint, "output", { field: "content" })).toEqual([]);
  });
});
