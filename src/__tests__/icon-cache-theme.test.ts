/*
 @vitest-environment jsdom
 */
import { describe, it, expect } from "vitest";
import { accentColors } from "../icon-cache";
import { _resetCanvasColorCache } from "../canvas/theme-colors";

describe("accentColors", () => {
  it("returns the 5 Midnight fallback tokens when no theme CSS is linked", () => {
    document.documentElement.removeAttribute("data-theme");
    document.documentElement.removeAttribute("style");
    _resetCanvasColorCache();
    expect(accentColors().sort()).toEqual(
      ["#4d9eff", "#a78bfa", "#34d399", "#f59e0b", "#f87171"].sort()
    );
  });

  it("tracks live --color-* custom properties instead of a hardcoded array (the regression this batch fixed)", () => {
    document.documentElement.style.setProperty("--cat-action", "rgb(1, 1, 1)");
    document.documentElement.style.setProperty("--color-error", "rgb(2, 2, 2)");

    _resetCanvasColorCache();
    expect(accentColors()).toContain("rgb(1, 1, 1)");
    expect(accentColors()).toContain("rgb(2, 2, 2)");
    expect(accentColors()).not.toContain("#4d9eff");
    expect(accentColors()).not.toContain("#f87171");
  });
});
