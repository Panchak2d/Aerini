// @vitest-environment jsdom

import { describe, it, expect } from "vitest";
import {
  resolveCanvasColors, getCanvasColors, _resetCanvasColorCache,
  headerTint, tintOver, resolveNoteColors,
} from "../canvas/theme-colors";

describe("resolveCanvasColors", () => {
  it("reads every token from the given getVar, trimmed", () => {
    const vars: Record<string, string> = { "--color-surface-1": "  #f0e8d6  ", "--brand-github": "  #181717  " };
    const colors = resolveCanvasColors((name) => vars[name] ?? "");
    expect(colors.surface1).toBe("#f0e8d6");
    expect(colors.brandGithub).toBe("#181717");
  });

  it("falls back to the exact pre-batch Midnight literals when a lookup returns empty", () => {
    const colors = resolveCanvasColors(() => "");
    expect(colors.surface1).toBe("#161b22");
    expect(colors.border).toBe("#30363d");
    expect(colors.actionNav).toBe("#4d9eff");
    expect(colors.actionRun).toBe("#34d399");
    expect(colors.error).toBe("#f87171");
    expect(colors.catTrigger).toBe("#8aa9c9");
    // Midnight's dark surfaces make GitHub/Notion's real near-black mark
    // illegible, so the canvas-bitmap fallback (no theme CSS reachable) is
    // white, not the literal brand hex — matches --brand-github/-notion's
    // own :root default in variables.css.
    expect(colors.brandGithub).toBe("#ffffff");
    expect(colors.brandNotion).toBe("#ffffff");
  });
});

describe("getCanvasColors — live cache", () => {
  it("re-reads after <html data-theme> changes; does not invalidate on an unrelated property change", async () => {
    document.documentElement.removeAttribute("data-theme");
    document.documentElement.style.setProperty("--color-surface-1", "rgb(1, 2, 3)");
    _resetCanvasColorCache();

    expect(getCanvasColors().surface1).toBe("rgb(1, 2, 3)");

    document.documentElement.style.setProperty("--color-surface-1", "rgb(9, 9, 9)");
    expect(getCanvasColors().surface1).toBe("rgb(1, 2, 3)"); // still cached

    document.documentElement.setAttribute("data-theme", "paper");
    await new Promise(resolve => setTimeout(resolve, 0)); // MutationObserver microtask

    expect(getCanvasColors().surface1).toBe("rgb(9, 9, 9)");
  });
});

describe("headerTint", () => {
  it("appends a fixed low-alpha suffix to whatever accent it's given", () => {
    expect(headerTint("#4d9eff")).toBe("#4d9eff26");
    expect(headerTint("#536e60")).toBe("#536e6026");
  });
});

describe("tintOver", () => {
  it("returns fg unchanged at full strength", () => {
    expect(tintOver("#4d9eff", "#161b22", 1)).toBe("#4d9eff");
  });

  it("returns bg unchanged at zero strength", () => {
    expect(tintOver("#4d9eff", "#161b22", 0)).toBe("#161b22");
  });

  it("blends proportionally at a fractional strength", () => {
    // Pure channel blend at t=0.5 between white and black is exactly mid-gray.
    expect(tintOver("#ffffff", "#000000", 0.5)).toBe("#808080");
  });
});

describe("resolveNoteColors", () => {
  const colors = resolveCanvasColors(() => ""); // Midnight fallback

  it("covers exactly the 5 names NoteEditor.ts writes to config.color", () => {
    expect(Object.keys(resolveNoteColors(colors)).sort()).toEqual(
      ["blue", "default", "green", "red", "yellow"]
    );
  });

  it("'default' uses the neutral surface/border/text tokens, not a status color", () => {
    const c = resolveNoteColors(colors).default;
    expect(c).toEqual({ bg: colors.surface2, border: colors.border, text: colors.textSecondary });
  });

  it("each hue variant's text/border reuse the matching status token", () => {
    const n = resolveNoteColors(colors);
    expect(n.yellow.text).toBe(colors.warning);
    expect(n.blue.text).toBe(colors.actionNav);
    expect(n.green.text).toBe(colors.actionRun);
    expect(n.red.text).toBe(colors.error);
    expect(n.yellow.border).toBe(colors.warning + "44");
  });

  it("each hue variant's bg is an opaque hex (never translucent — must match every other node body)", () => {
    const n = resolveNoteColors(colors);
    for (const key of ["yellow", "blue", "green", "red"] as const) {
      expect(n[key].bg).toMatch(/^#[0-9a-f]{6}$/);
    }
  });
});
