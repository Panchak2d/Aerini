// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createReveal, REVEAL_CAP_MS } from "../startup-reveal";

let frames: FrameRequestCallback[] = [];

function paint(): void {
  const queued = frames; frames = [];
  for (const f of queued) f(0);
}

beforeEach(() => {
  frames = [];
  vi.useFakeTimers();
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => { frames.push(cb); return frames.length; });
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

describe("window reveal at launch", () => {
  it("waits for two paints after content is ready, then shows once", () => {
    const show = vi.fn();
    const reveal = createReveal(show);
    reveal.afterPaint();
    expect(show).not.toHaveBeenCalled();
    paint();
    expect(show).not.toHaveBeenCalled();
    paint();
    expect(show).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(REVEAL_CAP_MS * 2);
    expect(show).toHaveBeenCalledTimes(1);
  });

  it("shows at the cap when content never becomes ready", () => {
    const show = vi.fn();
    createReveal(show);
    vi.advanceTimersByTime(REVEAL_CAP_MS - 1);
    expect(show).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(show).toHaveBeenCalledTimes(1);
  });

  it("shows at the cap even when the browser never delivers a frame", () => {
    const show = vi.fn();
    createReveal(show).afterPaint();
    vi.advanceTimersByTime(REVEAL_CAP_MS);
    expect(show).toHaveBeenCalledTimes(1);
    paint(); paint();
    expect(show).toHaveBeenCalledTimes(1);
  });

  it("fire() shows immediately and cancels the cap", () => {
    const show = vi.fn();
    const reveal = createReveal(show);
    reveal.fire();
    reveal.fire();
    vi.advanceTimersByTime(REVEAL_CAP_MS * 2);
    expect(show).toHaveBeenCalledTimes(1);
  });
});
