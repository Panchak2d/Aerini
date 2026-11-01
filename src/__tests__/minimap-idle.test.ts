// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { Minimap } from "../canvas/Minimap";
import type { Canvas } from "../canvas/Canvas";
import type { CanvasNode } from "../canvas/Node";

function fakeNode(id: string, x: number): CanvasNode {
  return {
    data: { id, name: id, position: { x, y: 0 }, config: {}, node_type_id: "http", node_type: "action" },
    height: 80, status: "idle", selected: false, disabled: false, ports: [],
  } as unknown as CanvasNode;
}

function setup() {
  const nodes = new Map([["a", fakeNode("a", 0)], ["b", fakeNode("b", 300)]]);
  const canvas = { nodes, connectors: new Map(), panX: 0, panY: 0, zoom: 1, onViewportChange: vi.fn() };
  const el = document.createElement("canvas");
  Object.defineProperty(el, "clientWidth", { value: 200 });
  Object.defineProperty(el, "clientHeight", { value: 120 });
  const noop = new Proxy({}, { get: (_t, k) => (k === "measureText" ? () => ({ width: 10 }) : () => {}), set: () => true });
  vi.spyOn(el, "getContext").mockReturnValue(noop as unknown as CanvasRenderingContext2D);
  const mm = new Minimap(canvas as unknown as Canvas, el);
  return { canvas, nodes, mm };
}

describe("minimap repaint", () => {
  it("skips frames whose inputs match the last paint", () => {
    const { mm } = setup();
    for (let i = 0; i < 120; i++) mm.draw(800, 600);
    expect(mm.paintCount).toBe(1);
  });

  it("repaints when a node moves, the view pans, a node is selected or the node set changes", () => {
    const { canvas, nodes, mm } = setup();
    mm.draw(800, 600);
    nodes.get("a")!.data.position.x = 40;
    mm.draw(800, 600);
    expect(mm.paintCount).toBe(2);
    canvas.panX = 25;
    mm.draw(800, 600);
    expect(mm.paintCount).toBe(3);
    nodes.get("b")!.selected = true;
    mm.draw(800, 600);
    expect(mm.paintCount).toBe(4);
    nodes.set("c", fakeNode("c", 600));
    mm.draw(800, 600);
    expect(mm.paintCount).toBe(5);
  });

  it("repaints a replaced workflow even when every position and name is unchanged", () => {
    const { canvas, mm } = setup();
    mm.draw(800, 600);
    canvas.nodes = new Map([["x", fakeNode("a", 0)], ["y", fakeNode("b", 300)]]);
    mm.draw(800, 600);
    expect(mm.paintCount).toBe(2);
  });

  it("keeps repainting every frame while a node is running", () => {
    const { nodes, mm } = setup();
    nodes.get("a")!.status = "running";
    for (let i = 0; i < 5; i++) mm.draw(800, 600);
    expect(mm.paintCount).toBe(5);
    nodes.get("a")!.status = "success";
    mm.draw(800, 600);
    mm.draw(800, 600);
    expect(mm.paintCount).toBe(6);
  });
});
