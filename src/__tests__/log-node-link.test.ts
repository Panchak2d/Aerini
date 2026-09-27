/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { RunManager } from "../run-manager";
import { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";
import type { CanvasNodeData } from "../canvas/Node";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

function makeNodeData(id: string): CanvasNodeData {
  return {
    id,
    node_type_id: "http_request",
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

function makeManagerWithCanvas(nodes: Map<string, CanvasNode>): {
  rm: RunManager;
  selectNode: ReturnType<typeof vi.fn>;
  clearSelection: ReturnType<typeof vi.fn>;
} {
  const selectNode = vi.fn();
  const clearSelection = vi.fn();
  const fakeCanvas = { nodes, selectNode, clearSelection } as unknown as Canvas;
  const rm = new RunManager(fakeCanvas, vi.fn(), vi.fn());
  return { rm, selectNode, clearSelection };
}

type PrivateWiring = { wireLogNodeLinks(container: Element): void };

describe("wireLogNodeLinks — click selects the node on canvas", () => {
  it("normal case: clicking a .log-line with a matching data-node-id selects that node", () => {
    const node = new CanvasNode(makeNodeData("n1"));
    const { rm, selectNode, clearSelection } = makeManagerWithCanvas(new Map([["n1", node]]));

    const container = document.createElement("div");
    container.innerHTML = `<div class="log-line" data-node-id="n1" tabindex="0" role="button">line</div>`;
    (rm as unknown as PrivateWiring).wireLogNodeLinks(container);

    container.querySelector<HTMLElement>(".log-line")!.click();

    expect(clearSelection).toHaveBeenCalledTimes(1);
    expect(selectNode).toHaveBeenCalledWith(node);
  });

  it("normal case: pressing Enter on a focused .log-line selects the node, same as a click", () => {
    const node = new CanvasNode(makeNodeData("n2"));
    const { rm, selectNode } = makeManagerWithCanvas(new Map([["n2", node]]));

    const container = document.createElement("div");
    container.innerHTML = `<div class="log-line" data-node-id="n2" tabindex="0" role="button">line</div>`;
    (rm as unknown as PrivateWiring).wireLogNodeLinks(container);

    const line = container.querySelector<HTMLElement>(".log-line")!;
    line.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));

    expect(selectNode).toHaveBeenCalledWith(node);
  });

  it("edge case: a data-node-id with no matching canvas node does not throw and does not select anything", () => {
    const { rm, selectNode, clearSelection } = makeManagerWithCanvas(new Map());

    const container = document.createElement("div");
    container.innerHTML = `<div class="log-line" data-node-id="ghost" tabindex="0" role="button">line</div>`;
    (rm as unknown as PrivateWiring).wireLogNodeLinks(container);

    expect(() => container.querySelector<HTMLElement>(".log-line")!.click()).not.toThrow();
    expect(selectNode).not.toHaveBeenCalled();
    expect(clearSelection).not.toHaveBeenCalled();
  });
});
