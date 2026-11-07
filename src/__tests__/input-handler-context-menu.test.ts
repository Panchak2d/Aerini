// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";
import { InputHandler } from "../canvas/InputHandler";
import type { Canvas } from "../canvas/Canvas";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function fakeCanvas(extra: Record<string, unknown> = {}) {
  const el = document.createElement("div");
  el.tabIndex = 0;
  document.body.appendChild(el);
  return {
    el, zoom: 1, panX: 0, panY: 0, camAnim: null,
    evSX: (e: MouseEvent) => ({ sx: e.clientX, sy: e.clientY }),
    s2w: (x: number, y: number) => ({ x, y }),
    cachedRect: () => ({ left: 0, top: 0, width: 800, height: 600 }),
    nodeAt: () => null,
    noteAt: () => null,
    connectors: new Map(),
    nodes: new Map(),
    selectedNodes: new Set<string>(),
    selectedNode: null,
    isShown: () => true,
    clearSelection: vi.fn(),
    selectConn: vi.fn(),
    selectNode: vi.fn(),
    onPaletteRequest: vi.fn(),
    ctxMenu: { show: vi.fn(), showWire: vi.fn(), showCanvas: vi.fn() },
    ...extra,
  };
}

const mouse = (type: string, x: number, y: number, button = 2): MouseEvent =>
  new MouseEvent(type, { clientX: x, clientY: y, button, bubbles: true, cancelable: true });

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("right-click menu", () => {
  it("opens the canvas menu on a still right-click", () => {
    const c = fakeCanvas();
    const input = new InputHandler(c as unknown as Canvas);
    input.onDown(mouse("mousedown", 100, 100));
    input.onRightClick(mouse("contextmenu", 102, 101));
    expect(c.ctxMenu.showCanvas).toHaveBeenCalledWith(102, 101, { x: 102, y: 101 }, false);
  });

  it("opens nothing when the pointer moved more than a few pixels between press and release", () => {
    const c = fakeCanvas();
    const input = new InputHandler(c as unknown as Canvas);
    input.onDown(mouse("mousedown", 100, 100));
    input.onRightClick(mouse("contextmenu", 140, 100));
    expect(c.ctxMenu.showCanvas).not.toHaveBeenCalled();
  });

  it("shows the wire menu and selects the wire instead of deleting it", () => {
    const wire = { containsPoint: () => true };
    const deleteConnector = vi.fn();
    const c = fakeCanvas({ connectors: new Map([["w", wire]]), deleteConnector });
    const input = new InputHandler(c as unknown as Canvas);
    input.onRightClick(mouse("contextmenu", 50, 60));
    expect(c.ctxMenu.showWire).toHaveBeenCalledWith(50, 60, wire, { x: 50, y: 60 }, false);
    expect(deleteConnector).not.toHaveBeenCalled();
    expect(c.selectConn).toHaveBeenCalledWith(wire);
  });
});

describe("keyboard menu", () => {
  it("opens the selection's menu with focus on the first row on the Menu key", () => {
    const node = { data: { id: "a", position: { x: 10, y: 20 } } };
    const c = fakeCanvas({ selectedNode: node, selectedNodes: new Set(["a"]) });
    const input = new InputHandler(c as unknown as Canvas);
    input.onKey(new KeyboardEvent("keydown", { key: "ContextMenu" }));
    expect(c.ctxMenu.show).toHaveBeenCalledWith(22, 32, node, true);
  });

  it("opens the same menu on Shift+F10", () => {
    const node = { data: { id: "a", position: { x: 0, y: 0 } } };
    const c = fakeCanvas({ selectedNode: node, selectedNodes: new Set(["a"]) });
    const input = new InputHandler(c as unknown as Canvas);
    input.onKey(new KeyboardEvent("keydown", { key: "F10", shiftKey: true }));
    expect(c.ctxMenu.show).toHaveBeenCalledTimes(1);
  });

  it("ignores the contextmenu event the browser sends after the key, so the menu opens once", () => {
    const node = { data: { id: "a", position: { x: 0, y: 0 } } };
    const c = fakeCanvas({ selectedNode: node, selectedNodes: new Set(["a"]) });
    const input = new InputHandler(c as unknown as Canvas);
    input.onKey(new KeyboardEvent("keydown", { key: "ContextMenu" }));
    input.onRightClick(mouse("contextmenu", 0, 0, 0));
    expect(c.ctxMenu.show).toHaveBeenCalledTimes(1);
    expect(c.ctxMenu.showCanvas).not.toHaveBeenCalled();
  });

  it("does not open the canvas menu while a sidebar button has focus", () => {
    document.body.insertAdjacentHTML("beforeend", `<button id="b">x</button>`);
    document.getElementById("b")!.focus();
    const c = fakeCanvas();
    const input = new InputHandler(c as unknown as Canvas);
    input.onKey(new KeyboardEvent("keydown", { key: "ContextMenu" }));
    expect(c.ctxMenu.showCanvas).not.toHaveBeenCalled();
  });
});

describe("palette request", () => {
  it("passes no position when the pointer has not been over the canvas", () => {
    const c = fakeCanvas();
    new InputHandler(c as unknown as Canvas).onKey(new KeyboardEvent("keydown", { code: "Space", key: " " }));
    expect(c.onPaletteRequest).toHaveBeenCalledWith(null);
  });
});
