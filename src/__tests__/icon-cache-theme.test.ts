/*
 @vitest-environment jsdom
 */
import { describe, it, expect } from "vitest";
import { accentColors, noteInkColors } from "../icon-cache";
import { _resetCanvasColorCache, getCanvasColors, resolveNoteColors } from "../canvas/theme-colors";

describe("accentColors", () => {
  it("returns the 5 Midnight fallback tokens when no theme CSS is linked", () => {
    document.documentElement.removeAttribute("data-theme");
    document.documentElement.removeAttribute("style");
    _resetCanvasColorCache();
    expect(accentColors().sort()).toEqual(
      ["#4d9eff", "#a78bfa", "#34d399", "#f59e0b", "#f87171"].sort()
    );
  });

  it("tracks live --color-* custom properties instead of a hardcoded array", () => {
    document.documentElement.style.setProperty("--cat-action", "rgb(1, 1, 1)");
    document.documentElement.style.setProperty("--color-error", "rgb(2, 2, 2)");

    _resetCanvasColorCache();
    expect(accentColors()).toContain("rgb(1, 1, 1)");
    expect(accentColors()).toContain("rgb(2, 2, 2)");
    expect(accentColors()).not.toContain("#4d9eff");
    expect(accentColors()).not.toContain("#f87171");
  });

  it("noteInkColors covers every note color, including ones outside accentColors", () => {
    document.documentElement.removeAttribute("style");
    _resetCanvasColorCache();
    const ink = noteInkColors();
    for (const n of Object.values(resolveNoteColors(getCanvasColors()))) expect(ink).toContain(n.text);
    expect(ink).toContain("#8b949e");
    expect(accentColors()).not.toContain("#8b949e");
  });
});
