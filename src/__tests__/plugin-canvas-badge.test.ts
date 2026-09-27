import { describe, it, expect, vi, afterEach } from "vitest";
import { NODE_IDS } from "../node-ids";
import { CanvasNode, type CanvasNodeData } from "../canvas/Node";
import { registerNodeDescriptors } from "../canvas/CanvasSerializer";
import type { NodeDescriptor } from "../ipc/workflow";

function makeMockCtx(): CanvasRenderingContext2D {
  const gradient = { addColorStop: vi.fn() };
  return {
    save: vi.fn(), restore: vi.fn(),
    beginPath: vi.fn(), closePath: vi.fn(), clip: vi.fn(),
    moveTo: vi.fn(), lineTo: vi.fn(), arcTo: vi.fn(), arc: vi.fn(), rect: vi.fn(),
    fill: vi.fn(), stroke: vi.fn(), fillRect: vi.fn(),
    fillText: vi.fn(), measureText: vi.fn(() => ({ width: 30 }) as TextMetrics),
    drawImage: vi.fn(),
    createLinearGradient: vi.fn(() => gradient),
    fillStyle: "", strokeStyle: "", font: "", lineWidth: 1,
    textAlign: "left", textBaseline: "alphabetic", globalAlpha: 1,
  } as unknown as CanvasRenderingContext2D;
}

function makeNodeData(node_type_id: string): CanvasNodeData {
  return {
    id: "n1",
    node_type_id,
    node_type: "action",
    name: "Test Node",
    config: {},
    credentials: {},
    position: { x: 0, y: 0 },
    ports: { inputs: [], outputs: [] },
    input_schema: {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  };
}

function makeDescriptor(type_id: string, is_plugin: boolean): NodeDescriptor {
  return {
    type_id,
    display_name: "Test Node",
    node_type: "action",
    version: "1.0",
    input_schema: {},
    output_schema: {},
    ports: { inputs: [], outputs: [] },
    is_plugin,
  };
}

describe("CanvasNode.draw — plugin badge", () => {
  afterEach(() => {
    // Registry is module-level state shared across the whole draw() surface
    // -- reset so one test's registration can't leak into the next.
    registerNodeDescriptors([]);
  });

  it("renders the plugin glyph for a node type registered as is_plugin", () => {
    registerNodeDescriptors([makeDescriptor("custom_plugin_node", true)]);
    const node = new CanvasNode(makeNodeData("custom_plugin_node"));
    const ctx = makeMockCtx();
    node.draw(ctx, 0);
    expect(ctx.fillText).toHaveBeenCalledWith("P", expect.any(Number), expect.any(Number));
  });

  it("does not render the plugin glyph for a built-in node type", () => {
    registerNodeDescriptors([makeDescriptor(NODE_IDS.HTTP_REQUEST, false)]);
    const node = new CanvasNode(makeNodeData(NODE_IDS.HTTP_REQUEST));
    const ctx = makeMockCtx();
    node.draw(ctx, 0);
    expect(ctx.fillText).not.toHaveBeenCalledWith("P", expect.any(Number), expect.any(Number));
  });

  it("does not render the plugin glyph for a node type absent from the registry", () => {
    // e.g. a placed node whose plugin was since uninstalled -- registry no
    // longer has an entry for its type_id, same as every other plugin-driven
    // lookup in this codebase (icon, ports) for that scenario.
    const node = new CanvasNode(makeNodeData("custom_plugin_node"));
    const ctx = makeMockCtx();
    node.draw(ctx, 0);
    expect(ctx.fillText).not.toHaveBeenCalledWith("P", expect.any(Number), expect.any(Number));
  });

  it("still renders the plugin glyph while the node is in a running status", () => {
    registerNodeDescriptors([makeDescriptor("custom_plugin_node", true)]);
    const node = new CanvasNode(makeNodeData("custom_plugin_node"));
    node.status = "running";
    const ctx = makeMockCtx();
    node.draw(ctx, 0.1);
    expect(ctx.fillText).toHaveBeenCalledWith("P", expect.any(Number), expect.any(Number));
  });
});

describe("CanvasNode.draw — unregistered-type badge", () => {
  afterEach(() => {
    registerNodeDescriptors([]);
  });

  it("renders the '?' glyph (and not 'P') for a type absent from a loaded registry", () => {
    registerNodeDescriptors([makeDescriptor(NODE_IDS.HTTP_REQUEST, false)]);
    const ctx = makeMockCtx();
    new CanvasNode(makeNodeData("custom_plugin_node")).draw(ctx, 0);
    expect(ctx.fillText).toHaveBeenCalledWith("?", expect.any(Number), expect.any(Number));
    expect(ctx.fillText).not.toHaveBeenCalledWith("P", expect.any(Number), expect.any(Number));
  });

  it("renders no '?' for a registered built-in, or when the registry has not loaded", () => {
    registerNodeDescriptors([makeDescriptor(NODE_IDS.HTTP_REQUEST, false)]);
    const registered = makeMockCtx();
    new CanvasNode(makeNodeData(NODE_IDS.HTTP_REQUEST)).draw(registered, 0);
    expect(registered.fillText).not.toHaveBeenCalledWith("?", expect.any(Number), expect.any(Number));

    registerNodeDescriptors([]);
    const empty = makeMockCtx();
    new CanvasNode(makeNodeData(NODE_IDS.HTTP_REQUEST)).draw(empty, 0);
    expect(empty.fillText).not.toHaveBeenCalledWith("?", expect.any(Number), expect.any(Number));
  });
});
