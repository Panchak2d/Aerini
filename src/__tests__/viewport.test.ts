/* @vitest-environment jsdom */
import { describe, it, expect, afterEach } from "vitest";
import { visibleViewport, fitViewports, parseSavedViewport } from "../canvas/viewport";

const rect = (r: Partial<DOMRect>): DOMRect =>
  ({ width: 800, height: 600, left: 0, top: 0, right: 800, bottom: 600, x: 0, y: 0, toJSON() { return {}; }, ...r }) as DOMRect;

function mountChat(offsetWidth: number): void {
  const chat = document.createElement("div");
  chat.id = "chat-panel";
  Object.defineProperty(chat, "offsetWidth", { value: offsetWidth });
  document.body.appendChild(chat);
}

function mountDrawer(top: number): void {
  const drawer = document.createElement("div");
  drawer.id = "output-drawer";
  drawer.getBoundingClientRect = () => rect({ left: 0, right: 800, top, bottom: 600 });
  document.body.appendChild(drawer);
}

afterEach(() => {
  document.getElementById("chat-panel")?.remove();
  document.getElementById("output-drawer")?.remove();
  document.getElementById("minimap-wrap")?.remove();
});

function mountMinimap(r: Partial<DOMRect>): void {
  const wrap = document.createElement("div");
  wrap.id = "minimap-wrap";
  wrap.getBoundingClientRect = () => rect(r);
  document.body.appendChild(wrap);
}

describe("visibleViewport", () => {
  it("returns the full canvas rect when nothing overlays it", () => {
    Object.defineProperty(window, "innerWidth", { value: 800, configurable: true });
    expect(visibleViewport(rect({}))).toEqual({ width: 800, height: 600 });
  });

  it("an open chat panel trims width from the right edge, leaving height alone", () => {
    Object.defineProperty(window, "innerWidth", { value: 800, configurable: true });
    mountChat(300);
    expect(visibleViewport(rect({}))).toEqual({ width: 500, height: 600 });
  });

  it("chat panel and drawer both apply; a closed chat panel (offsetWidth 0) does not", () => {
    Object.defineProperty(window, "innerWidth", { value: 800, configurable: true });
    mountDrawer(400);
    mountChat(300);
    expect(visibleViewport(rect({}))).toEqual({ width: 500, height: 400 });
    document.getElementById("chat-panel")!.remove();
    mountChat(0);
    expect(visibleViewport(rect({}))).toEqual({ width: 800, height: 400 });
  });

  it("edge case: chat panel wider than the canvas clamps to 0, never negative", () => {
    Object.defineProperty(window, "innerWidth", { value: 800, configurable: true });
    mountChat(900);
    expect(visibleViewport(rect({})).width).toBe(0);
  });
});

describe("fitViewports", () => {
  it("without a minimap it is exactly the visible viewport", () => {
    Object.defineProperty(window, "innerWidth", { value: 800, configurable: true });
    expect(fitViewports(rect({}))).toEqual([{ width: 800, height: 600 }]);
  });

  it("an overlapping minimap yields the region above it and the region left of it", () => {
    Object.defineProperty(window, "innerWidth", { value: 800, configurable: true });
    mountMinimap({ left: 640, right: 788, top: 500, bottom: 588, width: 148, height: 88 });
    expect(fitViewports(rect({}))).toEqual([{ width: 800, height: 500 }, { width: 640, height: 600 }]);
  });

  it("a minimap lying entirely below the drawer-trimmed viewport does not trim anything", () => {
    Object.defineProperty(window, "innerWidth", { value: 800, configurable: true });
    mountDrawer(300);
    mountMinimap({ left: 640, right: 788, top: 500, bottom: 588, width: 148, height: 88 });
    expect(fitViewports(rect({}))).toEqual([{ width: 800, height: 300 }]);
  });
});

describe("parseSavedViewport", () => {
  it("accepts a valid entry and clamps zoom into range", () => {
    expect(parseSavedViewport('{"panX":10,"panY":-5,"zoom":2}', 0.05, 4)).toEqual({ panX: 10, panY: -5, zoom: 2 });
    expect(parseSavedViewport('{"panX":0,"panY":0,"zoom":99}', 0.05, 4)?.zoom).toBe(4);
  });

  it("rejects parseable-but-malformed entries that would blank the canvas", () => {
    for (const raw of ["{}", "null", "[]", '"x"', "not json", '{"panX":null,"panY":0,"zoom":1}', '{"panX":0,"panY":0,"zoom":"1"}']) {
      expect(parseSavedViewport(raw, 0.05, 4)).toBeNull();
    }
  });
});
