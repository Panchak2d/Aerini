/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";
import { showPopover, closePopover } from "../popover/lifecycle";
import { registerNodeDescriptors } from "../canvas/node-registry";
import { showConfirm } from "../confirm";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));
vi.mock("../confirm", () => ({ showConfirm: vi.fn() }));

function makeNode(id: string, typeId: string): CanvasNode {
  return new CanvasNode({
    id,
    node_type_id: typeId,
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
const fakeCanvas = {} as unknown as Canvas;

async function openPopover(node: CanvasNode): Promise<void> {
  const p = showPopover(node, canvasEl, () => {}, fakeCanvas);
  await vi.advanceTimersByTimeAsync(200);
  await p;
}

const runCalls = (): number =>
  vi.mocked(invoke).mock.calls.filter(c => c[0] === "run_workflow").length;

async function clickTest(): Promise<void> {
  const before = vi.mocked(showConfirm).mock.calls.length + runCalls();
  document.querySelector<HTMLButtonElement>(".popover-test-btn")!.click();
  // Approval keys are hashed with WebCrypto, which settles on a real event-loop
  // tick that fake-timer advances don't wait for, so wait for the click's effect.
  await vi.waitFor(() => expect(vi.mocked(showConfirm).mock.calls.length + runCalls()).toBeGreaterThan(before));
  await vi.advanceTimersByTimeAsync(0);
  await vi.advanceTimersByTimeAsync(0);
}

describe("popover Test button -- dangerous-node confirmation", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = `<div id="confirm-modal" class="hidden"></div>`;
    vi.mocked(invoke).mockClear();
    vi.mocked(showConfirm).mockReset();
  });

  afterEach(() => {
    closePopover(false);
    registerNodeDescriptors([]);
    vi.useRealTimers();
  });

  it("declined confirmation for a code-executing node: nothing runs", async () => {
    vi.mocked(showConfirm).mockResolvedValue(false);
    await openPopover(makeNode("decl", "shell_exec"));
    await clickTest();

    expect(showConfirm).toHaveBeenCalledTimes(1);
    expect(runCalls()).toBe(0);
  });

  it("approved once: runs, is not asked again for that node, and a non-dangerous node is never asked", async () => {
    vi.mocked(showConfirm).mockResolvedValue(true);
    await openPopover(makeNode("appr", "shell_exec"));
    await clickTest();
    await clickTest();

    expect(showConfirm).toHaveBeenCalledTimes(1);
    expect(runCalls()).toBe(2);

    await openPopover(makeNode("plain", "http_request"));
    await clickTest();
    expect(showConfirm).toHaveBeenCalledTimes(1);
    expect(runCalls()).toBe(3);
  });

  it("clicking, Esc, or Tab while the confirm dialog is showing leaves the popover alone", async () => {
    await openPopover(makeNode("keep", "http_request"));
    const modal = document.getElementById("confirm-modal")!;
    modal.classList.remove("hidden");

    modal.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await vi.advanceTimersByTimeAsync(200);
    expect(document.getElementById("node-popover")).not.toBeNull();

    const focusable = [...document.querySelectorAll<HTMLElement>(
      '#node-popover input, #node-popover select, #node-popover textarea, #node-popover button, #node-popover [tabindex]:not([tabindex="-1"])',
    )].filter(el => !(el as HTMLButtonElement).disabled);
    focusable[focusable.length - 1].focus();
    const tab = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    document.dispatchEvent(tab);
    expect(tab.defaultPrevented).toBe(false);

    modal.classList.add("hidden");
    modal.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(200);
    expect(document.getElementById("node-popover")).toBeNull();
  });
});
