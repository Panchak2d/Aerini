import { describe, it, expect } from "vitest";
import { sideEffectOf } from "../side-effects";

describe("sideEffectOf", () => {
  it("flags_steps_that_act_outside_the_app_and_ignores_reads_and_unknown_types", () => {
    expect(sideEffectOf("email_send", {})?.label).toBe("sends an email");
    expect(sideEffectOf("http_request", { method: "POST" })?.label).toBe("sends an HTTP POST request");
    expect(sideEffectOf("http_request", { method: "{{Pick.output.verb}}" })).not.toBeNull();
    expect(sideEffectOf("http_request", { method: "GET" })).toBeNull();
    expect(sideEffectOf("http_request", {})).toBeNull();
    expect(sideEffectOf("file", { operation: "append" })?.label).toBe("writes to a file");
    expect(sideEffectOf("file", { operation: "read" })).toBeNull();
    expect(sideEffectOf("google_sheets", { action: "get_values" })).toBeNull();
    expect(sideEffectOf("ai_prompt", { provider: "openai" })).not.toBeNull();
    expect(sideEffectOf("ai_prompt", { provider: "local" })).toBeNull();
    expect(sideEffectOf("ai_prompt", { provider: "auto", base_url: "http://localhost:11434/v1" })).toBeNull();
    expect(sideEffectOf("transform", {})).toBeNull();
    expect(sideEffectOf("some_plugin_node", {})).toBeNull();
  });
});
