/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import { bindDrawerToggle } from "../toolbar";
import { enterMonitorMode, exitMonitorMode } from "../monitor-mode";

function makeFakeCanvas() {
  return { resize: vi.fn() } as unknown as Canvas;
}

function setDom(startCollapsed: boolean) {
  document.body.innerHTML = `
    <div id="output-drawer" class="${startCollapsed ? "hidden" : ""}">
      <div class="drawer-header" id="drawer-header" role="button" tabindex="0" aria-expanded="${!startCollapsed}">
        <span>Output</span>
        <div class="drawer-tabs" id="drawer-tabs"><button id="tab-summary">Summary</button></div>
        <div class="drawer-actions">
          <button id="btn-close-drawer">Close</button>
        </div>
      </div>
    </div>
  `;
}

describe("bindDrawerToggle", () => {
  it("clicking the header expands a collapsed drawer and flips aria-expanded", () => {
    setDom(true);
    const canvas = makeFakeCanvas();
    bindDrawerToggle(canvas);
    document.getElementById("drawer-header")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.getElementById("output-drawer")!.classList.contains("hidden")).toBe(false);
    expect(document.getElementById("drawer-header")!.getAttribute("aria-expanded")).toBe("true");
    expect(canvas.resize).toHaveBeenCalledTimes(1);
  });

  it("clicking the header again collapses an open drawer (toggle, not one-way)", () => {
    setDom(false);
    bindDrawerToggle(makeFakeCanvas());
    document.getElementById("drawer-header")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.getElementById("output-drawer")!.classList.contains("hidden")).toBe(true);
    expect(document.getElementById("drawer-header")!.getAttribute("aria-expanded")).toBe("false");
  });

  it("#btn-close-drawer always collapses, even if the drawer was already open, without re-toggling via the header's own handler", () => {
    setDom(false);
    bindDrawerToggle(makeFakeCanvas());
    document.getElementById("btn-close-drawer")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.getElementById("output-drawer")!.classList.contains("hidden")).toBe(true);
  });

  it("edge case: clicking a tab or an action button inside the header does not toggle collapse", () => {
    setDom(false);
    bindDrawerToggle(makeFakeCanvas());
    document.getElementById("tab-summary")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.getElementById("output-drawer")!.classList.contains("hidden")).toBe(false);
  });

  it("Enter/Space on the focused header toggles collapse; other keys are ignored", () => {
    setDom(true);
    bindDrawerToggle(makeFakeCanvas());
    const header = document.getElementById("drawer-header")!;
    header.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
    expect(document.getElementById("output-drawer")!.classList.contains("hidden")).toBe(true);
    header.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(document.getElementById("output-drawer")!.classList.contains("hidden")).toBe(false);
  });

  it("does nothing if the drawer markup isn't present (defensive, no throw)", () => {
    document.body.innerHTML = `<div id="toolbar"></div>`;
    expect(() => bindDrawerToggle(makeFakeCanvas())).not.toThrow();
  });

  it("normal case: header click does not expand the drawer while Monitor mode is active", () => {
    setDom(true);
    enterMonitorMode();
    bindDrawerToggle(makeFakeCanvas());
    document.getElementById("drawer-header")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.getElementById("output-drawer")!.classList.contains("hidden")).toBe(true);
    exitMonitorMode();
  });

  it("edge case: header Enter/Space also does not expand the drawer while Monitor mode is active", () => {
    setDom(true);
    enterMonitorMode();
    bindDrawerToggle(makeFakeCanvas());
    document.getElementById("drawer-header")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(document.getElementById("output-drawer")!.classList.contains("hidden")).toBe(true);
    exitMonitorMode();
  });

  it("R-bug3b/3c fix, normal case: the header releases focus after a click-toggle, instead of holding it forever and hijacking every later Space press", () => {
    setDom(true);
    bindDrawerToggle(makeFakeCanvas());
    const header = document.getElementById("drawer-header")!;
    header.focus();
    expect(document.activeElement).toBe(header);
    header.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.activeElement).not.toBe(header);
  });

  it("R-bug3b/3c fix, edge case: a keyboard-triggered toggle (Enter/Space) also releases focus, not just a mouse click", () => {
    setDom(true);
    bindDrawerToggle(makeFakeCanvas());
    const header = document.getElementById("drawer-header")!;
    header.focus();
    header.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(document.activeElement).not.toBe(header);
  });

  it("R-bug3b/3c fix: #btn-close-drawer also releases focus after collapsing, for the same reason", () => {
    setDom(false);
    bindDrawerToggle(makeFakeCanvas());
    const closeBtn = document.getElementById("btn-close-drawer")!;
    closeBtn.focus();
    expect(document.activeElement).toBe(closeBtn);
    closeBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.activeElement).not.toBe(closeBtn);
  });
});
