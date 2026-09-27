// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";
import { ContextMenu } from "../canvas/ContextMenu";
import { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeNode(): CanvasNode {
  return new CanvasNode({
    id: "n1",
    node_type_id: "http_request",
    node_type: "action",
    name: "Old Node Name",
    config: {},
    credentials: {},
    position: { x: 0, y: 0 },
    ports: {
      inputs:  [{ id: "input",  label: "Input",  position: "left"  }],
      outputs: [{ id: "output", label: "Output", position: "right" }],
    },
    input_schema:  {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  });
}

function makeCanvas(onCanvasChanged = vi.fn()): Canvas {
  const el = document.createElement("div");
  document.body.appendChild(el);
  return {
    el,
    zoom: 1,
    panX: 0,
    panY: 0,
    selectedNodes: new Set(),
    onCanvasChanged,
  } as unknown as Canvas;
}

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("ContextMenu.startInlineRename — commit on outside click", () => {
  it("normal case: a mousedown outside the input commits the new name and fires onCanvasChanged", async () => {
    const onCanvasChanged = vi.fn();
    const canvas = makeCanvas(onCanvasChanged);
    const node = makeNode();
    const menu = new ContextMenu(canvas);

    menu.startInlineRename(node);
    const inp = document.getElementById("canvas-rename-input") as HTMLInputElement;
    inp.value = "New Node Name";

    await new Promise(r => setTimeout(r, 0));
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));

    expect(node.data.name).toBe("New Node Name");
    expect(onCanvasChanged).toHaveBeenCalled();
    expect(document.getElementById("canvas-rename-input")).toBeNull();
  });

  it("edge case: the input is removed synchronously, before a click reaching a sibling node can be affected by it", async () => {
    const canvas = makeCanvas();
    const node = makeNode();
    const menu = new ContextMenu(canvas);

    menu.startInlineRename(node);
    await new Promise(r => setTimeout(r, 0));

    let inputPresentDuringCapture = true;
    document.addEventListener(
      "mousedown",
      () => { inputPresentDuringCapture = document.getElementById("canvas-rename-input") !== null; },
      { capture: false }, // bubble phase runs after the capture-phase dismiss listener
    );

    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));

    expect(inputPresentDuringCapture).toBe(false);
  });
});
