/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";
import { showPopover, closePopover } from "../popover/lifecycle";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeNode(x: number, y: number): CanvasNode {
  return new CanvasNode({
    id: "n1", node_type_id: "any_node", node_type: "action", name: "Node n1",
    config: {}, credentials: {}, position: { x, y },
    ports: { inputs: [], outputs: [{ id: "output", label: "Output", position: "right" }] },
    input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: false,
  });
}

async function open(node: CanvasNode): Promise<HTMLElement> {
  const canvasEl = document.createElement("canvas") as HTMLCanvasElement & { __canvas?: unknown };
  canvasEl.getBoundingClientRect = () => ({ left: 0, top: 0, right: 1000, bottom: 800, width: 1000, height: 800 } as DOMRect);
  canvasEl.__canvas = { zoom: 1, panX: 0, panY: 0, beginNodeEdit: () => {}, commitNodeEdit: () => {} } as unknown as Canvas;
  const p = showPopover(node, canvasEl, () => {}, canvasEl.__canvas as Canvas);
  await vi.advanceTimersByTimeAsync(200);
  await p;
  return document.querySelector(".node-popover") as HTMLElement;
}

describe("popover positioning uses the stylesheet's size", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = `<style>.node-popover { width: 400px; max-height: 700px; }</style>`;
    window.innerHeight = 800;
    Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
      configurable: true,
      get(this: HTMLElement) { return this.classList.contains("node-popover") ? 400 : 0; },
    });
  });

  afterEach(() => {
    closePopover(false);
    delete (HTMLElement.prototype as unknown as Record<string, unknown>).offsetWidth;
    vi.useRealTimers();
  });

  it("flips left when 400px (not 360px) of width would cross the right edge", async () => {
    // node right edge at 590 → opening right needs 602..1002 > 1000 - 12
    const pop = await open(makeNode(370, 100));
    expect(pop.style.left).toBe("166px"); // 590 - 400 - 24
  });

  it("clamps top against the 700px CSS max-height, not 680px", async () => {
    const node = makeNode(0, 0);
    node.data.position.y = 240 - node.height / 2; // top = 100 → 100 + 700 > 788
    const pop = await open(node);
    expect(pop.style.top).toBe("88px"); // 800 - 700 - 12
  });
});
