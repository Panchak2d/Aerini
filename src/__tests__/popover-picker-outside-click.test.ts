/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";
import { showPopover, closePopover } from "../popover/lifecycle";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));

const node = new CanvasNode({
  id: "n1",
  node_type_id: "manual_trigger",
  node_type: "action",
  name: "Node n1",
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
const canvasEl = document.createElement("canvas");
const fakeCanvas = { beginNodeEdit: () => {}, commitNodeEdit: () => {} } as unknown as Canvas;

describe("popover outside click — data picker", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });
  afterEach(() => {
    closePopover(false);
    vi.useRealTimers();
  });

  async function open(): Promise<void> {
    const p = showPopover(node, canvasEl, () => {}, fakeCanvas);
    await vi.advanceTimersByTimeAsync(200);
    await p;
  }

  it("a press on the data picker's tab keeps the popover open", async () => {
    await open();
    const picker = document.createElement("div");
    picker.className = "expr-picker";
    const tab = document.createElement("button");
    picker.appendChild(tab);
    document.body.appendChild(picker);

    tab.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));

    expect(document.querySelector(".node-popover")).not.toBeNull();
    expect(document.querySelector(".node-popover.popover-out")).toBeNull();
  });

  it("a press elsewhere outside the popover still closes it", async () => {
    await open();
    const other = document.createElement("div");
    document.body.appendChild(other);

    other.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));

    expect(document.querySelector(".node-popover.popover-out")).not.toBeNull();
  });
});
