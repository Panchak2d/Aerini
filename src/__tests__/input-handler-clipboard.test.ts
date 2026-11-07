/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import { InputHandler } from "../canvas/InputHandler";

function makeCanvas(over: Record<string, unknown> = {}) {
  return {
    el: { classList: { add: vi.fn(), remove: vi.fn() }, style: { cursor: "" } },
    isGestureActive: vi.fn(() => false),
    copySelected: vi.fn(() => true),
    cutSelected: vi.fn(() => true),
    pasteClipboard: vi.fn(() => true),
    s2w: vi.fn((x: number, y: number) => ({ x: x * 2, y: y * 2 })),
    ...over,
  };
}

const key = (k: string, init: KeyboardEventInit = {}): KeyboardEvent =>
  new KeyboardEvent("keydown", { key: k, ctrlKey: true, cancelable: true, ...init });

beforeEach(() => { window.getSelection()?.removeAllRanges(); });

describe("InputHandler clipboard keys", () => {
  it("Ctrl+C, Ctrl+X and Ctrl+V call the canvas and suppress the browser default", () => {
    const canvas = makeCanvas();
    const input = new InputHandler(canvas as unknown as Canvas);
    const copy = key("c"), cut = key("x"), paste = key("v");
    input.onKey(copy); input.onKey(cut); input.onKey(paste);
    expect(canvas.copySelected).toHaveBeenCalledTimes(1);
    expect(canvas.cutSelected).toHaveBeenCalledTimes(1);
    expect(canvas.pasteClipboard).toHaveBeenCalledWith(null);
    expect([copy, cut, paste].every(e => e.defaultPrevented)).toBe(true);
  });

  it("leaves the browser default alone when there is nothing to copy or paste", () => {
    const canvas = makeCanvas({ copySelected: vi.fn(() => false), pasteClipboard: vi.fn(() => false) });
    const input = new InputHandler(canvas as unknown as Canvas);
    const copy = key("c"), paste = key("v");
    input.onKey(copy); input.onKey(paste);
    expect(copy.defaultPrevented).toBe(false);
    expect(paste.defaultPrevented).toBe(false);
  });

  it("does nothing while a drag is in flight, on key repeat, or with Shift held", () => {
    const busy = makeCanvas({ isGestureActive: vi.fn(() => true) });
    new InputHandler(busy as unknown as Canvas).onKey(key("v"));
    expect(busy.pasteClipboard).not.toHaveBeenCalled();

    const canvas = makeCanvas();
    const input = new InputHandler(canvas as unknown as Canvas);
    input.onKey(key("v", { repeat: true }));
    input.onKey(key("c", { shiftKey: true }));
    expect(canvas.pasteClipboard).not.toHaveBeenCalled();
    expect(canvas.copySelected).not.toHaveBeenCalled();
  });

  it("keeps native copy and cut when page text is selected", () => {
    const p = document.createElement("p");
    p.textContent = "output text";
    document.body.appendChild(p);
    window.getSelection()?.selectAllChildren(p);
    const canvas = makeCanvas();
    new InputHandler(canvas as unknown as Canvas).onKey(key("c"));
    expect(canvas.copySelected).not.toHaveBeenCalled();
  });
});
