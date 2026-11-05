/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { CanvasNode } from "../canvas/Node";
import { renderConfigFieldsLoop, type PropSchema } from "../popover/field-renderer";
import type { ExtensionContext } from "../node-configs/popover-utils";
import { dataSwitchEligible, isDataValue } from "../data-switch";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeCtx(config: Record<string, unknown>): ExtensionContext {
  const node = new CanvasNode({
    id: "n1", node_type_id: "http_request", node_type: "action", name: "Fetch", config, credentials: {},
    position: { x: 0, y: 0 },
    ports: { inputs: [{ id: "input", label: "Input", position: "left" }], outputs: [{ id: "output", label: "Output", position: "right" }] },
    input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 }, fallback_node: null, dynamic_ports: false,
  });
  return { node, body: document.createElement("div"), canvasEl: document.createElement("canvas"), onChange: () => {}, creds: [], rerender: () => {} };
}

const NUMBER: Array<[string, PropSchema]> = [["timeout_ms", { type: "number" }]];
const BOOL: Array<[string, PropSchema]> = [["follow", { type: "boolean" }]];

describe("data switch eligibility", () => {
  it("offers number, boolean and dropdown fields but not safety switches or structural keys", () => {
    expect(dataSwitchEligible("http_request", "timeout_ms", { type: "number" })).toBe(true);
    expect(dataSwitchEligible("http_request", "method", { type: "string", enum: ["GET", "POST"] })).toBe(true);
    expect(dataSwitchEligible("http_request", "allow_local", { type: "boolean" })).toBe(false);
    expect(dataSwitchEligible("database", "db_type", { type: "string", enum: ["sqlite"] })).toBe(false);
    expect(dataSwitchEligible("collect_files", "recursive", { type: "boolean" })).toBe(false);
    expect(dataSwitchEligible("http_request", "body", { type: "string" })).toBe(false);
    expect(isDataValue("{{A.output.x}}")).toBe(true);
    expect(isDataValue(5)).toBe(false);
  });
});

describe("data switch in the popover", () => {
  it("number field: switching to data clears the number, typing stores the text, switching back restores it", () => {
    const ctx = makeCtx({ timeout_ms: 30 });
    renderConfigFieldsLoop(ctx, NUMBER, []);
    const btn = ctx.body.querySelector<HTMLButtonElement>(".field-data-switch")!;
    expect(ctx.body.querySelector<HTMLInputElement>('input[type="number"]')!.value).toBe("30");

    btn.click();
    expect(btn.getAttribute("aria-pressed")).toBe("true");
    expect(ctx.node.data.config.timeout_ms).toBeUndefined();
    const box = ctx.body.querySelector<HTMLInputElement>('input[type="text"]')!;
    box.value = "{{Settings.output.timeout}}";
    box.dispatchEvent(new Event("input"));
    expect(ctx.node.data.config.timeout_ms).toBe("{{Settings.output.timeout}}");

    btn.click();
    expect(ctx.node.data.config.timeout_ms).toBe(30);
    expect(ctx.body.querySelector('input[type="number"]')).not.toBeNull();
  });

  it("boolean field already holding a reference opens in data mode and keeps the text", () => {
    const ctx = makeCtx({ follow: "{{Flag.output.on}}" });
    renderConfigFieldsLoop(ctx, BOOL, []);
    expect(ctx.body.querySelector(".field-data-switch")!.getAttribute("aria-pressed")).toBe("true");
    expect(ctx.body.querySelector<HTMLInputElement>('input[type="text"]')!.value).toBe("{{Flag.output.on}}");
    expect(ctx.body.querySelector('input[type="checkbox"]')).toBeNull();
  });
});
