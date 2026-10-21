// @vitest-environment jsdom

import { describe, it, expect, vi } from "vitest";
import { Minimap } from "../canvas/Minimap";
import type { Canvas } from "../canvas/Canvas";

function setup() {
  const canvas = { zoomIn: vi.fn(), zoomOut: vi.fn(), fitToScreen: vi.fn() };
  const el = document.createElement("canvas");
  vi.spyOn(el, "getContext").mockReturnValue(null);
  new Minimap(canvas as unknown as Canvas, el);
  return { canvas, el };
}

function wheel(el: HTMLElement, deltaY: number): WheelEvent {
  const e = new WheelEvent("wheel", { deltaY, cancelable: true, bubbles: true });
  el.dispatchEvent(e);
  return e;
}

describe("minimap wheel and double-click", () => {
  it("scroll up zooms in, scroll down zooms out, and the page scroll is cancelled", () => {
    const { canvas, el } = setup();
    expect(wheel(el, -100).defaultPrevented).toBe(true);
    expect(canvas.zoomIn).toHaveBeenCalledTimes(1);
    expect(wheel(el, 100).defaultPrevented).toBe(true);
    expect(canvas.zoomOut).toHaveBeenCalledTimes(1);
  });

  it("a zero-delta wheel event changes nothing", () => {
    const { canvas, el } = setup();
    wheel(el, 0);
    expect(canvas.zoomIn).not.toHaveBeenCalled();
    expect(canvas.zoomOut).not.toHaveBeenCalled();
  });

  it("double-click fits the workflow to the screen", () => {
    const { canvas, el } = setup();
    el.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    expect(canvas.fitToScreen).toHaveBeenCalledTimes(1);
  });
});
