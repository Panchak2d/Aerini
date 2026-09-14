/* @vitest-environment jsdom */
import { describe, it, expect, vi, afterEach } from "vitest";
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
    centerOn: Canvas.prototype.centerOn,
    zoomAtCenter: (Canvas.prototype as unknown as { zoomAtCenter: (factor: number) => void }).zoomAtCenter,
    onZoomChange: vi.fn(),
    onViewportChange: vi.fn(),
    nodes: new Map(),
    ...overrides,
  };
}

function mountDrawer(rect: Partial<DOMRect>): HTMLElement {
  const drawer = document.createElement("div");
  drawer.id = "output-drawer";
  drawer.getBoundingClientRect = () => ({ width: 0, height: 0, left: 0, top: 0, right: 0, bottom: 0, x: 0, y: 0, toJSON() { return {}; }, ...rect });
  document.body.appendChild(drawer);
  return drawer;
}

afterEach(() => { document.getElementById("output-drawer")?.remove(); });

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

describe("Canvas.fitToScreen — callback wiring", () => {
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

describe("Canvas.fitToScreen — output drawer overlap", () => {
  const nodes = new Map([
    ["n1", { data: { position: { x: 0, y: 0 } }, height: 100 }],
    ["n2", { data: { position: { x: 400, y: 200 } }, height: 100 }],
  ]);

  it("an open drawer isn't display:none, so #canvas's own rect doesn't shrink -- fitToScreen must measure the drawer separately and fit above it, not into the space the drawer already covers", () => {
    // Drawer covers the canvas rect's (800x600) bottom third: y=400..600.
    mountDrawer({ left: 0, right: 800, top: 400, bottom: 600 });
    const fake = makeFakeCanvas({ nodes });
    Canvas.prototype.fitToScreen.call(fake);
    // Height term ((400-160)/300=0.8) now binds tighter than the width term
    // ((800-160)/620≈1.032) that alone governs when the drawer is absent --
    // the bug shipped fitToScreen with the width-term answer even here.
    expect(fake.zoom).toBeCloseTo(0.8, 5);
    expect(fake.panX).toBeCloseTo(152, 5);
    expect(fake.panY).toBeCloseTo(80, 5); // centered in the 0..400 visible strip, not 0..600
  });

  it("a drawer present but .hidden must not affect the fit -- same result as no drawer at all", () => {
    mountDrawer({ left: 0, right: 800, top: 400, bottom: 600 });
    document.getElementById("output-drawer")!.classList.add("hidden");
    const fake = makeFakeCanvas({ nodes });
    Canvas.prototype.fitToScreen.call(fake);
    expect(fake.zoom).toBeCloseTo(640 / 620, 5); // width-constrained, full 600px height available
  });

  it("edge case: drawer covering nearly the full canvas height doesn't invert the zoom (floor guard)", () => {
    mountDrawer({ left: 0, right: 800, top: 10, bottom: 600 }); // only 10px visible
    const fake = makeFakeCanvas({ nodes });
    Canvas.prototype.fitToScreen.call(fake);
    expect(fake.zoom).toBeGreaterThan(0);
    expect(Number.isFinite(fake.zoom)).toBe(true);
  });
});

describe("Canvas.centerOn — output drawer overlap", () => {
  it("centers vertically within the drawer-unobstructed strip, not the full element height", () => {
    // Drawer covers the canvas rect's (800x600) bottom third: y=400..600,
    // leaving y=0..400 (height 400) actually visible.
    mountDrawer({ left: 0, right: 800, top: 400, bottom: 600 });
    const fake = makeFakeCanvas();
    Canvas.prototype.centerOn.call(fake, 100, 50);
    expect(fake.panX).toBeCloseTo(300, 5); // 800/2 - 100*1, horizontal axis untouched by the drawer
    expect(fake.panY).toBeCloseTo(150, 5); // 400/2 - 50*1, not the buggy 600/2 - 50 = 250
  });

  it("a drawer present but .hidden must not affect centering -- same result as no drawer at all", () => {
    mountDrawer({ left: 0, right: 800, top: 400, bottom: 600 });
    document.getElementById("output-drawer")!.classList.add("hidden");
    const fake = makeFakeCanvas();
    Canvas.prototype.centerOn.call(fake, 100, 50);
    expect(fake.panY).toBeCloseTo(250, 5); // full 600px height available
  });
});

describe("Canvas.zoomIn / zoomOut — output drawer overlap", () => {
  it("anchors the zoom on the drawer-unobstructed visible center, not the full element height", () => {
    mountDrawer({ left: 0, right: 800, top: 400, bottom: 600 }); // visible strip: y=0..400
    const fake = makeFakeCanvas();
    Canvas.prototype.zoomIn.call(fake);
    const nz = 1 / 0.9;
    expect(fake.panX).toBeCloseTo(400 - 400 * nz, 5); // horizontal center (400) untouched by the drawer
    expect(fake.panY).toBeCloseTo(200 - 200 * nz, 5); // anchored on 400/2, not the buggy 600/2
  });

  it("a drawer present but .hidden must not affect the anchor -- same result as no drawer at all", () => {
    mountDrawer({ left: 0, right: 800, top: 400, bottom: 600 });
    document.getElementById("output-drawer")!.classList.add("hidden");
    const fake = makeFakeCanvas();
    Canvas.prototype.zoomIn.call(fake);
    const nz = 1 / 0.9;
    expect(fake.panY).toBeCloseTo(300 - 300 * nz, 5); // full 600px height available
  });
});
