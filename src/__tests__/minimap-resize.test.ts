/* @vitest-environment jsdom */
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import {
  bindMinimapResize, clampWidth, dragThresholdExceeded, dragWidthDelta, keyStepWidth,
  maxWidthForArea, minimapHeightFor, parseStoredWidth, MINIMAP_W_KEY,
} from "../minimap-resize";

describe("maxWidthForArea", () => {
  it("takes the smallest of the absolute cap, 40% of the width and the height-derived width", () => {
    expect(maxWidthForArea({ width: 5000, height: 5000 })).toBe(480);
    expect(maxWidthForArea({ width: 800, height: 5000 })).toBe(320);
    expect(maxWidthForArea({ width: 5000, height: 300 })).toBe(250); // 0.5 * 300 * 5/3
  });

  it("lets the minimum win over a smaller area and survives non-finite input", () => {
    expect(maxWidthForArea({ width: 100, height: 100 })).toBe(120);
    expect(maxWidthForArea({ width: NaN, height: 800 })).toBe(120);
  });
});

describe("clampWidth and minimapHeightFor", () => {
  it("rounds, clamps to [min, max], and lets the minimum win when max is below it", () => {
    expect(clampWidth(10, 400)).toBe(120);
    expect(clampWidth(999, 400)).toBe(400);
    expect(clampWidth(200.6, 400)).toBe(201);
    expect(clampWidth(300, 100)).toBe(120);
    expect(clampWidth(NaN, 400)).toBe(150);
  });

  it("derives a 5:3 height from the width", () => {
    expect(minimapHeightFor(150)).toBe(90);
    expect(minimapHeightFor(120)).toBe(72);
    expect(minimapHeightFor(163)).toBe(98); // 97.8 rounds up
  });
});

describe("parseStoredWidth", () => {
  it("falls back to the default for missing or non-numeric values and clamps out-of-range ones", () => {
    expect(parseStoredWidth(null)).toBe(150);
    expect(parseStoredWidth("")).toBe(150);
    expect(parseStoredWidth("wide")).toBe(150);
    expect(parseStoredWidth("Infinity")).toBe(150);
    expect(parseStoredWidth("5")).toBe(120);
    expect(parseStoredWidth("99999")).toBe(480);
    expect(parseStoredWidth("300")).toBe(300);
  });
});

describe("keyStepWidth", () => {
  it("steps by 10 for arrows and 40 for Page keys, clamped to the range", () => {
    expect(keyStepWidth("ArrowRight", 150, 400)).toBe(160);
    expect(keyStepWidth("ArrowUp", 150, 400)).toBe(160);
    expect(keyStepWidth("ArrowLeft", 150, 400)).toBe(140);
    expect(keyStepWidth("ArrowDown", 150, 400)).toBe(140);
    expect(keyStepWidth("PageUp", 150, 400)).toBe(190);
    expect(keyStepWidth("PageDown", 150, 400)).toBe(120); // 110 floors at the minimum
    expect(keyStepWidth("PageDown", 300, 400)).toBe(260);
    expect(keyStepWidth("ArrowRight", 395, 400)).toBe(400);
  });

  it("jumps to the minimum, the current maximum or the default, and ignores other keys", () => {
    expect(keyStepWidth("Home", 300, 400)).toBe(120);
    expect(keyStepWidth("End", 150, 400)).toBe(400);
    expect(keyStepWidth("Enter", 300, 400)).toBe(150);
    expect(keyStepWidth("a", 150, 400)).toBeNull();
    expect(keyStepWidth("Tab", 150, 400)).toBeNull();
  });
});

describe("drag math", () => {
  const start = { x: 500, y: 500 };

  it("grows when the pointer moves up or left and shrinks when it moves down or right", () => {
    expect(dragWidthDelta(start, { x: 400, y: 500 })).toBe(100);
    expect(dragWidthDelta(start, { x: 600, y: 500 })).toBe(-100);
    expect(dragWidthDelta(start, { x: 500, y: 440 })).toBeCloseTo(100, 6); // 60 * 5/3
    expect(dragWidthDelta(start, { x: 500, y: 560 })).toBeCloseTo(-100, 6);
  });

  it("uses whichever axis moved further after the aspect scaling", () => {
    expect(dragWidthDelta(start, { x: 480, y: 450 })).toBeCloseTo(50 * 5 / 3, 6); // vertical wins
    expect(dragWidthDelta(start, { x: 400, y: 490 })).toBe(100);                 // horizontal wins
  });

  it("starts resizing only once the pointer has moved 3px", () => {
    expect(dragThresholdExceeded(start, { x: 502, y: 501 })).toBe(false);
    expect(dragThresholdExceeded(start, { x: 503, y: 500 })).toBe(true);
  });
});

function rect(w: number, h: number): DOMRect {
  return { left: 0, top: 0, right: w, bottom: h, x: 0, y: 0, width: w, height: h, toJSON: () => ({}) } as DOMRect;
}

function ptr(type: string, o: { x?: number; y?: number; id?: number; primary?: boolean; button?: number } = {}): PointerEvent {
  return new PointerEvent(type, {
    bubbles: true, cancelable: true,
    clientX: o.x ?? 0, clientY: o.y ?? 0,
    pointerId: o.id ?? 1, isPrimary: o.primary ?? true, button: o.button ?? 0,
  });
}

function key(handle: HTMLElement, k: string, init: KeyboardEventInit = {}): KeyboardEvent {
  const e = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init });
  handle.dispatchEvent(e);
  return e;
}

const cssVar = (n: string): string => document.documentElement.style.getPropertyValue(n);

describe("bindMinimapResize", () => {
  let handle: HTMLElement;
  let area: HTMLElement;
  let areaSize = { w: 1000, h: 800 }; // max width = min(480, 400, 666) = 400

  const bind = (): void => {
    document.body.innerHTML = `
      <main id="canvas-area"><div id="minimap-wrap">
        <canvas id="minimap-canvas"></canvas><div id="minimap-resize-handle"></div>
      </div></main>`;
    area = document.getElementById("canvas-area")!;
    handle = document.getElementById("minimap-resize-handle")!;
    area.getBoundingClientRect = () => rect(areaSize.w, areaSize.h);
    handle.setPointerCapture = vi.fn();
    bindMinimapResize();
  };

  beforeEach(() => {
    areaSize = { w: 1000, h: 800 };
    localStorage.clear();
    bind();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    document.documentElement.removeAttribute("style");
    document.body.className = "";
    localStorage.clear();
  });

  it("exposes a labeled slider with live value attributes and no orientation", () => {
    expect(handle.tabIndex).toBe(0);
    expect(handle.getAttribute("role")).toBe("slider");
    expect(handle.getAttribute("aria-label")).toBe("Resize minimap");
    expect(handle.hasAttribute("aria-orientation")).toBe(false);
    expect(handle.getAttribute("aria-valuemin")).toBe("120");
    expect(handle.getAttribute("aria-valuemax")).toBe("400");
    expect(handle.getAttribute("aria-valuenow")).toBe("150");
    expect(handle.getAttribute("aria-valuetext")).toBe("150 by 90 pixels");
    expect(cssVar("--minimap-w")).toBe("150px");
    expect(cssVar("--minimap-h")).toBe("90px");
  });

  it("resizes with the keys, keeps the 5:3 ratio, updates aria and persists each step", () => {
    expect(key(handle, "ArrowRight").defaultPrevented).toBe(true);
    expect(cssVar("--minimap-w")).toBe("160px");
    expect(cssVar("--minimap-h")).toBe("96px");
    expect(handle.getAttribute("aria-valuenow")).toBe("160");
    expect(handle.getAttribute("aria-valuetext")).toBe("160 by 96 pixels");
    expect(localStorage.getItem(MINIMAP_W_KEY)).toBe("160");

    key(handle, "Home");
    expect(cssVar("--minimap-w")).toBe("120px");
    key(handle, "End");
    expect(cssVar("--minimap-w")).toBe("400px");
    expect(localStorage.getItem(MINIMAP_W_KEY)).toBe("400");
    key(handle, "Enter");
    expect(cssVar("--minimap-w")).toBe("150px");
  });

  it("leaves other keys and modified arrows alone", () => {
    expect(key(handle, "a").defaultPrevented).toBe(false);
    expect(key(handle, "ArrowRight", { altKey: true }).defaultPrevented).toBe(false);
    expect(cssVar("--minimap-w")).toBe("150px");
    expect(localStorage.getItem(MINIMAP_W_KEY)).toBeNull();
  });

  it("drag: ignores movement under the threshold, then resizes, locks the cursor and persists only on release", () => {
    handle.dispatchEvent(ptr("pointerdown", { x: 500, y: 500 }));
    handle.dispatchEvent(ptr("pointermove", { x: 499, y: 499 }));
    expect(document.body.classList.contains("resizing-minimap")).toBe(false);
    expect(cssVar("--minimap-w")).toBe("150px");

    handle.dispatchEvent(ptr("pointermove", { x: 400, y: 500 }));
    expect(document.body.classList.contains("resizing-minimap")).toBe(true);
    expect(handle.classList.contains("dragging")).toBe(true);
    expect(cssVar("--minimap-w")).toBe("250px");
    expect(localStorage.getItem(MINIMAP_W_KEY)).toBeNull();

    handle.dispatchEvent(ptr("pointerup", { x: 400, y: 500 }));
    expect(document.body.classList.contains("resizing-minimap")).toBe(false);
    expect(handle.classList.contains("dragging")).toBe(false);
    expect(localStorage.getItem(MINIMAP_W_KEY)).toBe("250");
  });

  it("drag: follows the vertical axis when it dominates and clamps to the maximum", () => {
    handle.dispatchEvent(ptr("pointerdown", { x: 500, y: 500 }));
    handle.dispatchEvent(ptr("pointermove", { x: 500, y: 440 }));
    expect(cssVar("--minimap-w")).toBe("250px");
    handle.dispatchEvent(ptr("pointermove", { x: -5000, y: 500 }));
    expect(cssVar("--minimap-w")).toBe("400px");
    handle.dispatchEvent(ptr("pointermove", { x: 5000, y: 500 }));
    expect(cssVar("--minimap-w")).toBe("120px");
    handle.dispatchEvent(ptr("pointerup", { x: 5000, y: 500 }));
  });

  it("drag: a cancelled gesture restores the previous width, persists nothing and clears every drag marker", () => {
    handle.dispatchEvent(ptr("pointerdown", { x: 500, y: 500 }));
    handle.dispatchEvent(ptr("pointermove", { x: 400, y: 500 }));
    expect(cssVar("--minimap-w")).toBe("250px");

    handle.dispatchEvent(ptr("pointercancel", { x: 400, y: 500 }));
    expect(document.body.classList.contains("resizing-minimap")).toBe(false);
    expect(handle.classList.contains("dragging")).toBe(false);
    expect(cssVar("--minimap-w")).toBe("150px");
    expect(localStorage.getItem(MINIMAP_W_KEY)).toBeNull();
  });

  it("drag: lost pointer capture ends the drag the same way", () => {
    handle.dispatchEvent(ptr("pointerdown", { x: 500, y: 500 }));
    handle.dispatchEvent(ptr("pointermove", { x: 400, y: 500 }));
    handle.dispatchEvent(ptr("lostpointercapture"));
    expect(document.body.classList.contains("resizing-minimap")).toBe(false);
    expect(cssVar("--minimap-w")).toBe("150px");
  });

  it("drag: ignores non-primary buttons, a second pointer, and a click without movement", () => {
    handle.dispatchEvent(ptr("pointerdown", { x: 500, y: 500, button: 2 }));
    handle.dispatchEvent(ptr("pointermove", { x: 400, y: 500 }));
    expect(cssVar("--minimap-w")).toBe("150px");

    handle.dispatchEvent(ptr("pointerdown", { x: 500, y: 500, id: 1 }));
    handle.dispatchEvent(ptr("pointerdown", { x: 100, y: 100, id: 2, primary: false }));
    handle.dispatchEvent(ptr("pointermove", { x: 100, y: 100, id: 2, primary: false }));
    expect(cssVar("--minimap-w")).toBe("150px");
    handle.dispatchEvent(ptr("pointerup", { x: 500, y: 500, id: 1 }));
    expect(localStorage.getItem(MINIMAP_W_KEY)).toBeNull();
  });

  it("drag: pointerdown does not bubble, so it cannot start another drag on a parent", () => {
    const parentSpy = vi.fn();
    document.getElementById("minimap-wrap")!.addEventListener("pointerdown", parentSpy);
    handle.dispatchEvent(ptr("pointerdown", { x: 500, y: 500 }));
    expect(parentSpy).not.toHaveBeenCalled();
    handle.dispatchEvent(ptr("pointerup", { x: 500, y: 500 }));
  });

  it("double-click resets to the default width and persists it", () => {
    key(handle, "End");
    handle.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    expect(cssVar("--minimap-w")).toBe("150px");
    expect(localStorage.getItem(MINIMAP_W_KEY)).toBe("150");
  });

  it("restores a saved width on bind", () => {
    localStorage.setItem(MINIMAP_W_KEY, "300");
    bind();
    expect(cssVar("--minimap-w")).toBe("300px");
    expect(cssVar("--minimap-h")).toBe("180px");
  });

  it("falls back to the default for a corrupt stored value and clamps an out-of-range one", () => {
    localStorage.setItem(MINIMAP_W_KEY, "{not a number");
    bind();
    expect(cssVar("--minimap-w")).toBe("150px");

    localStorage.setItem(MINIMAP_W_KEY, "99999");
    bind();
    expect(cssVar("--minimap-w")).toBe("400px");
  });

  it("re-clamps to the available space on window resize without overwriting the saved request", () => {
    localStorage.setItem(MINIMAP_W_KEY, "300");
    bind();
    areaSize = { w: 500, h: 400 }; // max = min(480, 200, 333) = 200
    window.dispatchEvent(new Event("resize"));
    expect(cssVar("--minimap-w")).toBe("200px");
    expect(handle.getAttribute("aria-valuemax")).toBe("200");
    expect(localStorage.getItem(MINIMAP_W_KEY)).toBe("300");

    areaSize = { w: 1000, h: 800 };
    window.dispatchEvent(new Event("resize"));
    expect(cssVar("--minimap-w")).toBe("300px");
  });

  it("re-clamps when the chat panel opens and recovers when it closes", async () => {
    const innerWidth = window.innerWidth;
    Object.defineProperty(window, "innerWidth", { value: 1000, configurable: true });
    localStorage.setItem(MINIMAP_W_KEY, "300");
    bind();
    expect(cssVar("--minimap-w")).toBe("300px");

    const chat = document.createElement("div");
    chat.id = "chat-panel";
    Object.defineProperty(chat, "offsetWidth", { value: 700, configurable: true });
    document.body.appendChild(chat);
    document.body.classList.add("chat-panel-open");
    await new Promise((r) => setTimeout(r, 0));
    expect(cssVar("--minimap-w")).toBe("120px"); // 300px visible -> 40% = 120
    expect(localStorage.getItem(MINIMAP_W_KEY)).toBe("300");

    chat.remove();
    document.body.classList.remove("chat-panel-open");
    await new Promise((r) => setTimeout(r, 0));
    expect(cssVar("--minimap-w")).toBe("300px");
    Object.defineProperty(window, "innerWidth", { value: innerWidth, configurable: true });
  });

  it("keeps working when localStorage reads and writes throw", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("denied"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("quota"); });
    bind();
    expect(cssVar("--minimap-w")).toBe("150px");
    expect(() => key(handle, "ArrowRight")).not.toThrow();
    expect(cssVar("--minimap-w")).toBe("160px");
  });

  it("binding twice does not double-handle events and clears a stale drag marker", () => {
    document.body.classList.add("resizing-minimap");
    handle.classList.add("dragging");
    bindMinimapResize();
    expect(document.body.classList.contains("resizing-minimap")).toBe(false);
    expect(handle.classList.contains("dragging")).toBe(false);

    key(handle, "ArrowRight");
    expect(cssVar("--minimap-w")).toBe("160px");
  });
});
