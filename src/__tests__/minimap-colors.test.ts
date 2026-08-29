/* @vitest-environment jsdom */
import { describe, it, expect } from "vitest";
import { resolveMinimapColors } from "../canvas/Minimap";
import type { CanvasThemeColors } from "../canvas/theme-colors";

// Only the 4 fields resolveMinimapColors actually reads need real values;
// the rest are irrelevant filler so the object satisfies the full interface.
function makeColors(overrides: Partial<CanvasThemeColors>): CanvasThemeColors {
  return {
    surface1: "#000", surface2: "#000", surface3: "#000",
    border: "#000",
    textPrimary: "#000", textSecondary: "#000",
    actionNav: "#000", actionRun: "#000", warning: "#000", error: "#000", ai: "#000",
    catAction: "#000", catAI: "#000", catLogic: "#000", catUtility: "#000", catTrigger: "#000",
    wire: "#000", wireHover: "#000",
    brandGithub: "#000", brandNotion: "#000",
    ...overrides,
  };
}

describe("resolveMinimapColors", () => {
  it("maps each token to the exact literal it replaces, given Midnight's real values", () => {
    const colors = resolveMinimapColors(makeColors({
      border: "#30363d", surface3: "#21262d", actionNav: "#4d9eff", error: "#f87171", wire: "#686f78",
    }));
    expect(colors.colNodeIdle).toBe("#21262d");   // exact match, no alpha
    expect(colors.colSelected).toBe("#4d9eff44"); // accent + original alpha suffix
    expect(colors.colError).toBe("#f8717133");    // error + original alpha suffix
    expect(colors.colViewport).toBe("#4d9eff99"); // accent + viewport-rect alpha
    expect(colors.colConn).toBe("#686f78");       // dedicated wire token, solid (no alpha)
  });

  it("tracks the Paper theme's own token values just as correctly (theme-safety is the whole point)", () => {
    const colors = resolveMinimapColors(makeColors({
      border: "#d9cdb0", surface3: "#e4dac3", actionNav: "#536e60", error: "#9f4839", wire: "#7f7764",
    }));
    expect(colors.colNodeIdle).toBe("#e4dac3");
    expect(colors.colSelected).toBe("#536e6044");
    expect(colors.colConn).toBe("#7f7764");
  });
});
