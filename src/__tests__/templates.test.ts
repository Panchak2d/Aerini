import { describe, expect, it } from "vitest";
import { readdirSync } from "node:fs";
import {
  bundledTemplates, describeTemplate, getTemplates, templateDifficulty, templateNeeds,
} from "../templates";
import { firstSentence } from "../node-descriptions";

const wf = (nodes: Array<Record<string, unknown>>, extra: Record<string, unknown> = {}) =>
  ({ name: "T", description: "", nodes, edges: [], ...extra });
const node = (node_type_id: string, config: Record<string, unknown> = {}) => ({ node_type_id, config });

describe("template gallery model", () => {
  it("offers_every_shipped_example_file_as_a_template", () => {
    const files = readdirSync("examples").filter(f => f.endsWith(".aerini"));
    expect(bundledTemplates().map(t => t.id).sort()).toEqual(files.map(f => `tpl_${f}`).sort());
  });

  it("lists_beginner_templates_first_and_keeps_the_built_in_fetch_example", () => {
    const all = getTemplates();
    expect(all[0].difficulty).toBe("Beginner");
    expect(all.some(t => t.id === "tpl_getting_started" && t.needs.length === 0)).toBe(true);
    const order = all.map(t => ["Beginner", "Intermediate", "Advanced"].indexOf(t.difficulty));
    expect(order).toEqual([...order].sort((a, b) => a - b));
  });

  it("rates_small_plain_flows_beginner_and_agent_or_loop_flows_advanced", () => {
    expect(templateDifficulty(wf([node("manual_trigger"), node("http_request"), node("output")]))).toBe("Beginner");
    expect(templateDifficulty(wf([node("manual_trigger"), node("http_request"), node("output"), node("set_variable"), node("if_condition")]))).toBe("Intermediate");
    expect(templateDifficulty(wf([node("manual_trigger"), node("ai_agent")]))).toBe("Advanced");
  });

  it("derives_needs_from_ai_provider_and_account_nodes", () => {
    expect(templateNeeds(wf([node("ai_prompt", { provider: "local" })]))).toEqual(["Ollama"]);
    expect(templateNeeds(wf([node("ai_prompt", { provider: "openai" }), node("email_send")]))).toEqual(["API key", "Account credentials"]);
    expect(templateNeeds(wf([node("http_request")]))).toEqual([]);
  });

  it("skips_files_that_are_not_workflows_and_cuts_the_summary_at_the_first_sentence", () => {
    expect(describeTemplate("x", { name: "T" })).toBeNull();
    expect(describeTemplate("x", wf([], { description: "Fetches a page. Then shows it." }))?.summary).toBe("Fetches a page.");
    expect(firstSentence("No full stop here")).toBe("No full stop here");
  });
});
