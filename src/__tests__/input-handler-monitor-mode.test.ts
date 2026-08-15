/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import { InputHandler } from "../canvas/InputHandler";
import { enterMonitorMode, exitMonitorMode } from "../monitor-mode";

function makeFakeCanvas() {
  return {
    el: { classList: { add: vi.fn(), remove: vi.fn() }, style: { cursor: "" } },
    undo: vi.fn(),
  };
}

beforeEach(() => {
  document.body.className = "";
  document.body.innerHTML = `<div id="output-drawer" class="hidden"></div>`;
});

describe("InputHandler — onKey suppressed while Monitor mode is active", () => {
  it("normal case: Monitor active suppresses a canvas-mutating shortcut (Ctrl+Z / undo)", () => {
    enterMonitorMode();
    const canvas = makeFakeCanvas();
    const input = new InputHandler(canvas as unknown as Canvas);

    input.onKey(new KeyboardEvent("keydown", { key: "z", ctrlKey: true }));

    expect(canvas.undo).not.toHaveBeenCalled();
    exitMonitorMode();
  });

  it("edge case: Monitor inactive leaves the same shortcut's existing behavior unchanged", () => {
    const canvas = makeFakeCanvas();
    const input = new InputHandler(canvas as unknown as Canvas);

    input.onKey(new KeyboardEvent("keydown", { key: "z", ctrlKey: true }));

    expect(canvas.undo).toHaveBeenCalledTimes(1);
  });
});
