// @vitest-environment jsdom
 
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import { bindZoomControls } from "../toolbar";

function makeFakeCanvas(zoom = 1) {
  return {
    zoom,
    MIN_ZOOM: 0.05,
    MAX_ZOOM: 4,
    zoomIn:  vi.fn(function (this: { zoom: number }) { this.zoom = Math.min(4, this.zoom * 2); }),
    zoomOut: vi.fn(function (this: { zoom: number }) { this.zoom = Math.max(0.05, this.zoom / 2); }),
    fitToScreen: vi.fn(),
  } as unknown as Canvas;
}

describe("bindZoomControls", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = `<div id="toolbar"><div class="toolbar-divider"></div></div>`;
  });
  afterEach(() => { vi.useRealTimers(); });

  it("inserts zoom-out, readout, zoom-in, and fit buttons after the toolbar divider, with a second divider trailing them", () => {
    bindZoomControls(makeFakeCanvas());
    const group = document.querySelector(".toolbar-zoom-group");
    expect(group).toBeTruthy();
    expect(document.getElementById("btn-zoom-out")).toBeTruthy();
    expect(document.getElementById("btn-zoom-in")).toBeTruthy();
    expect(document.getElementById("btn-fit-screen")).toBeTruthy();
    expect(group?.previousElementSibling?.className).toBe("toolbar-divider");
    expect(group?.nextElementSibling?.className).toBe("toolbar-divider");
  });

  it("clicking zoom-in/zoom-out calls the canvas methods, not a reimplementation", () => {
    const canvas = makeFakeCanvas();
    bindZoomControls(canvas);
    document.getElementById("btn-zoom-in")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(canvas.zoomIn).toHaveBeenCalledTimes(1);
    document.getElementById("btn-zoom-out")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(canvas.zoomOut).toHaveBeenCalledTimes(1);
  });

  it("clicking fit calls canvas.fitToScreen()", () => {
    const canvas = makeFakeCanvas();
    bindZoomControls(canvas);
    document.getElementById("btn-fit-screen")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(canvas.fitToScreen).toHaveBeenCalledTimes(1);
  });

  it("readout reflects canvas.zoom on the next poll tick, whatever changed it (not just this file's own buttons)", () => {
    const canvas = makeFakeCanvas(1);
    bindZoomControls(canvas);
    expect(document.querySelector(".toolbar-zoom-readout")!.textContent).toBe("100%");
    (canvas as unknown as { zoom: number }).zoom = 2.5; // simulate a wheel-zoom from InputHandler.ts
    vi.advanceTimersByTime(300);
    expect(document.querySelector(".toolbar-zoom-readout")!.textContent).toBe("250%");
  });

  it("disables zoom-out at MIN_ZOOM and zoom-in at MAX_ZOOM", () => {
    const canvas = makeFakeCanvas(0.05);
    bindZoomControls(canvas);
    vi.advanceTimersByTime(300);
    expect((document.getElementById("btn-zoom-out") as HTMLButtonElement).disabled).toBe(true);
    expect((document.getElementById("btn-zoom-in")  as HTMLButtonElement).disabled).toBe(false);

    (canvas as unknown as { zoom: number }).zoom = 4;
    vi.advanceTimersByTime(300);
    expect((document.getElementById("btn-zoom-out") as HTMLButtonElement).disabled).toBe(false);
    expect((document.getElementById("btn-zoom-in")  as HTMLButtonElement).disabled).toBe(true);
  });

  it("does nothing if the toolbar divider isn't present (defensive, no throw)", () => {
    document.body.innerHTML = `<div id="toolbar"></div>`;
    expect(() => bindZoomControls(makeFakeCanvas())).not.toThrow();
    expect(document.querySelector(".toolbar-zoom-group")).toBeNull();
  });
});
