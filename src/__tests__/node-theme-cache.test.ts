/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { CanvasNode, type CanvasNodeData } from "../canvas/Node";

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

function makeMockCtx() {
  const fillRectCalls: Array<{ colorAtCall: unknown }> = [];
  const ctx = {
    save: vi.fn(), restore: vi.fn(),
    beginPath: vi.fn(), closePath: vi.fn(), clip: vi.fn(),
    moveTo: vi.fn(), lineTo: vi.fn(), arcTo: vi.fn(), arc: vi.fn(), rect: vi.fn(),
    fill: vi.fn(), stroke: vi.fn(),
    fillRect: vi.fn(() => { fillRectCalls.push({ colorAtCall: ctx.fillStyle }); }),
    fillText: vi.fn(),
    measureText: vi.fn(() => ({ width: 30 }) as TextMetrics),
    drawImage: vi.fn(),
    createLinearGradient: vi.fn(() => ({ addColorStop: vi.fn() })),
    fillStyle: "", strokeStyle: "", font: "", lineWidth: 1,
    textAlign: "left", textBaseline: "alphabetic", globalAlpha: 1,
  } as unknown as CanvasRenderingContext2D;
  return { ctx, fillRectCalls };
}

describe("Node.ts catAccents cache invalidation on theme switch", () => {
  it("re-reads --cat-action after <html data-theme> changes; does not invalidate on an unrelated property change", async () => {
    document.documentElement.removeAttribute("data-theme");
    document.documentElement.style.setProperty("--cat-action", "rgb(1, 2, 3)");

    // node_type_id "http_request" -> node_type "action", per
    // node-kind-hierarchy.test.ts's own established fixture.
    const node = new CanvasNode(makeNodeData("http_request"));

    const first = makeMockCtx();
    node.draw(first.ctx, 0);
    expect(first.fillRectCalls[0]?.colorAtCall).toBe("rgb(1, 2, 3)");

    // Change the underlying custom property WITHOUT touching data-theme —
    // by design this must NOT invalidate the cache (it's keyed off the
    // attribute, not polled every frame).
    document.documentElement.style.setProperty("--cat-action", "rgb(9, 9, 9)");
    const second = makeMockCtx();
    node.draw(second.ctx, 0);
    expect(second.fillRectCalls[0]?.colorAtCall).toBe("rgb(1, 2, 3)");

    // Flip the theme attribute — the actual invalidation trigger.
    // MutationObserver callbacks run at the next microtask checkpoint.
    document.documentElement.setAttribute("data-theme", "paper");
    await new Promise(resolve => setTimeout(resolve, 0));

    const third = makeMockCtx();
    node.draw(third.ctx, 0);
    expect(third.fillRectCalls[0]?.colorAtCall).toBe("rgb(9, 9, 9)");
  });
});
