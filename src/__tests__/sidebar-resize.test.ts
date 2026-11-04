//@vitest-environment jsdom

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { bindResizeHandle } from "../sidebar-sections";

const LS_WIDTH = "aerini_sidebar_w_v2";

function setWindowWidth(w: number): void {
  Object.defineProperty(window, "innerWidth", { value: w, configurable: true, writable: true });
}

function currentSidebarW(): number {
  return parseInt(
    document.documentElement.style.getPropertyValue("--sidebar-w"),
    10
  );
}

describe("bindResizeHandle (sidebar)", () => {
  let handle: HTMLElement;

  beforeEach(() => {
    setWindowWidth(929); // default sidebar width = 24% of the window, floored at 320px
    localStorage.removeItem(LS_WIDTH);
    document.body.innerHTML = `<div id="sidebar-resize-handle"></div>`;
    handle = document.getElementById("sidebar-resize-handle")!;
    bindResizeHandle();
  });

  afterEach(() => {
    document.documentElement.style.removeProperty("--sidebar-w");
    document.body.className = "";
    localStorage.removeItem(LS_WIDTH);
  });

  it("makes the handle a labeled, keyboard-reachable separator", () => {
    expect(handle.tabIndex).toBe(0);
    expect(handle.getAttribute("role")).toBe("separator");
    expect(handle.getAttribute("aria-orientation")).toBe("vertical");
    expect(handle.getAttribute("aria-label")).toBeTruthy();
  });

  it("drag: toggles the resizing-h body class and dragging class for the drag's duration only", () => {
    handle.dispatchEvent(new MouseEvent("mousedown", { clientX: 500, bubbles: true }));
    expect(document.body.classList.contains("resizing-h")).toBe(true);
    expect(handle.classList.contains("dragging")).toBe(true);

    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 560, bubbles: true }));
    // Dragging the handle right (clientX increases) widens the sidebar —
    // startW (320, from the cleared/default localStorage) + 60.
    expect(currentSidebarW()).toBe(380);

    window.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    expect(document.body.classList.contains("resizing-h")).toBe(false);
    expect(handle.classList.contains("dragging")).toBe(false);
  });

  it("drag: clamps to the 180px floor and the 560px ceiling", () => {
    handle.dispatchEvent(new MouseEvent("mousedown", { clientX: 500, bubbles: true }));
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 0, bubbles: true })); // drag far left
    expect(currentSidebarW()).toBe(180);

    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 2000, bubbles: true })); // drag far right
    expect(currentSidebarW()).toBe(560);
    window.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
  });

  it("double-click resets to the default width (320px), persisted immediately", () => {
    handle.dispatchEvent(new MouseEvent("mousedown", { clientX: 500, bubbles: true }));
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 400, bubbles: true }));
    window.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    expect(currentSidebarW()).not.toBe(320);

    handle.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    expect(currentSidebarW()).toBe(320);
    expect(JSON.parse(localStorage.getItem(LS_WIDTH)!)).toBe(320);
  });

  it("default width is 24% of the window, kept within 320px and 420px", () => {
    const reset = (w: number) => {
      setWindowWidth(w);
      handle.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
      return currentSidebarW();
    };
    expect(reset(1400)).toBe(336);
    expect(reset(1920)).toBe(420);
    expect(reset(700)).toBe(320);
  });

  it("ArrowRight grows the sidebar by one step, ArrowLeft shrinks it — both persisted", () => {
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    expect(currentSidebarW()).toBe(344); // 320 + 24
    expect(JSON.parse(localStorage.getItem(LS_WIDTH)!)).toBe(344);

    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true }));
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true }));
    expect(currentSidebarW()).toBe(296); // 344 - 24 - 24
  });

  it("ArrowLeft floors at 180px and does not go negative", () => {
    for (let i = 0; i < 20; i++) {
      handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true }));
    }
    expect(currentSidebarW()).toBe(180);
  });

  it("ignores keys other than ArrowLeft/ArrowRight", () => {
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    expect(document.documentElement.style.getPropertyValue("--sidebar-w")).toBe("");
  });

  it("drag: a corrupt stored width falls back to the default instead of producing NaN", () => {
    localStorage.setItem(LS_WIDTH, "{}");
    handle.dispatchEvent(new MouseEvent("mousedown", { clientX: 500, bubbles: true }));
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 560, bubbles: true }));
    window.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    expect(currentSidebarW()).toBe(380);
  });
});
