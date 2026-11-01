import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createViewportPersist } from "../viewport-persist";

let id: string | null;
let vp: { panX: number; panY: number; zoom: number };
const write = vi.fn();

function make() {
  return createViewportPersist({ currentId: () => id, read: () => vp, write });
}

beforeEach(() => { vi.useFakeTimers(); write.mockClear(); id = "wf1"; vp = { panX: 0, panY: 0, zoom: 1 }; });
afterEach(() => { vi.useRealTimers(); });

describe("viewport persistence", () => {
  it("turns a burst of changes into one write of the latest view after 200 ms", () => {
    const p = make();
    for (let i = 1; i <= 50; i++) { vp = { panX: i, panY: 0, zoom: 1 }; p.note(); vi.advanceTimersByTime(3); }
    expect(write).not.toHaveBeenCalled();
    vi.advanceTimersByTime(200);
    expect(write).toHaveBeenCalledTimes(1);
    expect(write).toHaveBeenCalledWith("aerini_viewport_wf1", JSON.stringify({ panX: 50, panY: 0, zoom: 1 }));
  });

  it("saves the view under the workflow it was noted for, even after a switch", () => {
    const p = make();
    vp = { panX: 5, panY: 6, zoom: 2 }; p.note();
    id = "wf2"; vp = { panX: 99, panY: 99, zoom: 3 };
    p.flush();
    expect(write).toHaveBeenCalledWith("aerini_viewport_wf1", JSON.stringify({ panX: 5, panY: 6, zoom: 2 }));
  });

  it("flush writes at once, once, and does nothing when nothing is pending", () => {
    const p = make();
    p.flush();
    expect(write).not.toHaveBeenCalled();
    p.note(); p.flush();
    vi.advanceTimersByTime(500);
    expect(write).toHaveBeenCalledTimes(1);
  });

  it("writes nothing while no workflow is open", () => {
    id = null;
    const p = make();
    p.note(); p.flush();
    expect(write).not.toHaveBeenCalled();
  });
});
