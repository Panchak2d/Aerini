import { describe, it, expect } from "vitest";
import { fitFrameRect, FRAME_PAD, FRAME_HEADER, FRAME_MIN_W, FRAME_MIN_H } from "../canvas/note-geometry";

describe("fitFrameRect", () => {
  it("wraps the contents with padding on every side and a title band on top", () => {
    const r = fitFrameRect([{ x: 100, y: 200, w: 220, h: 60 }, { x: 400, y: 300, w: 220, h: 80 }])!;
    expect(r.x).toBe(100 - FRAME_PAD);
    expect(r.y).toBe(200 - FRAME_PAD - FRAME_HEADER);
    expect(r.w).toBe(520 + FRAME_PAD * 2);
    expect(r.h).toBe(180 + FRAME_PAD * 2 + FRAME_HEADER);
  });

  it("returns null with nothing to wrap and never goes below the frame minimum", () => {
    expect(fitFrameRect([])).toBeNull();
    const tiny = fitFrameRect([{ x: 0, y: 0, w: 1, h: 1 }])!;
    expect(tiny.w).toBeGreaterThanOrEqual(FRAME_MIN_W);
    expect(tiny.h).toBeGreaterThanOrEqual(FRAME_MIN_H);
  });
});
