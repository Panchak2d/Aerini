/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { CanvasNode } from "../canvas/Node";
import { renderPluginRawConfigEditor } from "../popover/extensions/plugin-config";
import type { ExtensionContext } from "../node-configs/popover-utils";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level)
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeNode(config: Record<string, unknown>): CanvasNode {
  return new CanvasNode({
    id: "n1",
    node_type_id: "my_plugin_trigger",
    node_type: "action",
    name: "Node n1",
    config,
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

function makeCtx(node: CanvasNode, hasConfigSection: boolean, onChange: () => void, rerender: () => void): ExtensionContext {
  return {
    node,
    body:     document.createElement("div"),
    canvasEl: document.createElement("canvas"),
    onChange, creds: [], rerender,
    hasConfigSection,
  };
}

describe("renderPluginRawConfigEditor", () => {
  it("normal case: valid JSON on blur replaces node.data.config and commits via onChange+rerender", () => {
    const node = makeNode({ old_key: "old_value" });
    const onChange = vi.fn();
    const rerender = vi.fn();
    const ctx = makeCtx(node, true, onChange, rerender);

    renderPluginRawConfigEditor(ctx);
    const ta = ctx.body.querySelector("textarea") as HTMLTextAreaElement;
    expect(JSON.parse(ta.value)).toEqual({ old_key: "old_value" });

    ta.value = JSON.stringify({ new_key: "new_value" });
    ta.dispatchEvent(new Event("blur"));

    expect(node.data.config).toEqual({ new_key: "new_value" });
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(rerender).toHaveBeenCalledTimes(1);
  });

  it("edge case: invalid JSON on blur is rejected — config unchanged, no commit, error shown", () => {
    const node = makeNode({ old_key: "old_value" });
    const onChange = vi.fn();
    const rerender = vi.fn();
    const ctx = makeCtx(node, true, onChange, rerender);

    renderPluginRawConfigEditor(ctx);
    const ta = ctx.body.querySelector("textarea") as HTMLTextAreaElement;

    ta.value = "{ not valid json";
    ta.dispatchEvent(new Event("blur"));

    expect(node.data.config).toEqual({ old_key: "old_value" });
    expect(onChange).not.toHaveBeenCalled();
    expect(rerender).not.toHaveBeenCalled();
    const err = ctx.body.querySelector('[role="alert"]') as HTMLElement;
    expect(err.style.display).toBe("block");
  });

  it("edge case: blurring without editing commits nothing", () => {
    const node = makeNode({ old_key: "old_value" });
    const onChange = vi.fn();
    const rerender = vi.fn();
    const ctx = makeCtx(node, true, onChange, rerender);

    renderPluginRawConfigEditor(ctx);
    (ctx.body.querySelector("textarea") as HTMLTextAreaElement).dispatchEvent(new Event("blur"));

    expect(onChange).not.toHaveBeenCalled();
    expect(rerender).not.toHaveBeenCalled();
  });

  it("hidden keys are not shown, survive a commit, and cannot be set from the JSON", () => {
    const node = makeNode({ old_key: "old_value", api_key: "sk-inline" });
    const ctx = makeCtx(node, true, vi.fn(), vi.fn());

    renderPluginRawConfigEditor(ctx, new Set(["api_key"]));
    const ta = ctx.body.querySelector("textarea") as HTMLTextAreaElement;
    expect(JSON.parse(ta.value)).toEqual({ old_key: "old_value" });
    expect(ta.value).not.toContain("sk-inline");

    ta.value = JSON.stringify({ new_key: 1, api_key: "attacker-value" });
    ta.dispatchEvent(new Event("blur"));

    expect(node.data.config).toEqual({ api_key: "sk-inline", new_key: 1 });
  });
});
