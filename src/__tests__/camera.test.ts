import { describe, it, expect, vi, afterEach } from "vitest";
import { moveCamera, stepCamera, cancelCamera, CAMERA_EASE_MS, type CameraHost } from "../canvas/camera";

const withMotionPreference = (reduce: boolean) => vi.stubGlobal("window", { matchMedia: () => ({ matches: reduce }) });
const motionAllowed = () => withMotionPreference(false);
const host = (o: Partial<CameraHost> = {}): CameraHost => ({ panX: 0, panY: 0, zoom: 1, onZoomChange: vi.fn(), onViewportChange: vi.fn(), ...o });

afterEach(() => { vi.unstubAllGlobals(); });

describe("camera easing", () => {
  it("eases to the target over about 180 ms and reports the final view once", () => {
    motionAllowed();
    const h = host();
    moveCamera(h, { panX: 300, panY: -60, zoom: 2 });
    expect(h.panX).toBe(0);
    expect(h.camAnim).toBeTruthy();
    expect(stepCamera(h, 1000)).toBe(true);
    stepCamera(h, 1000 + CAMERA_EASE_MS / 2);
    expect(h.panX).toBeGreaterThan(150);
    expect(h.panX).toBeLessThan(300);
    expect(stepCamera(h, 1000 + CAMERA_EASE_MS)).toBe(false);
    expect([h.panX, h.panY, h.zoom]).toEqual([300, -60, 2]);
    expect(h.camAnim).toBeNull();
    expect(h.onViewportChange).toHaveBeenCalledTimes(1);
  });

  it("jumps at once when the user prefers reduced motion", () => {
    withMotionPreference(true);
    const h = host();
    moveCamera(h, { panX: 300, panY: 0, zoom: 1 });
    expect(h.panX).toBe(300);
    expect(h.camAnim).toBeFalsy();
  });

  it("jumps at once right after a workflow's maps were replaced, but eases later", () => {
    motionAllowed();
    const fresh = host({ mapsReplacedAt: performance.now() });
    moveCamera(fresh, { panX: 300, panY: 0, zoom: 1 });
    expect(fresh.panX).toBe(300);
    const settled = host({ mapsReplacedAt: performance.now() - 600 });
    moveCamera(settled, { panX: 300, panY: 0, zoom: 1 });
    expect(settled.panX).toBe(0);
    expect(settled.camAnim).toBeTruthy();
  });

  it("stops where it is when cancelled and does not report a finished move", () => {
    motionAllowed();
    const h = host();
    moveCamera(h, { panX: 300, panY: 0, zoom: 1 });
    stepCamera(h, 0); stepCamera(h, 60);
    const mid = h.panX;
    cancelCamera(h);
    expect(stepCamera(h, 500)).toBe(false);
    expect(h.panX).toBe(mid);
    expect(h.onViewportChange).not.toHaveBeenCalled();
  });

  it("an instant move cancels a running one", () => {
    motionAllowed();
    const h = host();
    moveCamera(h, { panX: 300, panY: 0, zoom: 1 });
    moveCamera(h, { panX: 50, panY: 0, zoom: 1 }, true);
    expect(h.panX).toBe(50);
    expect(h.camAnim).toBeFalsy();
  });
});
