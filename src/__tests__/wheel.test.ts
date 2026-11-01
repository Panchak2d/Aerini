import { describe, it, expect } from "vitest";
import { WheelInterpreter, wheelZoomFactor, normalizeDelta, parseScrollMode, type WheelLike } from "../canvas/wheel";

const ev = (o: Partial<WheelLike>): WheelLike => ({ deltaX: 0, deltaY: 0, deltaMode: 0, ctrlKey: false, shiftKey: false, ...o });
const VH = 600;

describe("wheel zoom factor", () => {
  it("gives the same total zoom however a scroll is split into events", () => {
    const one = wheelZoomFactor(ev({ deltaY: -100, wheelDelta: 120 }), VH);
    let many = 1;
    for (let i = 0; i < 10; i++) many *= wheelZoomFactor(ev({ deltaY: -10 }), VH);
    expect(many).toBeCloseTo(one, 10);
  });

  it("treats one mouse notch as the former 1/0.9 step", () => {
    expect(wheelZoomFactor(ev({ deltaY: -100 }), VH)).toBeCloseTo(1 / 0.9, 10);
    expect(wheelZoomFactor(ev({ deltaY: 100 }), VH)).toBeCloseTo(0.9, 10);
  });

  it("gives the same zoom for the same distance in pixels, lines or pages", () => {
    const px = wheelZoomFactor(ev({ deltaY: 99 }), VH);
    expect(wheelZoomFactor(ev({ deltaY: 3, deltaMode: 1 }), VH)).toBeCloseTo(px, 10);
    expect(normalizeDelta(0.5, 2, VH)).toBe(300);
  });

  it("clamps one huge event so a single flick cannot jump the zoom", () => {
    expect(wheelZoomFactor(ev({ deltaY: -100000 }), VH)).toBeCloseTo(Math.exp(120 * Math.log(1 / 0.9) / 100), 10);
    expect(wheelZoomFactor(ev({ deltaY: -100000, ctrlKey: true }), VH)).toBeCloseTo(Math.exp(0.25), 10);
  });

  it("zooms faster per pixel for a pinch (ctrl, small fractional deltas) than for a notch", () => {
    expect(wheelZoomFactor(ev({ deltaY: -5.5, ctrlKey: true }), VH)).toBeCloseTo(Math.exp(0.055), 10);
  });
});

describe("wheel interpretation", () => {
  it("pans on a trackpad scroll (sideways delta present) and zooms on a plain integer wheel", () => {
    const w = new WheelInterpreter();
    expect(w.interpret(ev({ deltaX: 3, deltaY: 12 }), VH, 0)).toEqual({ kind: "pan", dx: 3, dy: 12 });
    expect(new WheelInterpreter().interpret(ev({ deltaY: 100, wheelDelta: -120 }), VH, 0).kind).toBe("zoom");
  });

  it("defaults to zoom when the device is unclear", () => {
    expect(new WheelInterpreter().interpret(ev({ deltaY: 4 }), VH, 0).kind).toBe("zoom");
  });

  it("keeps panning for 250 ms after a trackpad event, then falls back to zoom", () => {
    const w = new WheelInterpreter();
    w.interpret(ev({ deltaX: 1.5, deltaY: 2.5 }), VH, 1000);
    expect(w.interpret(ev({ deltaY: 4 }), VH, 1200).kind).toBe("pan");
    expect(w.interpret(ev({ deltaY: 4 }), VH, 1300).kind).toBe("zoom");
  });

  it("a clear mouse notch ends a trackpad run at once", () => {
    const w = new WheelInterpreter();
    w.interpret(ev({ deltaX: 1.5, deltaY: 2.5 }), VH, 1000);
    expect(w.interpret(ev({ deltaY: 100, wheelDelta: -120 }), VH, 1010).kind).toBe("zoom");
  });

  it("ctrl+wheel always zooms and shift+wheel always pans sideways", () => {
    const w = new WheelInterpreter();
    w.mode = "pan";
    expect(w.interpret(ev({ deltaY: -5.5, ctrlKey: true }), VH, 0).kind).toBe("zoom");
    expect(w.interpret(ev({ deltaY: 100, shiftKey: true }), VH, 0)).toEqual({ kind: "pan", dx: 100, dy: 0 });
  });

  it("the Zoom and Pan settings override device detection", () => {
    const w = new WheelInterpreter();
    w.mode = "zoom";
    expect(w.interpret(ev({ deltaX: 3, deltaY: 12.5 }), VH, 0).kind).toBe("zoom");
    w.mode = "pan";
    expect(w.interpret(ev({ deltaY: 100, wheelDelta: -120 }), VH, 0)).toEqual({ kind: "pan", dx: 0, dy: 100 });
  });

  it("an unknown stored setting reads as Auto", () => {
    expect(parseScrollMode("zoom")).toBe("zoom");
    expect(parseScrollMode("pan")).toBe("pan");
    expect(parseScrollMode("sideways")).toBe("auto");
    expect(parseScrollMode(null)).toBe("auto");
  });
});
