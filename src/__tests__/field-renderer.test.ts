/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { CanvasNode } from "../canvas/Node";
import { renderConfigFieldsLoop, type PropSchema } from "../popover/field-renderer";
import type { ExtensionContext } from "../node-configs/popover-utils";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level)
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeNode(id: string): CanvasNode {
  return new CanvasNode({
    id,
    node_type_id: "save_to_folder",
    node_type: "action",
    name: `Node ${id}`,
    config: {},
    credentials: {},
    position: { x: 0, y: 0 },
    ports: {
      inputs:  [{ id: "input",  label: "Input",  position: "left"  }],
      outputs: [{ id: "output", label: "Output", position: "right" }],
    },
    input_schema: {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  });
}

function makeCtx(node: CanvasNode): ExtensionContext {
  return {
    node,
    body:     document.createElement("div"),
    canvasEl: document.createElement("canvas"),
    onChange: () => {},
    creds:    [],
    rerender: () => {},
  };
}

describe("field-renderer — maxLength", () => {
  it("normal case: a maxLength-bearing prop sets the input's native maxLength and appends it to the hint", () => {
    const ctx = makeCtx(makeNode("n1"));
    const props: Array<[string, PropSchema]> = [
      ["filename_prefix", {
        type: "string",
        description: "Optional prefix prepended to every saved filename",
        maxLength: 50,
      }],
    ];

    renderConfigFieldsLoop(ctx, props, []);

    const input = ctx.body.querySelector("input") as HTMLInputElement;
    expect(input.maxLength).toBe(50);

    const hint = ctx.body.querySelector(".field-hint") as HTMLElement;
    expect(hint.textContent).toBe(
      "Optional prefix prepended to every saved filename · Max 50 characters"
    );
  });

  it("edge case: a prop with no maxLength leaves the native maxLength unset and the hint as the bare description", () => {
    const ctx = makeCtx(makeNode("n1"));
    const props: Array<[string, PropSchema]> = [
      ["some_field", { type: "string", description: "Just a description" }],
    ];

    renderConfigFieldsLoop(ctx, props, []);

    const input = ctx.body.querySelector("input") as HTMLInputElement;
    expect(input.hasAttribute("maxlength")).toBe(false);

    const hint = ctx.body.querySelector(".field-hint") as HTMLElement;
    expect(hint.textContent).toBe("Just a description");
  });
});
