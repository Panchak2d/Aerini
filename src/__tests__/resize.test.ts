/* @vitest-environment jsdom */
import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { bindDrawerResize } from "../resize";

function mockOffsetHeight(el: HTMLElement, initial: number): void {
  Object.defineProperty(el, "offsetHeight", {
    configurable: true,
    get(this: HTMLElement) {
      const h = parseInt(this.style.height, 10);
      return Number.isFinite(h) ? h : initial;
    },
  });
}

function currentDrawerH(): number {
  return parseInt(
    document.documentElement.style.getPropertyValue("--drawer-h"),
    10
  );
}

describe("bindDrawerResize", () => {
  let handle: HTMLElement;
  let drawer: HTMLElement;

  beforeEach(() => {
    document.body.innerHTML = `
      <div id="output-drawer">
        <div id="drawer-resize-handle"></div>
      </div>
    `;
    handle = document.getElementById("drawer-resize-handle")!;
    drawer = document.getElementById("output-drawer")!;
    mockOffsetHeight(drawer, 260);
    bindDrawerResize();
  });

  afterEach(() => {
    document.documentElement.style.removeProperty("--drawer-h");
    document.body.className = "";
  });

  it("makes the handle a labeled, keyboard-reachable separator", () => {
    expect(handle.tabIndex).toBe(0);
    expect(handle.getAttribute("role")).toBe("separator");
    expect(handle.getAttribute("aria-orientation")).toBe("horizontal");
    expect(handle.getAttribute("aria-label")).toBeTruthy();
  });

  it("drag: toggles the resizing-v body class and dragging class for the drag's duration only", () => {
    handle.dispatchEvent(new MouseEvent("mousedown", { clientY: 500, bubbles: true }));
    expect(document.body.classList.contains("resizing-v")).toBe(true);
    expect(handle.classList.contains("dragging")).toBe(true);

    window.dispatchEvent(new MouseEvent("mousemove", { clientY: 460, bubbles: true }));
    // Dragging the handle up (clientY decreases) grows the drawer.
    expect(drawer.style.height).toBe("300px");

    window.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    expect(document.body.classList.contains("resizing-v")).toBe(false);
    expect(handle.classList.contains("dragging")).toBe(false);
  });

  it("drag: clamps to the 120px floor and the 70vh ceiling", () => {
    Object.defineProperty(window, "innerHeight", { value: 500, configurable: true });

    handle.dispatchEvent(new MouseEvent("mousedown", { clientY: 500, bubbles: true }));
    window.dispatchEvent(new MouseEvent("mousemove", { clientY: 900, bubbles: true })); // drag far down
    expect(drawer.style.height).toBe("120px");

    window.dispatchEvent(new MouseEvent("mousemove", { clientY: -900, bubbles: true })); // drag far up
    expect(drawer.style.height).toBe("350px"); // 500 * 0.7
    window.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
  });

  it("double-click resets to the default height, regardless of current height", () => {
    handle.dispatchEvent(new MouseEvent("mousedown", { clientY: 500, bubbles: true }));
    window.dispatchEvent(new MouseEvent("mousemove", { clientY: 350, bubbles: true }));
    window.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    expect(drawer.style.height).not.toBe("260px");

    handle.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    expect(drawer.style.height).toBe("260px");
    expect(currentDrawerH()).toBe(260);
  });

  it("ArrowUp grows the drawer by one step, ArrowDown shrinks it", () => {
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true, cancelable: true }));
    expect(drawer.style.height).toBe("284px"); // 260 + 24

    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    expect(drawer.style.height).toBe("236px"); // 284 - 24 - 24
  });

  it("ArrowDown floors at 120px and does not go negative", () => {
    for (let i = 0; i < 20; i++) {
      handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    }
    expect(drawer.style.height).toBe("120px");
  });

  it("ignores keys other than ArrowUp/ArrowDown", () => {
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    expect(drawer.style.height).toBe("");
  });
});
