/**
 * Batch M (T2-1 frontend, S8-1/S9-4): src/node-ids.ts's DANGEROUS_NODE_IDS had
 * drifted from the Rust canonical list (aerini_engine::nodes::DANGEROUS_NODE_TYPE_IDS,
 * Batch L) — it listed `file` instead of `database`. These tests pin the exact
 * set so a future edit to either side that breaks the sync fails loudly here
 * instead of silently drifting again.
 *
 * Node.ts's draw() is otherwise untestable in this environment — jsdom has no
 * 2D canvas context (confirmed limitation, Batch D's canvas-safety.test.ts) —
 * but the danger badge's own call chain (save/beginPath/moveTo/lineTo/closePath/
 * fill/font/fillText/restore, plus measureText for positioning) has no
 * dependency on real canvas rendering, so a duck-typed CanvasRenderingContext2D
 * mock exercises the real draw() method end to end. getIconBitmap's bitmap
 * cache is empty in this environment (nothing calls preloadAllIcons), so
 * draw() always takes its font-fallback icon branch, not ctx.drawImage — the
 * mock does not need to satisfy that path.
 */
import { describe, it, expect, vi } from "vitest";
import { NODE_IDS, DANGEROUS_NODE_IDS } from "../node-ids";
import { CanvasNode, type CanvasNodeData } from "../canvas/Node";

describe("DANGEROUS_NODE_IDS", () => {
  it("matches the Rust canonical list exactly: shell_exec, code, database", () => {
    expect(DANGEROUS_NODE_IDS).toEqual(new Set(["shell_exec", "code", "database"]));
  });

  it("no longer contains file (regression guard for the Batch L residual)", () => {
    expect(DANGEROUS_NODE_IDS.has(NODE_IDS.FILE)).toBe(false);
  });
});

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

describe("CanvasNode.draw — danger badge", () => {
  it("renders the danger glyph for a node type in DANGEROUS_NODE_IDS", () => {
    const node = new CanvasNode(makeNodeData(NODE_IDS.SHELL_EXEC));
    const ctx = makeMockCtx();
    node.draw(ctx, 0);
    expect(ctx.fillText).toHaveBeenCalledWith("!", expect.any(Number), expect.any(Number));
  });

  it("does not render the danger glyph for a non-dangerous node type", () => {
    const node = new CanvasNode(makeNodeData(NODE_IDS.HTTP_REQUEST));
    const ctx = makeMockCtx();
    node.draw(ctx, 0);
    expect(ctx.fillText).not.toHaveBeenCalledWith("!", expect.any(Number), expect.any(Number));
  });

  it("still renders the danger glyph while the node is in a running status", () => {
    const node = new CanvasNode(makeNodeData(NODE_IDS.DATABASE));
    node.status = "running";
    const ctx = makeMockCtx();
    node.draw(ctx, 0.1);
    expect(ctx.fillText).toHaveBeenCalledWith("!", expect.any(Number), expect.any(Number));
  });
});
