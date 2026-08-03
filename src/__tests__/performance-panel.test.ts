import { describe, it, expect } from "vitest";
import { buildGraphSvg } from "../panels/PerformancePanel";

describe("buildGraphSvg", () => {
  it("edge case: fewer than 2 samples renders the empty state, not a degenerate/empty svg", () => {
    expect(buildGraphSvg([])).toContain("Not enough samples yet");
    expect(buildGraphSvg([{ at_ms: 0, bytes: 100 }])).toContain("Not enough samples yet");
  });

  it("normal case: maps samples to a polyline spanning the full 0..100 x / 0..40 y viewBox", () => {
    const svg = buildGraphSvg([
      { at_ms: 0, bytes: 0 },
      { at_ms: 500, bytes: 50 },
      { at_ms: 1000, bytes: 100 },
    ]);
    // First point: x=0 (earliest timestamp), y=40 (lowest byte value maps
    // to the bottom of the viewBox, since SVG y grows downward).
    expect(svg).toContain("0.00,40.00");
    // Last point: x=100 (latest timestamp), y=0 (highest byte value maps
    // to the top).
    expect(svg).toContain("100.00,0.00");
    // Midpoint: linear in both axes here, so exactly centered.
    expect(svg).toContain("50.00,20.00");
  });

  it("edge case: a perfectly flat line (all samples equal bytes) doesn't divide by zero", () => {
    const svg = buildGraphSvg([
      { at_ms: 0, bytes: 42 },
      { at_ms: 1000, bytes: 42 },
    ]);
    expect(svg).not.toContain("NaN");
    expect(svg).toContain("0.00,40.00");
    expect(svg).toContain("100.00,40.00");
  });

  it("edge case: all samples sharing one timestamp doesn't divide by zero", () => {
    const svg = buildGraphSvg([
      { at_ms: 5000, bytes: 10 },
      { at_ms: 5000, bytes: 20 },
    ]);
    expect(svg).not.toContain("NaN");
  });
});
