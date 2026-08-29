// @vitest-environment jsdom

import { describe, it, expect, vi } from "vitest";
import { SnapEngine } from "../canvas/SnapEngine";
import { CanvasNode, NODE_WIDTH } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level)
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

// height = NODE_HEADER(40) + bodyRows*PORT_GAP(28) + PADDING(14)
// 1 port  -> bodyRows 1 -> height 82
// 3 ports -> bodyRows 3 -> height 138
function makeNode(id: string, x: number, y: number, outputCount = 1): CanvasNode {
  return new CanvasNode({
    id,
    node_type_id: "http_request",
    node_type: "action",
    name: `Node ${id}`,
    config: {},
    credentials: {},
    position: { x, y },
    ports: {
      inputs: [{ id: "input", label: "Input", position: "left" }],
      outputs: Array.from({ length: outputCount }, (_, i) => ({
        id: `output_${i}`, label: `Output ${i}`, position: "right" as const,
      })),
    },
    input_schema: {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  });
}

function stubCanvas(nodes: CanvasNode[]): Canvas {
  const map = new Map(nodes.map(n => [n.data.id, n]));
  return { nodes: map } as unknown as Canvas;
}

describe("SnapEngine.computeSnap — y-axis alignment", () => {
  it("produces a horizontal guide when two differently-tall nodes share a top edge (normal case)", () => {
    const moving = makeNode("a", 0, 100, 1);   // height 82
    const other  = makeNode("b", 400, 100, 3); // height 138 — heights differ, tops match
    const snap = new SnapEngine(stubCanvas([moving, other]));

    snap.computeSnap(moving);

    const horizontalAtTop = snap.snapGuides.filter(g => g.y1 === g.y2 && g.y1 === 100);
    expect(horizontalAtTop.length).toBe(1);
    // Guide spans the x-range of both nodes, padded by 20 on each side.
    expect(horizontalAtTop[0].x1).toBe(Math.min(0, 400) - 20);
    expect(horizontalAtTop[0].x2).toBe(Math.max(0 + NODE_WIDTH, 400 + NODE_WIDTH) + 20);
  });

  it("produces a horizontal guide when two differently-tall nodes' bottom edges align, tops unaligned (edge case)", () => {
    // moving: y=140, height 82 (1 output)  -> bottom 222
    // other:  y=84,  height 138 (3 outputs) -> bottom 222
    // Bottom edges align, tops (140 vs 84) do not.
    const moving = makeNode("a", 0, 140, 1);
    const other  = makeNode("b", 400, 84, 3);
    const snap = new SnapEngine(stubCanvas([moving, other]));

    snap.computeSnap(moving);

    expect(snap.snapGuides.length).toBe(1);
    expect(snap.snapGuides[0].y1).toBe(140);
    expect(snap.snapGuides[0].y1).toBe(snap.snapGuides[0].y2);
  });

  it("still produces a vertical guide for x-axis (left-edge) alignment — pre-existing behavior unaffected", () => {
    const moving = makeNode("a", 50, 0, 1);
    const other  = makeNode("b", 50, 500, 1);
    const snap = new SnapEngine(stubCanvas([moving, other]));

    snap.computeSnap(moving);

    const verticalAtLeft = snap.snapGuides.filter(g => g.x1 === g.x2 && g.x1 === 50);
    expect(verticalAtLeft.length).toBeGreaterThanOrEqual(1);
  });

  it("produces no guide when neither axis is within the snap threshold", () => {
    const moving = makeNode("a", 0, 0, 1);
    const other  = makeNode("b", 500, 500, 1);
    const snap = new SnapEngine(stubCanvas([moving, other]));

    snap.computeSnap(moving);

    expect(snap.snapGuides.length).toBe(0);
  });
});
