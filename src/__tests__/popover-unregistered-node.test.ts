/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";
import { showPopover, closePopover } from "../popover/lifecycle";
import { registerNodeDescriptors } from "../canvas/node-registry";
import type { NodeDescriptor } from "../ipc/workflow";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeNode(typeId: string, config: Record<string, unknown> = {}, inputSchema: Record<string, unknown> = {}): CanvasNode {
  return new CanvasNode({
    id: "n1",
    node_type_id: typeId,
    node_type: "action",
    name: "Node n1",
    config,
    credentials: {},
    position: { x: 0, y: 0 },
    ports: { inputs: [], outputs: [{ id: "output", label: "Output", position: "right" }] },
    input_schema: inputSchema,
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  });
}

function descriptor(typeId: string, isPlugin: boolean): NodeDescriptor {
  return {
    type_id: typeId,
    display_name: typeId,
    node_type: "action",
    version: "1.0",
    input_schema: { type: "object", properties: { greeting: { type: "string" } } },
    output_schema: {},
    ports: { inputs: [], outputs: [] },
    is_plugin: isPlugin,
  };
}

const canvasEl = document.createElement("canvas");
const fakeCanvas = {} as unknown as Canvas;

async function openPopover(node: CanvasNode): Promise<void> {
  const p = showPopover(node, canvasEl, () => {}, fakeCanvas);
  await vi.advanceTimersByTimeAsync(200);
  await p;
}

const notice = (): HTMLElement | null => document.querySelector<HTMLElement>(".popover-body [role='note']");

describe("popover -- unregistered node type notice", () => {
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

  it("loaded registry without this type: text notice shows the type id as text, saved-schema fields stay editable", async () => {
    registerNodeDescriptors([descriptor("some_builtin", false)]);
    const typeId = "gone<b>_plugin";
    await openPopover(makeNode(typeId, { greeting: "hi" }, { type: "object", properties: { greeting: { type: "string" } } }));

    const el = notice()!;
    expect(el).not.toBeNull();
    expect(el.textContent).toContain("Node type not available");
    expect(el.querySelector("code")!.textContent).toBe(typeId);
    expect(el.querySelector("b")).toBeNull();
    expect([...document.querySelectorAll(".popover-body .field-label")].map(l => l.textContent)).toContain("Greeting");
  });

  it("registered built-in and registered plugin: no notice", async () => {
    registerNodeDescriptors([descriptor("a_builtin", false), descriptor("a_plugin", true)]);
    await openPopover(makeNode("a_builtin"));
    expect(notice()).toBeNull();
    closePopover(false);
    await openPopover(makeNode("a_plugin"));
    expect(notice()).toBeNull();
  });

  it("empty registry (not loaded yet, or no backend): no notice", async () => {
    registerNodeDescriptors([]);
    await openPopover(makeNode("http_request"));
    expect(notice()).toBeNull();
  });
});
