/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";
import { showPopover, closePopover } from "../popover/lifecycle";
import { registerNodeDescriptors } from "../canvas/node-registry";
import type { NodeDescriptor } from "../ipc/workflow";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level);
// lifecycle.ts's listCredentials() goes through the same module.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeNode(id: string, node_type_id: string): CanvasNode {
  return new CanvasNode({
    id,
    node_type_id,
    node_type: "action",
    name: `Node ${id}`,
    config: {},
    credentials: {},
    position: { x: 0, y: 0 },
    ports: { inputs: [], outputs: [{ id: "output", label: "Output", position: "right" }] },
    input_schema: {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  });
}

function makeDescriptor(type_id: string, extra: Partial<NodeDescriptor> = {}): NodeDescriptor {
  return {
    type_id,
    display_name: "Test Node",
    node_type: "action",
    version: "1.0",
    input_schema: {},
    output_schema: {},
    ports: { inputs: [], outputs: [] },
    ...extra,
  };
}

const canvasEl = document.createElement("canvas");
const fakeCanvas = {} as unknown as Canvas;

async function openPopover(node: CanvasNode): Promise<void> {
  const p = showPopover(node, canvasEl, () => {}, fakeCanvas);
  await vi.advanceTimersByTimeAsync(200); // flush listCredentials() await + the 50ms/120ms setTimeouts
  await p;
}

describe("popover header — plugin meta", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });

  afterEach(() => {
    closePopover(false);
    registerNodeDescriptors([]);
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("built-in node: no plugin tag or author line in the header", async () => {
    registerNodeDescriptors([makeDescriptor("manual_trigger")]);
    await openPopover(makeNode("n1", "manual_trigger"));

    expect(document.querySelector(".popover-plugin-meta")).toBeNull();
    expect(document.querySelector(".popover-header-text > .popover-subtitle")).not.toBeNull();
  });

  it("plugin node with no declared author: shows the Plugin tag, no author line", async () => {
    registerNodeDescriptors([makeDescriptor("custom_plugin_node", { is_plugin: true })]);
    await openPopover(makeNode("n1", "custom_plugin_node"));

    const meta = document.querySelector(".popover-plugin-meta")!;
    expect(meta).not.toBeNull();
    expect(meta.querySelector(".palette-plugin-tag")?.textContent).toBe("Plugin");
    expect(meta.querySelector(".popover-plugin-author")).toBeNull();
    expect(meta.previousElementSibling?.classList.contains("popover-subtitle")).toBe(true);
  });

  it("plugin node with a declared author: shows the Plugin tag and 'by {author}'", async () => {
    registerNodeDescriptors([makeDescriptor("custom_plugin_node", { is_plugin: true, author: "Acme Corp" })]);
    await openPopover(makeNode("n1", "custom_plugin_node"));

    const meta = document.querySelector(".popover-plugin-meta")!;
    expect(meta.querySelector(".palette-plugin-tag")?.textContent).toBe("Plugin");
    expect(meta.querySelector(".popover-plugin-author")?.textContent).toBe("by Acme Corp");
  });

  it("node type absent from the descriptor registry (e.g. plugin since uninstalled): no plugin meta, no crash", async () => {
    registerNodeDescriptors([]);
    await openPopover(makeNode("n1", "custom_plugin_node"));

    expect(document.querySelector(".popover-plugin-meta")).toBeNull();
  });

  it("recovers field schema from the shared registry when the saved node has an empty input_schema", async () => {
    registerNodeDescriptors([makeDescriptor("custom_plugin_node", {
      is_plugin: true,
      input_schema: { type: "object", properties: { greeting: { type: "string" } } },
    })]);
    await openPopover(makeNode("n1", "custom_plugin_node"));

    const labels = [...document.querySelectorAll(".popover-body .field-label")].map(l => l.textContent);
    expect(labels).toContain("Greeting");
  });
});
