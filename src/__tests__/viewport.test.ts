/* @vitest-environment jsdom */
import { describe, it, expect, afterEach } from "vitest";
import { visibleViewport } from "../canvas/viewport";

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
});

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
