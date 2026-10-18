// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { openMenu, type MenuEntry } from "../menu";

let invoker: HTMLButtonElement;

function items(onSelect: () => void = () => {}): MenuEntry[] {
  return [
    { label: "One", onSelect },
    { label: "Two", onSelect: () => {} },
    "separator",
    { label: "Three", onSelect: () => {} },
  ];
}
const open = (entries: MenuEntry[] = items()) =>
  openMenu({ id: "m", variant: "dropdown", items: entries, invoker, anchor: invoker });
const menuItems = () => Array.from(document.querySelectorAll<HTMLElement>('#m [role="menuitem"]'));
const key = (k: string, target: Element = document.activeElement!) =>
  target.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));

beforeEach(() => {
  vi.useFakeTimers();
  document.body.innerHTML = `<button id="inv">open</button>`;
  invoker = document.getElementById("inv") as HTMLButtonElement;
});
afterEach(() => { vi.useRealTimers(); });

describe("openMenu", () => {
  it("moves focus to the first item and exposes menu roles", () => {
    open();
    expect(document.activeElement).toBe(menuItems()[0]);
    expect(document.getElementById("m")!.getAttribute("role")).toBe("menu");
    expect(invoker.getAttribute("aria-expanded")).toBe("true");
    expect(invoker.getAttribute("aria-haspopup")).toBe("menu");
  });

  it("wraps arrow navigation and supports Home and End", () => {
    open();
    key("ArrowUp");
    expect(document.activeElement).toBe(menuItems()[2]);
    key("ArrowDown");
    expect(document.activeElement).toBe(menuItems()[0]);
    key("End");
    expect(document.activeElement).toBe(menuItems()[2]);
    key("Home");
    expect(document.activeElement).toBe(menuItems()[0]);
  });

  it("closes on Escape and returns focus to the invoker", () => {
    open();
    key("Escape");
    expect(document.getElementById("m")).toBeNull();
    expect(document.activeElement).toBe(invoker);
    expect(invoker.getAttribute("aria-expanded")).toBe("false");
  });

  it("closes on Tab", () => {
    open();
    key("Tab");
    expect(document.getElementById("m")).toBeNull();
  });

  it.each(["Enter", " "])("activates the focused item with %j", k => {
    const onSelect = vi.fn();
    open(items(onSelect));
    key(k);
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(document.getElementById("m")).toBeNull();
  });

  it("activates on click but not on mousedown", () => {
    const onSelect = vi.fn();
    open(items(onSelect));
    menuItems()[0].dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(onSelect).not.toHaveBeenCalled();
    menuItems()[0].click();
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("ignores activation of an aria-disabled item and keeps the menu open", () => {
    const onSelect = vi.fn();
    open([{ label: "Off", disabled: true, onSelect }, { label: "On", onSelect: () => {} }]);
    expect(document.activeElement).toBe(menuItems()[1]);
    menuItems()[0].click();
    expect(onSelect).not.toHaveBeenCalled();
    expect(document.getElementById("m")).not.toBeNull();
  });

  it("dismisses on an outside mousedown and not on an inside one", () => {
    open();
    vi.runAllTimers();
    menuItems()[0].dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(document.getElementById("m")).not.toBeNull();
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(document.getElementById("m")).toBeNull();
  });

  it("keeps a single menu open at a time", () => {
    open();
    openMenu({ id: "other", variant: "context", items: items(), invoker, at: { x: 1, y: 1 } });
    expect(document.getElementById("m")).toBeNull();
    expect(document.getElementById("other")).not.toBeNull();
  });

  it("clamps the menu inside the viewport", () => {
    const w = vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(200);
    const h = vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(100);
    openMenu({ id: "m", variant: "context", items: items(), invoker, at: { x: window.innerWidth + 500, y: window.innerHeight + 500 } });
    const el = document.getElementById("m")!;
    expect(parseInt(el.style.left, 10)).toBe(window.innerWidth - 200 - 8);
    expect(parseInt(el.style.top, 10)).toBe(window.innerHeight - 100 - 8);
    w.mockRestore(); h.mockRestore();
  });

  it("uses Left and Right for a horizontal menu", () => {
    openMenu({ id: "m", variant: "dropdown", orientation: "horizontal", items: items(), invoker, anchor: invoker });
    key("ArrowRight");
    expect(document.activeElement).toBe(menuItems()[1]);
    key("ArrowLeft");
    expect(document.activeElement).toBe(menuItems()[0]);
  });
});
