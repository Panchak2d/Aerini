// @vitest-environment jsdom

import { describe, it, expect, vi } from "vitest";
import { Minimap } from "../canvas/Minimap";
import type { Canvas } from "../canvas/Canvas";
import type { CanvasNode } from "../canvas/Node";

function fakeNode(x: number, y: number): CanvasNode {
  return {
    data: { id: `n${x}`, name: "N", position: { x, y }, config: {}, node_type_id: "http", node_type: "action" },
    height: 80, status: "idle", selected: false, disabled: false, ports: [],
  } as unknown as CanvasNode;
}

function setup(nodes: CanvasNode[] = []) {
  const canvas = {
    zoomIn: vi.fn(), zoomOut: vi.fn(), fitToScreen: vi.fn(), revealNode: vi.fn(),
    nodes: new Map(nodes.map(n => [n.data.id, n])), connectors: new Map(),
    panX: 0, panY: 0, zoom: 1,
  };
  const el = document.createElement("canvas");
  Object.defineProperty(el, "clientWidth", { value: 200 });
  Object.defineProperty(el, "clientHeight", { value: 120 });
  el.getBoundingClientRect = () => ({ left: 0, top: 0, right: 200, bottom: 120, width: 200, height: 120, x: 0, y: 0, toJSON() {} });
  const noop = new Proxy({}, { get: (_t, k) => (k === "measureText" ? () => ({ width: 10 }) : () => {}), set: () => true });
  vi.spyOn(el, "getContext").mockReturnValue(noop as unknown as CanvasRenderingContext2D);
  const mm = new Minimap(canvas as unknown as Canvas, el);
  return { canvas, el, mm };
}

function wheel(el: HTMLElement, deltaY: number, mods: WheelEventInit = {}): WheelEvent {
  const e = new WheelEvent("wheel", { deltaY, cancelable: true, bubbles: true, ...mods });
  el.dispatchEvent(e);
  return e;
}

function dblclick(el: HTMLElement, x: number, y: number): void {
  el.dispatchEvent(new MouseEvent("dblclick", { clientX: x, clientY: y, bubbles: true }));
}

describe("minimap wheel", () => {
  it("plain scroll up zooms the canvas in, scroll down zooms it out, and the page scroll is cancelled", () => {
    const { canvas, el } = setup();
    expect(wheel(el, -100).defaultPrevented).toBe(true);
    expect(canvas.zoomIn).toHaveBeenCalledTimes(1);
    expect(wheel(el, 100).defaultPrevented).toBe(true);
    expect(canvas.zoomOut).toHaveBeenCalledTimes(1);
  });

  it("Ctrl or Cmd + scroll leaves the canvas zoom alone and still cancels the browser's page zoom", () => {
    const { canvas, el } = setup();
    expect(wheel(el, -100, { ctrlKey: true }).defaultPrevented).toBe(true);
    expect(wheel(el, 100, { metaKey: true }).defaultPrevented).toBe(true);
    expect(canvas.zoomIn).not.toHaveBeenCalled();
    expect(canvas.zoomOut).not.toHaveBeenCalled();
  });

  it("a zero-delta wheel event changes nothing", () => {
    const { canvas, el } = setup();
    wheel(el, 0);
    expect(canvas.zoomIn).not.toHaveBeenCalled();
    expect(canvas.zoomOut).not.toHaveBeenCalled();
  });
});

describe("minimap double-click", () => {
  it("on empty space it fits the workflow to the screen and selects nothing", () => {
    const { canvas, el } = setup();
    dblclick(el, 100, 60);
    expect(canvas.fitToScreen).toHaveBeenCalledTimes(1);
    expect(canvas.revealNode).not.toHaveBeenCalled();
  });

  it("on a node it selects and reveals that node instead of fitting; a single click does neither", () => {
    const node = fakeNode(0, 0);
    const { canvas, el, mm } = setup([node]);
    mm.draw(800, 600);
    el.dispatchEvent(new MouseEvent("click", { clientX: 100, clientY: 60, bubbles: true }));
    expect(canvas.revealNode).not.toHaveBeenCalled();
    dblclick(el, 100, 60);
    expect(canvas.revealNode).toHaveBeenCalledWith(node);
    expect(canvas.fitToScreen).not.toHaveBeenCalled();
  });

  it("beside the node's mark it falls back to fit", () => {
    const { canvas, el, mm } = setup([fakeNode(0, 0)]);
    mm.draw(800, 600);
    dblclick(el, 3, 3);
    expect(canvas.revealNode).not.toHaveBeenCalled();
    expect(canvas.fitToScreen).toHaveBeenCalledTimes(1);
  });
});
