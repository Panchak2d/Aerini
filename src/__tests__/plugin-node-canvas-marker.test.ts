import { describe, it, expect, vi, afterEach } from "vitest";
import { NODE_IDS } from "../node-ids";
import { CanvasNode, type CanvasNodeData } from "../canvas/Node";
import { registerNodeDescriptors, isDangerousNodeType } from "../canvas/node-registry";
import type { NodeDescriptor } from "../ipc/workflow";

const PLUGIN_ID = "my_plugin_node";

const PLUGIN_DESCRIPTOR: NodeDescriptor = {
  type_id: PLUGIN_ID, display_name: "My Plugin", node_type: "action", version: "1",
  input_schema: {}, output_schema: {},
  ports: { inputs: [], outputs: [] },
  is_plugin: true,
};

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
    id: "n1", node_type_id, node_type: "action", name: "Test Node",
    config: {}, credentials: {}, position: { x: 0, y: 0 },
    ports: { inputs: [], outputs: [] },
    input_schema: {}, output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: false,
  };
}

function drawn(typeId: string): CanvasRenderingContext2D {
  const ctx = makeMockCtx();
  new CanvasNode(makeNodeData(typeId)).draw(ctx, 0);
  return ctx;
}

const drewGlyph = (ctx: CanvasRenderingContext2D, g: string) =>
  (ctx.fillText as ReturnType<typeof vi.fn>).mock.calls.some(c => c[0] === g);

afterEach(() => registerNodeDescriptors([]));

describe("CanvasNode.draw — plugin marker", () => {
  it("draws the P chip but no danger badge for a registered plugin node", () => {
    registerNodeDescriptors([PLUGIN_DESCRIPTOR]);
    const ctx = drawn(PLUGIN_ID);
    expect(drewGlyph(ctx, "P")).toBe(true);
    expect(drewGlyph(ctx, "!")).toBe(false);
  });

  it("built-in dangerous nodes keep the danger badge but get no P chip", () => {
    const ctx = drawn(NODE_IDS.SHELL_EXEC);
    expect(drewGlyph(ctx, "!")).toBe(true);
    expect(drewGlyph(ctx, "P")).toBe(false);
  });

  it("does not add the marker to the serialized node", () => {
    registerNodeDescriptors([PLUGIN_DESCRIPTOR]);
    const wire = new CanvasNode(makeNodeData(PLUGIN_ID)).toWorkflowNode() as Record<string, unknown>;
    expect(Object.keys(wire).sort()).toEqual([
      "config", "credentials", "disabled", "dynamic_ports", "fallback_node", "id",
      "input_schema", "name", "node_type", "node_type_id", "output_schema",
      "ports", "position", "retry",
    ]);
  });
});

describe("isDangerousNodeType", () => {
  it("is true for built-in dangerous ids and registered plugins, false otherwise", () => {
    registerNodeDescriptors([PLUGIN_DESCRIPTOR]);
    expect(isDangerousNodeType(NODE_IDS.SHELL_EXEC)).toBe(true);
    expect(isDangerousNodeType(PLUGIN_ID)).toBe(true);
    expect(isDangerousNodeType(NODE_IDS.HTTP_REQUEST)).toBe(false);
  });
});
