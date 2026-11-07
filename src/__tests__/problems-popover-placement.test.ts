/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { problemsPopoverPlacement } from "../problems-panel";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

const viewport = { width: 1366, height: 768 };
const chip = { left: 1200, top: 746 };
const WIDTH = 360;

function zIndexOf(css: string, selector: string): number {
  const rule = css.match(new RegExp(`${selector.replace(/[#.]/g, "\\$&")}\\s*\\{[^}]*\\}`));
  const z = rule?.[0].match(/z-index:\s*(\d+)/);
  if (!z) throw new Error(`no z-index on ${selector}`);
  return Number(z[1]);
}

describe("problems popover stacking", () => {
  it("paints above the output drawer and the chat panel but below modals and tooltips", () => {
    const workspace = readFileSync(resolve(__dirname, "../styles/workspace.css"), "utf-8");
    const chat = readFileSync(resolve(__dirname, "../styles/chat.css"), "utf-8");
    const panels = readFileSync(resolve(__dirname, "../styles/panels.css"), "utf-8");
    const popover = zIndexOf(workspace, ".problems-popover");
    expect(popover).toBeGreaterThan(zIndexOf(workspace, "#output-drawer"));
    expect(popover).toBeGreaterThan(zIndexOf(chat, "#chat-panel"));
    expect(popover).toBeLessThan(zIndexOf(panels, "#note-inline-editor"));
  });
});

describe("problemsPopoverPlacement", () => {
  it("anchors above the drawer and caps height to the space above it", () => {
    const at = problemsPopoverPlacement(viewport, chip, { bottom: 52 + 537, right: 0 }, WIDTH);
    expect(at.bottom).toBe(52 + 537 + 6);
    expect(at.maxHeight).toBe(768 - at.bottom - 8);
    expect(at.left).toBe(1366 - 8 - WIDTH);
  });

  it("keeps clear of an open chat panel and of the left edge", () => {
    const withChat = problemsPopoverPlacement(viewport, chip, { bottom: 52 + 120, right: 360 }, WIDTH);
    expect(withChat.left + WIDTH).toBeLessThanOrEqual(1366 - 360 - 8);
    const nearLeft = problemsPopoverPlacement(viewport, { left: 2, top: 746 }, { bottom: 52, right: 0 }, WIDTH);
    expect(nearLeft.left).toBe(8);
  });

  it("falls back to the chip and overlaps the drawer when the drawer leaves no usable room", () => {
    const short = { width: 900, height: 500 };
    const at = problemsPopoverPlacement(short, { left: 700, top: 478 }, { bottom: 52 + 350, right: 0 }, WIDTH);
    expect(at.bottom).toBe(500 - 478 + 6);
    expect(at.maxHeight).toBe(250);
  });
});
