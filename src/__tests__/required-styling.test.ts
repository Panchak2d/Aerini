/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { CanvasNode } from "../canvas/Node";
import { renderConfigFieldsLoop, type PropSchema } from "../popover/field-renderer";
import type { ExtensionContext } from "../node-configs/popover-utils";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeCtx(typeId: string, config: Record<string, unknown>): ExtensionContext {
  const node = new CanvasNode({
    id: "n1", node_type_id: typeId, node_type: "action", name: "Split", config, credentials: {},
    position: { x: 0, y: 0 },
    ports: { inputs: [{ id: "input", label: "Input", position: "left" }], outputs: [{ id: "output", label: "Output", position: "right" }] },
    input_schema: { required: ["text"] }, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 }, fallback_node: null, dynamic_ports: false,
  });
  return { node, body: document.createElement("div"), canvasEl: document.createElement("canvas"), onChange: () => {}, creds: [], rerender: () => {} };
}

const FIELDS: Array<[string, PropSchema]> = [["text", { type: "string" }], ["source_field", { type: "string" }]];

describe("required styling", () => {
  it("a_blank_required_field_is_marked_unless_its_accepted_alternative_is_filled", () => {
    const blank = makeCtx("text_splitter", {});
    renderConfigFieldsLoop(blank, FIELDS, ["text"]);
    expect(blank.body.querySelectorAll(".field-required-empty").length).toBe(1);

    const alt = makeCtx("text_splitter", { source_field: "Feed.output.body" });
    renderConfigFieldsLoop(alt, FIELDS, ["text"]);
    expect(alt.body.querySelectorAll(".field-required-empty").length).toBe(0);
  });
});
