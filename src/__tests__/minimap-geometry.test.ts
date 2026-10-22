import { describe, it, expect } from "vitest";
import {
  activeBounds, backingSize, computeBounds, DEFAULT_VIEW, MAX_VIEW_ZOOM, contentPoint, dragCenter, edgeArrow, EDGE_INSET, floorRect, grabOffset,
  hitTopmost, hoverText, minimapLabel, minimapToWorld, nextErrorIndex, normalizeDpr, placeLabel, pulseAlpha, snapStroke,
  viewBounds, viewportOnContent, wireControlOffset, zoomView, wireKind, worldToMinimap, type Rect,
} from "../canvas/Minimap";

const NODE: Rect = { x: 100, y: 200, w: 220, h: 100 };

describe("computeBounds", () => {
  it("fits the nodes plus a margin, inset from the minimap edge", () => {
    const b = computeBounds([NODE], { w: 140, h: 80 })!;
    expect(b).toMatchObject({ minX: 70, minY: 170, scale: (80 - 2 * EDGE_INSET) / 160, offY: EDGE_INSET });
    expect(b.offX).toBeGreaterThan(EDGE_INSET);
  });

  it("spans every node, so the farthest ones still land inside the inset", () => {
    const far: Rect = { x: 4000, y: -900, w: 220, h: 100 };
    const size = { w: 140, h: 80 };
    const b = computeBounds([NODE, far], size)!;
    const tr = worldToMinimap(b, far.x + far.w + 30, far.y - 30), bl = worldToMinimap(b, NODE.x - 30, NODE.y + NODE.h + 30);
    expect(tr.x).toBeLessThanOrEqual(size.w - EDGE_INSET + 1e-9);
    expect(tr.y).toBeGreaterThanOrEqual(EDGE_INSET - 1e-9);
    expect(bl.x).toBeGreaterThanOrEqual(EDGE_INSET - 1e-9);
    expect(bl.y).toBeLessThanOrEqual(size.h - EDGE_INSET + 1e-9);
  });

  it("returns null for an empty graph and for a minimap no larger than twice the inset", () => {
    expect(computeBounds([], { w: 100, h: 50 })).toBeNull();
    expect(computeBounds([NODE], { w: 0, h: 80 })).toBeNull();
    expect(computeBounds([NODE], { w: 2 * EDGE_INSET, h: 80 })).toBeNull();
  });
});

describe("pointer mapping", () => {
  it("maps the center of the displayed content box to the center of the world bounds, border included", () => {
    const b = computeBounds([NODE], { w: 148, h: 88 })!;
    const p = contentPoint(575, 345, { left: 500, top: 300 }, { left: 1, top: 1 });
    expect(p).toEqual({ x: 74, y: 44 });
    const w = minimapToWorld(b, p.x, p.y);
    expect(w.x).toBeCloseTo(210, 6);
    expect(w.y).toBeCloseTo(250, 6);
  });

  it("round-trips a world point when the content is letterboxed", () => {
    const b = computeBounds([NODE], { w: 200, h: 80 })!;
    expect(b.offX).toBeGreaterThan(EDGE_INSET);
    const m = worldToMinimap(b, 200, 250);
    const w = minimapToWorld(b, m.x, m.y);
    expect(w.x).toBeCloseTo(200, 6);
    expect(w.y).toBeCloseTo(250, 6);
  });

  it("clamps a pointer outside the content to the edge of the world bounds", () => {
    const b = computeBounds([NODE], { w: 148, h: 88 })!;
    const w = minimapToWorld(b, -50, 999);
    expect(w.x).toBeCloseTo(70, 6);
    expect(w.y).toBeCloseTo(330, 6);
  });
});

describe("drag", () => {
  const vp: Rect = { x: 100, y: 100, w: 100, h: 50 };

  it("keeps the grab offset when pressed inside the viewport and centers on the point otherwise", () => {
    const inside = grabOffset({ x: 110, y: 110 }, vp);
    expect(inside).toEqual({ x: -40, y: -15 });
    expect(dragCenter({ x: 120, y: 130 }, inside)).toEqual({ x: 160, y: 145 });
    expect(grabOffset({ x: 0, y: 0 }, vp)).toEqual({ x: 0, y: 0 });
  });

  it("uses the bounds frozen at drag start instead of the live ones", () => {
    const frozen = computeBounds([NODE], { w: 140, h: 80 })!;
    const live = computeBounds([NODE, { x: 2000, y: 2000, w: 220, h: 100 }], { w: 140, h: 80 })!;
    expect(activeBounds({ bounds: frozen, offset: { x: 0, y: 0 }, moved: false }, live)).toBe(frozen);
    expect(activeBounds(null, live)).toBe(live);
  });
});

describe("viewport indicator", () => {
  const size = { w: 140, h: 80 };
  const b = computeBounds([NODE], size)!;

  it("treats the viewport as on the content only when a visible part of it overlaps the content area", () => {
    expect(viewportOnContent(b, { x: 150, y: 220, w: 100, h: 50 })).toBe(true);
    expect(viewportOnContent(b, { x: -1000, y: -1000, w: 3000, h: 3000 })).toBe(true);
    expect(viewportOnContent(b, { x: 150, y: 220, w: 1, h: 1 })).toBe(true);
    expect(viewportOnContent(b, { x: 1000, y: 1000, w: 100, h: 100 })).toBe(false);
    expect(viewportOnContent(b, { x: 350, y: 220, w: 100, h: 50 })).toBe(false);
    expect(viewportOnContent(b, { x: 349, y: 220, w: 100, h: 50 })).toBe(false);
  });

  it("still finds the viewport on content that is only a thin strip", () => {
    const strip = computeBounds([NODE, { x: 20000, y: 200, w: 220, h: 100 }], size)!;
    expect(viewportOnContent(strip, { x: 10000, y: 220, w: 2000, h: 50 })).toBe(true);
  });

  it("points the arrow from the minimap center toward the viewport with every vertex inside the minimap", () => {
    const right = edgeArrow(b, { x: 5000, y: 250, w: 100, h: 50 })!;
    expect(right[0].x).toBeCloseTo(size.w - EDGE_INSET, 6);
    expect(right[0].y).toBeCloseTo(size.h / 2, 0);
    expect(right[1].x).toBeLessThan(right[0].x);
    expect(right[2].x).toBeLessThan(right[0].x);
    for (let deg = 0; deg < 360; deg += 15) {
      const rad = (deg * Math.PI) / 180;
      const a = edgeArrow(b, { x: 210 + 9000 * Math.cos(rad) - 50, y: 250 + 9000 * Math.sin(rad) - 25, w: 100, h: 50 })!;
      for (const p of a) {
        expect(p.x).toBeGreaterThanOrEqual(1);
        expect(p.x).toBeLessThanOrEqual(size.w - 1);
        expect(p.y).toBeGreaterThanOrEqual(1);
        expect(p.y).toBeLessThanOrEqual(size.h - 1);
      }
    }
    expect(edgeArrow(b, { x: NaN, y: 0, w: 1, h: 1 })).toBeNull();
  });

  it("snaps a stroke center to the middle of a device pixel for odd widths and to a pixel edge for even ones", () => {
    expect(snapStroke(10.3, 1, 1)).toBe(10.5);
    expect(snapStroke(10.7, 1, 1)).toBe(10.5);
    expect(snapStroke(10.2, 1, 2)).toBe(10);
    expect(snapStroke(10.6, 1, 2)).toBe(10.5);
  });
});

describe("wires and running pulse", () => {
  it("ranks an active wire over a selected one over the rest, and reaches as far as the canvas wire does", () => {
    expect(wireKind(true, true, true)).toBe("active");
    expect(wireKind(false, true, false)).toBe("selected");
    expect(wireKind(false, false, true)).toBe("selected");
    expect(wireKind(false, false, false)).toBe("base");
    expect(wireControlOffset(0, 100)).toBe(80);
    expect(wireControlOffset(0, 400)).toBeCloseTo(220, 9);
    expect(wireControlOffset(400, 0)).toBeCloseTo(220, 9);
  });

  it("pulses between the minimum and full alpha, and stays at full alpha for reduced motion", () => {
    expect(pulseAlpha(300, true)).toBe(1);
    expect(pulseAlpha(300, false)).toBeCloseTo(1, 9);
    expect(pulseAlpha(900, false)).toBeCloseTo(0.65, 9);
    for (let t = 0; t < 2400; t += 100) {
      expect(pulseAlpha(t, false)).toBeGreaterThanOrEqual(0.65 - 1e-9);
      expect(pulseAlpha(t, false)).toBeLessThanOrEqual(1 + 1e-9);
    }
  });
});

describe("hover label and error cycling", () => {
  it("hits the topmost mark, with a small slop around it", () => {
    const rects: Rect[] = [{ x: 10, y: 10, w: 20, h: 20 }, { x: 20, y: 20, w: 4, h: 3 }];
    expect(hitTopmost(rects, { x: 22, y: 21 })).toBe(1);
    expect(hitTopmost(rects, { x: 27, y: 22 })).toBe(0);
    expect(hitTopmost(rects, { x: 31.5, y: 15 })).toBe(0);
    expect(hitTopmost(rects, { x: 33, y: 15 })).toBe(-1);
  });

  it("describes a node by name, then any status other than idle, then disabled", () => {
    expect(hoverText("Fetch", "idle", false)).toBe("Fetch");
    expect(hoverText("Fetch", "error", true)).toBe("Fetch \u00b7 error \u00b7 disabled");
    expect(hoverText("", "idle", false)).toBe("Untitled");
  });

  it("puts the label above the mark and keeps it inside the minimap", () => {
    const box = { w: 50, h: 14 }, size = { w: 140, h: 80 };
    expect(placeLabel({ x: 70, y: 40, w: 10, h: 6 }, box, size)).toEqual({ x: 50, y: 22 });
    expect(placeLabel({ x: 70, y: 5, w: 10, h: 6 }, box, size)).toEqual({ x: 50, y: 15 });
    expect(placeLabel({ x: 3, y: 40, w: 4, h: 3 }, box, size).x).toBe(EDGE_INSET);
    expect(placeLabel({ x: 130, y: 40, w: 10, h: 6 }, box, size).x).toBe(size.w - EDGE_INSET - box.w);
  });

  it("cycles through the errors in both directions and recovers from a stale index", () => {
    expect(nextErrorIndex(-1, 3, false)).toBe(0);
    expect(nextErrorIndex(-1, 3, true)).toBe(2);
    expect(nextErrorIndex(2, 3, false)).toBe(0);
    expect(nextErrorIndex(0, 3, true)).toBe(2);
    expect(nextErrorIndex(5, 3, false)).toBe(0);
    expect(nextErrorIndex(0, 0, false)).toBe(-1);
  });
});

describe("sizing helpers", () => {
  it("grows a too-small node mark around its own center", () => {
    expect(floorRect({ x: 10, y: 10, w: 2, h: 1 }, 4, 3)).toEqual({ x: 9, y: 9, w: 4, h: 3 });
    expect(floorRect({ x: 0, y: 0, w: 10, h: 8 }, 4, 3)).toEqual({ x: 0, y: 0, w: 10, h: 8 });
  });

  it("scales the backing store by the pixel ratio and ignores invalid ratios", () => {
    expect(backingSize({ w: 148, h: 88 }, 1.5)).toEqual({ w: 222, h: 132 });
    expect(normalizeDpr(NaN)).toBe(1);
    expect(normalizeDpr(0)).toBe(1);
  });

  it("describes the node and error counts for assistive tech", () => {
    expect(minimapLabel(0, 0)).toBe("Workflow minimap: empty");
    expect(minimapLabel(1, 0)).toBe("Workflow minimap: 1 node");
    expect(minimapLabel(12, 1)).toBe("Workflow minimap: 12 nodes, 1 with an error");
    expect(minimapLabel(12, 3)).toBe("Workflow minimap: 12 nodes, 3 with errors");
  });
});

describe("computeBounds with a canvas viewport", () => {
  const size = { w: 140, h: 80 };

  it("a viewport inside the nodes changes nothing", () => {
    const inside: Rect = { x: 150, y: 220, w: 40, h: 30 };
    expect(computeBounds([NODE], size, inside)).toEqual(computeBounds([NODE], size));
  });

  it("a huge viewport grows the box by at most 2x, so the nodes keep a readable share", () => {
    const huge: Rect = { x: -50000, y: -50000, w: 100000, h: 100000 };
    const plain = computeBounds([NODE], size)!, grown = computeBounds([NODE], size, huge)!;
    expect(grown.scale).toBeLessThan(plain.scale);
    expect(grown.scale).toBeGreaterThanOrEqual(plain.scale / 2 - 1e-9);
  });

  it("a viewport far from the nodes is ignored", () => {
    const away: Rect = { x: 90000, y: 90000, w: 800, h: 600 };
    expect(computeBounds([NODE], size, away)).toEqual(computeBounds([NODE], size));
  });
});

describe("minimap own zoom (viewBounds / zoomView)", () => {
  const base = computeBounds([NODE], { w: 140, h: 80 })!;

  it("zoom 1 is the fitted base, untouched", () => {
    expect(viewBounds(base, DEFAULT_VIEW)).toBe(base);
    expect(viewBounds(null, DEFAULT_VIEW)).toBeNull();
  });

  it("wheel-in keeps the world point under the pointer fixed", () => {
    const p = { x: 100, y: 30 };
    const before = minimapToWorld(base, p.x, p.y);
    const view = zoomView(base, DEFAULT_VIEW, -100, p);
    const zoomed = viewBounds(base, view)!;
    expect(view.zoom).toBeGreaterThan(1);
    expect(zoomed.scale).toBeGreaterThan(base.scale);
    const after = minimapToWorld(zoomed, p.x, p.y);
    expect(after.x).toBeCloseTo(before.x, 6);
    expect(after.y).toBeCloseTo(before.y, 6);
  });

  it("zoom is capped, and zooming back out to 1 resets the pan", () => {
    let v = DEFAULT_VIEW;
    for (let i = 0; i < 60; i++) v = zoomView(base, v, -100, { x: 70, y: 40 });
    expect(v.zoom).toBe(MAX_VIEW_ZOOM);
    for (let i = 0; i < 60; i++) v = zoomView(base, v, 100, { x: 70, y: 40 });
    expect(v).toEqual(DEFAULT_VIEW);
  });

  it("zoom scales with the wheel delta: a pinch's small delta zooms less than a notch, and a huge delta is capped", () => {
    const p = { x: 70, y: 40 };
    const notch = zoomView(base, DEFAULT_VIEW, -100, p).zoom;
    expect(notch).toBeCloseTo(1.25, 6);
    expect(zoomView(base, DEFAULT_VIEW, -10, p).zoom).toBeLessThan(notch);
    expect(zoomView(base, DEFAULT_VIEW, -100000, p).zoom).toBeCloseTo(zoomView(base, DEFAULT_VIEW, -300, p).zoom, 9);
  });

  it("the magnified content always still covers the panel", () => {
    const v = zoomView(base, DEFAULT_VIEW, -100, { x: 0, y: 0 });
    const b = viewBounds(base, { ...v, panX: 99999, panY: -99999 })!;
    expect(b.offX).toBeLessThanOrEqual(0);
    expect(b.offY).toBeLessThanOrEqual(0);
  });
});
