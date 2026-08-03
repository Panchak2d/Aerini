/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { Canvas } from "../canvas/Canvas";

function makeFakeCanvas(overrides: Partial<Record<string, unknown>> = {}) {
  const rect = { width: 800, height: 600, left: 0, top: 0, right: 800, bottom: 600, x: 0, y: 0, toJSON() { return {}; } };
  return {
    el: { getBoundingClientRect: () => rect },
    zoom: 1,
    panX: 0,
    panY: 0,
    MIN_ZOOM: 0.05,
    MAX_ZOOM: 4,
    s2w: Canvas.prototype.s2w,
    zoomAtCenter: (Canvas.prototype as unknown as { zoomAtCenter: (factor: number) => void }).zoomAtCenter,
    onZoomChange: vi.fn(),
    onViewportChange: vi.fn(),
    nodes: new Map(),
    ...overrides,
  };
}

describe("Canvas.zoomIn / zoomOut", () => {
  it("zoomIn increases zoom by the same 1/0.9 step onWheel already uses, anchored at the viewport center", () => {
    const fake = makeFakeCanvas();
    Canvas.prototype.zoomIn.call(fake);
    expect(fake.zoom).toBeCloseTo(1 / 0.9, 5);
    // Anchored at center (400,300): world point under the center before
    // zooming must land back under the center after — panX/panY shift to
    // compensate, they should not stay at 0.
    expect(fake.panX).not.toBe(0);
    expect(fake.panY).not.toBe(0);
    expect(fake.onZoomChange).toHaveBeenCalledWith(fake.zoom);
    expect(fake.onViewportChange).toHaveBeenCalledTimes(1);
  });

  it("zoomOut decreases zoom by the 0.9 step", () => {
    const fake = makeFakeCanvas();
    Canvas.prototype.zoomOut.call(fake);
    expect(fake.zoom).toBeCloseTo(0.9, 5);
    expect(fake.onZoomChange).toHaveBeenCalledWith(fake.zoom);
  });

  it("zoomIn clamps at MAX_ZOOM and does not overshoot", () => {
    const fake = makeFakeCanvas({ zoom: 3.9 });
    Canvas.prototype.zoomIn.call(fake);
    expect(fake.zoom).toBe(4);
  });

  it("zoomOut clamps at MIN_ZOOM and does not undershoot", () => {
    const fake = makeFakeCanvas({ zoom: 0.052 });
    Canvas.prototype.zoomOut.call(fake);
    expect(fake.zoom).toBe(0.05);
  });

  it("repeated zoomOut calls never go below MIN_ZOOM", () => {
    const fake = makeFakeCanvas({ zoom: 1 });
    for (let i = 0; i < 50; i++) Canvas.prototype.zoomOut.call(fake);
    expect(fake.zoom).toBe(0.05);
  });
});

describe("Canvas.fitToScreen — Batch 2 callback wiring", () => {
  it("does nothing (fires no callbacks) when there are no nodes", () => {
    const fake = makeFakeCanvas({ nodes: new Map() });
    Canvas.prototype.fitToScreen.call(fake);
    expect(fake.onZoomChange).not.toHaveBeenCalled();
    expect(fake.onViewportChange).not.toHaveBeenCalled();
    expect(fake.zoom).toBe(1); // untouched
  });

  it("computes zoom/pan and fires onZoomChange + onViewportChange when nodes exist", () => {
    const nodes = new Map([
      ["n1", { data: { position: { x: 0, y: 0 } }, height: 100 }],
      ["n2", { data: { position: { x: 400, y: 200 } }, height: 100 }],
    ]);
    const fake = makeFakeCanvas({ nodes, zoom: 1 });
    Canvas.prototype.fitToScreen.call(fake);
    expect(fake.onZoomChange).toHaveBeenCalledTimes(1);
    expect(fake.onZoomChange).toHaveBeenCalledWith(fake.zoom);
    expect(fake.onViewportChange).toHaveBeenCalledTimes(1);
    expect(fake.zoom).not.toBe(1); // actually recomputed
  });
});
