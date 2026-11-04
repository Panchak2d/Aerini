// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

const run = vi.fn();
vi.mock("../ipc/workflow", () => ({
  runWorkflow: (...a: unknown[]) => run(...a),
  cancelRun: vi.fn(() => Promise.resolve()),
}));

import { mountDescribeIt } from "../popover/describe-panel";
import { setRunOutputs } from "../data-sources";

function setup(withProvider: boolean) {
  const nodes = new Map<string, any>();
  nodes.set("up", { data: { id: "up", name: "Feed", node_type_id: "feed_parser", config: {}, ports: {}, input_schema: {}, output_schema: { type: "object", properties: { digest: { type: "string" } } } } });
  nodes.set("cur", { data: { id: "cur", name: "Notify", node_type_id: "notification", config: {}, ports: {}, input_schema: {} } });
  if (withProvider) {
    nodes.set("ai", { data: { id: "ai", name: "Ask", node_type_id: "ai_prompt", config: { provider: "auto", model: "llama3", base_url: "http://localhost:11434/v1" }, credentials: {}, ports: {}, input_schema: {}, output_schema: {} } });
  }
  const graph = { nodes, connectors: new Map([["e", { data: { from_node: "up", to_node: "cur" } }]]) };
  const host = document.createElement("div");
  const bar = document.createElement("div");
  const field = document.createElement("textarea");
  host.append(field, bar);
  document.body.appendChild(host);
  const onInput = vi.fn();
  field.addEventListener("input", onInput);
  mountDescribeIt(field, host, bar, "cur", () => graph as never);
  return { host, field, onInput, button: bar.querySelector<HTMLButtonElement>(".field-describe")! };
}

const q = (host: HTMLElement, sel: string) => host.querySelector<HTMLElement>(sel)!;

beforeEach(() => {
  document.body.innerHTML = "";
  run.mockReset();
  setRunOutputs({ up: { digest: "hello" } });
});

describe("describe it panel", () => {
  it("explains_what_is_missing_instead_of_failing_when_no_model_is_set_up", () => {
    const { host, button } = setup(false);
    button.click();
    expect(q(host, ".field-describe-note").textContent).toContain("needs an AI model");
    expect(host.textContent).toContain("Open Credentials");
    expect(run).not.toHaveBeenCalled();
  });

  it("shows_what_will_be_sent_and_sends_nothing_until_propose", () => {
    const { host, button } = setup(true);
    button.click();
    expect(q(host, ".field-describe-pre").textContent).toContain("{{Feed.output.digest}}");
    expect(q(host, ".field-describe-pre").textContent).not.toContain("hello");
    expect(run).not.toHaveBeenCalled();
  });

  it("applies_a_validated_proposal_only_after_use_and_never_on_cancel", async () => {
    run.mockResolvedValue({ success: true, node_outputs: { describe_ai: { content: "Summary {{Feed.output.digest}} {{Bad.output.x}}" } }, logs: [] });
    const { host, field, onInput, button } = setup(true);
    button.click();
    const input = q(host, ".field-describe-input") as HTMLTextAreaElement;
    input.value = "the digest";
    input.dispatchEvent(new Event("input"));
    q(host, ".field-describe-primary").click();
    await vi.waitFor(() => expect(host.querySelector(".field-describe-proposal")).not.toBeNull());
    expect(field.value).toBe("");
    expect(q(host, ".field-describe-proposal").textContent).toBe("Summary {{Feed.output.digest}}");
    expect(host.textContent).toContain("{{Bad.output.x}}");
    q(host, ".field-describe-actions .field-describe-primary").click();
    expect(field.value).toBe("Summary {{Feed.output.digest}}");
    expect(onInput).toHaveBeenCalledTimes(1);
  });
});
