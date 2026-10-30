// @vitest-environment jsdom

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { toggleMinimap, restoreMinimapVisibility, MINIMAP_HIDDEN_KEY } from "../minimap-visibility";

function mount(): { wrap: HTMLElement; btn: HTMLElement } {
  document.body.innerHTML = '<div id="minimap-wrap"><button id="btn-minimap-toggle" aria-pressed="true"></button></div>';
  return { wrap: document.getElementById("minimap-wrap")!, btn: document.getElementById("btn-minimap-toggle")! };
}

describe("minimap visibility", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.restoreAllMocks());

  it("toggles the minimap, tells assistive tech which state the button is in, and remembers it", () => {
    const { wrap, btn } = mount();
    toggleMinimap();
    expect(wrap.classList.contains("minimap-hidden")).toBe(true);
    expect(btn.getAttribute("aria-pressed")).toBe("false");
    expect(localStorage.getItem(MINIMAP_HIDDEN_KEY)).toBe("true");
    toggleMinimap();
    expect(wrap.classList.contains("minimap-hidden")).toBe(false);
    expect(btn.getAttribute("aria-pressed")).toBe("true");
    expect(localStorage.getItem(MINIMAP_HIDDEN_KEY)).toBe("false");
  });

  it("restores a hidden minimap on start, and shows it when nothing was stored", () => {
    const { wrap, btn } = mount();
    localStorage.setItem(MINIMAP_HIDDEN_KEY, "true");
    restoreMinimapVisibility();
    expect(wrap.classList.contains("minimap-hidden")).toBe(true);
    expect(btn.getAttribute("aria-pressed")).toBe("false");
    localStorage.clear();
    restoreMinimapVisibility();
    expect(wrap.classList.contains("minimap-hidden")).toBe(false);
    expect(btn.getAttribute("aria-pressed")).toBe("true");
  });

  it("still toggles and restores when storage throws", () => {
    const { wrap } = mount();
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("denied"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("denied"); });
    expect(() => restoreMinimapVisibility()).not.toThrow();
    expect(() => toggleMinimap()).not.toThrow();
    expect(wrap.classList.contains("minimap-hidden")).toBe(true);
  });
});
