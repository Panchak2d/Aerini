/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";
import { showPopover, closePopover } from "../popover/lifecycle";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level);
// lifecycle.ts's listCredentials() goes through the same module.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeNode(id: string): CanvasNode {
  return new CanvasNode({
    id,
    node_type_id: "manual_trigger",
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

const canvasEl = document.createElement("canvas");
const fakeCanvas = { beginNodeEdit: () => {}, commitNodeEdit: () => {} } as unknown as Canvas;

async function openPopover(node: CanvasNode): Promise<void> {
  const p = showPopover(node, canvasEl, () => {}, fakeCanvas);
  await vi.advanceTimersByTimeAsync(200); // flush listCredentials() await + the 50ms/120ms setTimeouts
  await p;
}

describe("popover lifecycle — document listener cleanup", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });

  afterEach(() => {
    closePopover(false);
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("normal case: closing via Esc removes the focus-trap, outside-click, and Esc listeners", async () => {
    const addSpy = vi.spyOn(document, "addEventListener");
    const removeSpy = vi.spyOn(document, "removeEventListener");

    await openPopover(makeNode("n1"));
    expect(addSpy.mock.calls.filter(c => c[0] === "keydown").length).toBe(2);   // focus trap + Esc
    expect(addSpy.mock.calls.filter(c => c[0] === "mousedown").length).toBe(1); // outside click

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));

    expect(removeSpy.mock.calls.filter(c => c[0] === "keydown").length).toBe(2);
    expect(removeSpy.mock.calls.filter(c => c[0] === "mousedown").length).toBe(1);
  });

  it("edge case: closing via a direct closePopover() call (the × button's path, and the path a superseding popover takes) still removes all three listeners", async () => {
    const removeSpy = vi.spyOn(document, "removeEventListener");

    await openPopover(makeNode("n1"));
    closePopover(false); // what closeBtn's click handler does — not Esc, not outside-click

    expect(removeSpy.mock.calls.filter(c => c[0] === "keydown").length).toBe(2);
    expect(removeSpy.mock.calls.filter(c => c[0] === "mousedown").length).toBe(1);
  });

  it("edge case: opening a second popover while the first is open tears down the first's listeners before the second's are added (the exact leak path)", async () => {
    const removeSpy = vi.spyOn(document, "removeEventListener");

    await openPopover(makeNode("a"));
    removeSpy.mockClear();
    await openPopover(makeNode("b")); // showPopover() calls closePopover(false) on "a" internally, first

    expect(removeSpy.mock.calls.filter(c => c[0] === "keydown").length).toBe(2);
    expect(removeSpy.mock.calls.filter(c => c[0] === "mousedown").length).toBe(1);
  });
});

describe("popover lifecycle — auto-focus target (N-11)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });

  afterEach(() => {
    closePopover(false);
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("normal case: focus lands on the first body field, not the header's Test button", async () => {
    await openPopover(makeNode("n1"));

    const testBtn = document.querySelector(".popover-test-btn");
    expect(testBtn).not.toBeNull();
    expect(document.activeElement).not.toBe(testBtn);
    expect(document.activeElement?.closest(".popover-body")).not.toBeNull();
  });
});
