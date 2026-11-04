import { beforeEach, describe, expect, it } from "vitest";
import { setRunOutputs } from "../data-sources";
import {
  buildDescribeContext,
  buildDescribeWorkflow,
  describePromptText,
  listDescribeProviders,
  parseProposal,
} from "../describe-field";

function graph() {
  const nodes = new Map<string, { data: { id: string; name: string; output_schema?: Record<string, unknown> } }>();
  nodes.set("up", { data: { id: "up", name: "Feed", output_schema: { type: "object", properties: { digest: { type: "string" } } } } });
  nodes.set("cur", { data: { id: "cur", name: "Notify" } });
  return { nodes, connectors: new Map([["e", { data: { from_node: "up", to_node: "cur" } }]]) };
}

beforeEach(() => setRunOutputs({ up: { digest: "secret-looking sample text" } }));

describe("describe context", () => {
  it("default_context_lists_names_and_types_but_never_sample_values", () => {
    const ctx = buildDescribeContext(graph(), "cur", false);
    const text = describePromptText("the digest", ctx);
    expect(text).toContain("{{Feed.output.digest}} (string)");
    expect(text).not.toContain("secret-looking");
  });

  it("sample_values_are_included_only_when_asked", () => {
    const ctx = buildDescribeContext(graph(), "cur", true);
    expect(describePromptText("the digest", ctx)).toContain("secret-looking sample text");
  });
});

describe("describe reply validation", () => {
  const valid = new Set(["Feed.output", "Feed.output.digest"]);

  it("keeps_known_references_and_drops_invented_ones", () => {
    const p = parseProposal("Digest: {{ Feed.output.digest }} and {{Feed.output.invented}}", valid);
    expect(p.ok).toBe(true);
    expect(p.text).toBe("Digest: {{Feed.output.digest}} and");
    expect(p.dropped).toEqual(["Feed.output.invented"]);
  });

  it("rejects_a_reply_that_only_contains_invented_data_or_nothing", () => {
    expect(parseProposal("{{Nope.output.x}}", valid).ok).toBe(false);
    expect(parseProposal("   ", valid).ok).toBe(false);
    expect(parseProposal(undefined, valid).ok).toBe(false);
  });

  it("strips_a_code_fence_around_the_reply", () => {
    expect(parseProposal("```\n{{Feed.output.digest}}\n```", valid).text).toBe("{{Feed.output.digest}}");
  });
});

describe("describe providers", () => {
  const node = (id: string, config: Record<string, unknown>, credentials: Record<string, string> = {}) =>
    ({ data: { id, name: id, node_type_id: "ai_prompt", config, credentials } });

  it("offers_local_models_and_saved_connections_only", () => {
    const list = listDescribeProviders([
      node("a", { provider: "anthropic", model: "m" }),
      node("b", { provider: "anthropic", model: "m" }, { api_key: "cred1" }),
      node("c", { provider: "auto", model: "llama3", base_url: "http://localhost:11434/v1" }),
      node("d", { provider: "openai", model: "" }, { api_key: "cred2" }),
    ]);
    expect(list.map((p) => [p.nodeId, p.local])).toEqual([["b", false], ["c", true]]);
  });

  it("an_ai_agent_node_with_a_saved_connection_is_offered_and_other_node_types_are_not", () => {
    const agent = { data: { id: "ag", name: "Agent", node_type_id: "ai_agent", config: { provider: "openai", model: "m" }, credentials: { api_key: "c9" } } };
    const other = { data: { id: "ht", name: "Http", node_type_id: "http_request", config: { provider: "openai", model: "m" }, credentials: { api_key: "c9" } } };
    expect(listDescribeProviders([agent, other]).map((p) => p.nodeId)).toEqual(["ag"]);
  });

  it("request_workflow_carries_the_credential_id_and_no_key_text", () => {
    const [p] = listDescribeProviders([node("b", { provider: "anthropic", model: "m", api_key: "sk-LEAK" }, { api_key: "cred1" })]);
    const json = buildDescribeWorkflow(p, { ports: { inputs: [], outputs: [] }, input_schema: {}, output_schema: {} }, "sys", "prompt", "w1");
    expect(json).not.toContain("sk-LEAK");
    expect(JSON.parse(json).nodes[1].credentials).toEqual({ api_key: "cred1" });
  });
});
