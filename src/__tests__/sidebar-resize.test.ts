//@vitest-environment jsdom

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { bindResizeHandle } from "../sidebar-sections";

const LS_WIDTH = "aerini_sidebar_w_v2";

function currentSidebarW(): number {
  return parseInt(
    document.documentElement.style.getPropertyValue("--sidebar-w"),
    10
  );
}

describe("bindResizeHandle (sidebar)", () => {
  let handle: HTMLElement;

  beforeEach(() => {
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
    // startW (260, from the cleared/default localStorage) + 60.
    expect(currentSidebarW()).toBe(320);

    window.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    expect(document.body.classList.contains("resizing-h")).toBe(false);
    expect(handle.classList.contains("dragging")).toBe(false);
  });

  it("drag: clamps to the 180px floor and the 420px ceiling", () => {
    handle.dispatchEvent(new MouseEvent("mousedown", { clientX: 500, bubbles: true }));
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 0, bubbles: true })); // drag far left
    expect(currentSidebarW()).toBe(180);

    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 2000, bubbles: true })); // drag far right
    expect(currentSidebarW()).toBe(420);
    window.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
  });

  it("double-click resets to the default width (260px), persisted immediately", () => {
    handle.dispatchEvent(new MouseEvent("mousedown", { clientX: 500, bubbles: true }));
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 400, bubbles: true }));
    window.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    expect(currentSidebarW()).not.toBe(260);

    handle.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    expect(currentSidebarW()).toBe(260);
    expect(JSON.parse(localStorage.getItem(LS_WIDTH)!)).toBe(260);
  });

  it("ArrowRight grows the sidebar by one step, ArrowLeft shrinks it — both persisted", () => {
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    expect(currentSidebarW()).toBe(284); // 260 + 24
    expect(JSON.parse(localStorage.getItem(LS_WIDTH)!)).toBe(284);

    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true }));
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true }));
    expect(currentSidebarW()).toBe(236); // 284 - 24 - 24
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
});
