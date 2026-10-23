/* @vitest-environment jsdom */
import { describe, it, expect } from "vitest";
import { resolveMinimapColors, resolveNodeStyle, type MinimapNodeInput } from "../canvas/Minimap";
import { typeMeta } from "../canvas/Node";
import { getCanvasColors, resolveNoteColors, type CanvasThemeColors } from "../canvas/theme-colors";

const MIDNIGHT: CanvasThemeColors = {
  surface1: "#161b22", surface2: "#1c2128", surface3: "#21262d",
  border: "#30363d",
  textPrimary: "#e6edf3", textSecondary: "#8b949e",
  actionNav: "#4d9eff", actionRun: "#34d399", warning: "#f59e0b", error: "#f87171", ai: "#a78bfa",
  catAction: "#4d9eff", catAI: "#a78bfa", catLogic: "#34d399", catUtility: "#f59e0b", catTrigger: "#8aa9c9",
  wire: "#686f78", wireHover: "#858d97",
  brandGithub: "#ffffff", brandNotion: "#ffffff",
};

const PAPER: CanvasThemeColors = {
  surface1: "#f0e8d6", surface2: "#eae1cc", surface3: "#e4dac3",
  border: "#d9cdb0",
  textPrimary: "#2b2820", textSecondary: "#625b49",
  actionNav: "#4c6558", actionRun: "#6d5e1d", warning: "#7c5a27", error: "#363959", ai: "#7a508c",
  catAction: "#4c6558", catAI: "#7a508c", catLogic: "#6d5e1d", catUtility: "#7c5a27", catTrigger: "#436382",
  wire: "#7f7764", wireHover: "#6e6651",
  brandGithub: "#181717", brandNotion: "#181717",
};

const BASE: MinimapNodeInput = {
  isNote: false, noteColor: undefined, status: "idle",
  selected: false, disabled: false, isTrigger: false, categoryAccent: "#aaaaaa", dimmed: false,
};

describe("resolveMinimapColors", () => {
  it("takes wire, nav, run and panel straight from the active theme's tokens", () => {
    expect(resolveMinimapColors(MIDNIGHT)).toEqual({ conn: "#686f78", nav: "#4d9eff", run: "#34d399", panel: "#161b22" });
    expect(resolveMinimapColors(PAPER)).toEqual({ conn: "#7f7764", nav: "#4c6558", run: "#6d5e1d", panel: "#f0e8d6" });
  });
});

describe("resolveNodeStyle", () => {
  it("fills with run status first, then trigger color, then the node's category color", () => {
    const fill = (o: Partial<MinimapNodeInput>) => resolveNodeStyle({ ...BASE, ...o }, MIDNIGHT, resolveNoteColors(MIDNIGHT)).fill;
    expect(fill({})).toBe("#aaaaaa");
    expect(fill({ isTrigger: true })).toBe(MIDNIGHT.catTrigger);
    expect(fill({ isTrigger: true, status: "running" })).toBe(MIDNIGHT.warning);
    expect(fill({ status: "success" })).toBe(MIDNIGHT.actionRun);
    expect(fill({ status: "error" })).toBe(MIDNIGHT.error);
  });

  it("marks run states with a ring and ranks them above idle so errors are drawn last and largest", () => {
    const style = (status: MinimapNodeInput["status"]) =>
      resolveNodeStyle({ ...BASE, status }, PAPER, resolveNoteColors(PAPER));
    const idle = style("idle"), success = style("success"), running = style("running"), error = style("error");
    expect(idle.stroke).toBeNull();
    for (const s of [success, running, error]) expect(s.stroke).toBe(PAPER.textPrimary);
    expect([idle.rank, success.rank, running.rank, error.rank]).toEqual([1, 2, 3, 4]);
    expect(error.strokeWidth).toBeGreaterThan(running.strokeWidth);
    expect(error.minW).toBeGreaterThan(idle.minW);
    expect(error.minH).toBeGreaterThan(idle.minH);
  });

  it("dims a disabled node and outlines it in its own color at full strength", () => {
    const s = resolveNodeStyle({ ...BASE, disabled: true, isTrigger: true }, MIDNIGHT, resolveNoteColors(MIDNIGHT));
    expect(s.fillAlpha).toBe(0.35);
    expect(s.fill).toBe(MIDNIGHT.catTrigger);
    expect(s.stroke).toBe(MIDNIGHT.catTrigger);
  });

  it("carries the trigger flag and category glyph color, and fades only non-error nodes outside the selection", () => {
    const style = (o: Partial<MinimapNodeInput>) => resolveNodeStyle({ ...BASE, ...o }, MIDNIGHT, resolveNoteColors(MIDNIGHT));
    expect(style({ isTrigger: true })).toMatchObject({ trigger: true, glyphAccent: "#aaaaaa" });
    expect(style({})).toMatchObject({ trigger: false, fillAlpha: 1 });
    expect(style({ dimmed: true }).fillAlpha).toBe(0.5);
    expect(style({ dimmed: true, disabled: true }).fillAlpha).toBe(0.35);
    expect(style({ dimmed: true, isNote: true }).fillAlpha).toBe(1);
  });

  it("draws notes first as an outlined rectangle in their own palette, falling back to default", () => {
    const notes = resolveNoteColors(MIDNIGHT);
    const yellow = resolveNodeStyle({ ...BASE, isNote: true, noteColor: "yellow", selected: true }, MIDNIGHT, notes);
    expect(yellow).toMatchObject({ rank: 0, fill: notes["yellow"].bg, stroke: notes["yellow"].text, selected: true });
    const unknown = resolveNodeStyle({ ...BASE, isNote: true, noteColor: "no-such-color" }, MIDNIGHT, notes);
    expect(unknown.fill).toBe(notes["default"].bg);
  });
});

describe("typeMeta", () => {
  it("falls back to the utility category color for an unknown node type", () => {
    const colors = getCanvasColors();
    expect(typeMeta("not-a-real-type").accent).toBe(colors.catUtility);
    expect(typeMeta("action").accent).toBe(colors.catAction);
  });
});
