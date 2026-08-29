/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import { InputHandler } from "../canvas/InputHandler";

function makeFakeCanvas() {
  return {
    el: { classList: { add: vi.fn(), remove: vi.fn() }, style: { cursor: "" } },
    toggleFocusMode: vi.fn(),
  };
}

describe("InputHandler — F-key focus-mode toggle", () => {
  it("normal case: a bare 'f' (or Shift+f, i.e. 'F') toggles focus mode", () => {
    const canvas = makeFakeCanvas();
    const input = new InputHandler(canvas as unknown as Canvas);

    input.onKey(new KeyboardEvent("keydown", { key: "f" }));
    expect(canvas.toggleFocusMode).toHaveBeenCalledTimes(1);

    input.onKey(new KeyboardEvent("keydown", { key: "F", shiftKey: true }));
    expect(canvas.toggleFocusMode).toHaveBeenCalledTimes(2);
  });

  it("Ctrl+F and Cmd+F (no Shift) toggle focus mode", () => {
    const canvas = makeFakeCanvas();
    const input = new InputHandler(canvas as unknown as Canvas);

    input.onKey(new KeyboardEvent("keydown", { key: "f", ctrlKey: true }));
    expect(canvas.toggleFocusMode).toHaveBeenCalledTimes(1);

    input.onKey(new KeyboardEvent("keydown", { key: "f", metaKey: true }));
    expect(canvas.toggleFocusMode).toHaveBeenCalledTimes(2);
  });

  it("edge case (R-bug5, preserved): Ctrl+Shift+F and Cmd+Shift+F must not toggle focus mode -- that chord is reserved for toolbar.ts's fitToScreen shortcut", () => {
    const canvas = makeFakeCanvas();
    const input = new InputHandler(canvas as unknown as Canvas);

    input.onKey(new KeyboardEvent("keydown", { key: "F", ctrlKey: true, shiftKey: true }));
    input.onKey(new KeyboardEvent("keydown", { key: "F", metaKey: true, shiftKey: true }));

    expect(canvas.toggleFocusMode).not.toHaveBeenCalled();
  });

  it("preventDefault is called for Ctrl+F/Cmd+F (suppresses the browser/webview's native Find) but not for bare f/F", () => {
    const canvas = makeFakeCanvas();
    const input = new InputHandler(canvas as unknown as Canvas);

    const bare = new KeyboardEvent("keydown", { key: "f", cancelable: true });
    input.onKey(bare);
    expect(bare.defaultPrevented).toBe(false);

    const ctrlF = new KeyboardEvent("keydown", { key: "f", ctrlKey: true, cancelable: true });
    input.onKey(ctrlF);
    expect(ctrlF.defaultPrevented).toBe(true);
  });
});
