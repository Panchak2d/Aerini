/* @vitest-environment jsdom */
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { bindDrawerInset } from "../drawer-inset";

const inset = (): string => document.documentElement.style.getPropertyValue("--drawer-inset");

describe("bindDrawerInset", () => {
  let height = 260;
  let notify: () => void = () => {};

  beforeEach(() => {
    document.body.innerHTML = `<div id="output-drawer"></div>`;
    const drawer = document.getElementById("output-drawer")!;
    Object.defineProperty(drawer, "offsetHeight", { configurable: true, get: () => height });
    vi.stubGlobal("ResizeObserver", class {
      constructor(cb: () => void) { notify = cb; }
      observe(): void {}
    });
  });

  afterEach(() => {
    document.documentElement.style.removeProperty("--drawer-inset");
    vi.unstubAllGlobals();
    height = 260;
  });

  it("publishes the rendered height and follows every later resize", () => {
    bindDrawerInset();
    expect(inset()).toBe("260px");
    height = 31;
    notify();
    expect(inset()).toBe("31px");
  });

  it("is a no-op without ResizeObserver or without the drawer", () => {
    vi.stubGlobal("ResizeObserver", undefined);
    expect(() => bindDrawerInset()).not.toThrow();
    expect(inset()).toBe("");
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
    expect(() => bindDrawerInset()).not.toThrow();
  });
});
