/* @vitest-environment jsdom */
import { describe, it, expect, beforeEach, vi } from "vitest";
import { initTooltips } from "../tooltip-manager";

// jsdom's MutationObserver callback runs as a microtask, same as a real
// browser's — flush the microtask queue before asserting on its effects.
function flush(): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, 0));
}

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("initTooltips — dynamic content", () => {
  it("binds a [data-tooltip] element present at call time", async () => {
    const el = document.createElement("button");
    el.setAttribute("data-tooltip", "Existing");
    document.body.appendChild(el);

    initTooltips();
    vi.useFakeTimers();
    el.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(500);
    vi.useRealTimers();

    const tip = document.getElementById("aria-tt");
    expect(tip?.hidden).toBe(false);
    expect(tip?.textContent).toBe("Existing");
  });

  it("binds a [data-tooltip] element added to the DOM after initTooltips() already ran", async () => {
    initTooltips();

    const el = document.createElement("button");
    el.setAttribute("data-tooltip", "Added later");
    document.body.appendChild(el);
    await flush();

    vi.useFakeTimers();
    el.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(500);
    vi.useRealTimers();

    const tip = document.getElementById("aria-tt");
    expect(tip?.hidden).toBe(false);
    expect(tip?.textContent).toBe("Added later");
  });

  it("binds an element whose data-tooltip attribute is set after it was already in the DOM", async () => {
    initTooltips();

    const el = document.createElement("span");
    document.body.appendChild(el);
    await flush();
    el.setAttribute("data-tooltip", "Set after insert");
    await flush();

    vi.useFakeTimers();
    el.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(500);
    vi.useRealTimers();

    const tip = document.getElementById("aria-tt");
    expect(tip?.hidden).toBe(false);
    expect(tip?.textContent).toBe("Set after insert");
  });

  it("does not show the tooltip immediately on hover, only after the show delay elapses", async () => {
    const el = document.createElement("button");
    el.setAttribute("data-tooltip", "Delayed");
    document.body.appendChild(el);

    initTooltips();
    vi.useFakeTimers();
    el.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
    expect(document.getElementById("aria-tt")?.hidden ?? true).toBe(true);

    await vi.advanceTimersByTimeAsync(500);
    expect(document.getElementById("aria-tt")?.hidden).toBe(false);
    vi.useRealTimers();
  });

  it("never shows the tooltip if the pointer leaves before the show delay elapses", async () => {
    const el = document.createElement("button");
    el.setAttribute("data-tooltip", "Quick pass-through");
    document.body.appendChild(el);

    initTooltips();
    vi.useFakeTimers();
    el.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(200);
    el.dispatchEvent(new MouseEvent("mouseleave", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(1000);

    expect(document.getElementById("aria-tt")?.hidden ?? true).toBe(true);
    vi.useRealTimers();
  });

  it("does not re-attach listeners on a second initTooltips() call for the same element", () => {
    const el = document.createElement("button");
    el.setAttribute("data-tooltip", "Once");
    document.body.appendChild(el);

    initTooltips();
    const spy = vi.spyOn(el, "addEventListener");
    initTooltips();
    expect(spy).not.toHaveBeenCalled();
  });

  it("strips a duplicate native title attribute on bind, so only the themed tooltip shows", () => {
    const el = document.createElement("button");
    el.setAttribute("title", "Delete workflow");
    el.setAttribute("data-tooltip", "Delete workflow");
    document.body.appendChild(el);

    initTooltips();

    expect(el.hasAttribute("title")).toBe(false);
  });

  it("hides an orphaned tooltip when its trigger is removed from the DOM while showing", async () => {
    const el = document.createElement("button");
    el.setAttribute("data-tooltip", "Test this node in isolation");
    document.body.appendChild(el);

    initTooltips();
    vi.useFakeTimers();
    el.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(500);
    expect(document.getElementById("aria-tt")?.hidden).toBe(false);
    vi.useRealTimers();

    el.remove();
    await flush();

    expect(document.getElementById("aria-tt")?.hidden).toBe(true);
  });
});

describe("initTooltips — placement and native title", () => {
  const rect = (left: number, top: number, width: number, height: number) =>
    ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) }) as DOMRect;

  async function hover(el: HTMLElement): Promise<HTMLElement> {
    vi.useFakeTimers();
    el.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(500);
    vi.useRealTimers();
    return document.getElementById("aria-tt")!;
  }

  function mockRects(trigger: HTMLElement, triggerRect: DOMRect): void {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      return this === trigger ? triggerRect : rect(0, 0, 300, 40);
    });
  }

  it("places the tooltip beside the trigger when there is room", async () => {
    const el = document.createElement("button");
    el.setAttribute("data-tooltip", "Roomy");
    document.body.appendChild(el);
    mockRects(el, rect(100, 100, 40, 24));
    initTooltips();

    const tip = await hover(el);

    expect(tip.style.left).toBe("148px");
    expect(tip.style.top).toBe("92px");
    vi.restoreAllMocks();
  });

  it("falls back to below the trigger, inside the window, when a trigger sits at the right edge", async () => {
    const el = document.createElement("button");
    el.setAttribute("data-tooltip", "Edge");
    document.body.appendChild(el);
    mockRects(el, rect(window.innerWidth - 60, 10, 50, 24));
    initTooltips();

    const tip = await hover(el);

    expect(tip.style.top).toBe("42px");
    expect(parseFloat(tip.style.left) + 300).toBeLessThanOrEqual(window.innerWidth - 8);
    expect(parseFloat(tip.style.left)).toBeGreaterThanOrEqual(8);
    vi.restoreAllMocks();
  });

  it("drops a native title set after bind, so it cannot stack a second tooltip", async () => {
    const el = document.createElement("button");
    el.setAttribute("data-tooltip", "Refreshed label");
    document.body.appendChild(el);
    initTooltips();

    el.setAttribute("title", "Refreshed label");
    await hover(el);

    expect(el.hasAttribute("title")).toBe(false);
  });
});
