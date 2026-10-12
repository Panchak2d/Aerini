/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";
import { showPopover, closePopover } from "../popover/lifecycle";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeNode(): CanvasNode {
  return new CanvasNode({
    id: "n1", node_type_id: "manual_trigger", node_type: "action", name: "Node",
    config: {}, credentials: {}, position: { x: 0, y: 0 },
    ports: { inputs: [], outputs: [{ id: "output", label: "Output", position: "right" }] },
    input_schema: {}, output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 }, fallback_node: null, dynamic_ports: false,
  });
}

const canvasEl = document.createElement("canvas");
const fakeCanvas = { beginNodeEdit: () => {}, commitNodeEdit: () => {} } as unknown as Canvas;

async function openRetryInput(node: CanvasNode): Promise<HTMLInputElement> {
  const p = showPopover(node, canvasEl, () => {}, fakeCanvas);
  await vi.advanceTimersByTimeAsync(200);
  await p;
  const field = [...document.querySelectorAll(".pop-field, .popover-field, label")]
    .find(el => el.textContent?.includes("Retry attempts"));
  const inp = (field?.closest("div") ?? document).querySelector<HTMLInputElement>('input[type="number"][max="10"]');
  if (!inp) throw new Error("retry input not found");
  return inp;
}

describe("popover retry attempts", () => {
  beforeEach(() => { vi.useFakeTimers(); document.body.innerHTML = ""; });
  afterEach(() => { closePopover(false); vi.useRealTimers(); vi.restoreAllMocks(); });

  it("normal case: an in-range value is stored unchanged", async () => {
    const node = makeNode();
    const inp = await openRetryInput(node);
    inp.value = "4";
    inp.dispatchEvent(new Event("input"));
    expect(node.data.retry.max_attempts).toBe(4);
  });

  it("edge case: a value above the engine's cap is clamped to 10 in both the node and the field", async () => {
    const node = makeNode();
    const inp = await openRetryInput(node);
    inp.value = "999";
    inp.dispatchEvent(new Event("input"));
    expect(node.data.retry.max_attempts).toBe(10);
    expect(inp.value).toBe("10");
  });
});
