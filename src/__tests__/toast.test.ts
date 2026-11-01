// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { showToast, TOAST_MAX_VISIBLE, TOAST_SHOW_MS } from "../toast";

const texts = () => [...document.querySelectorAll("#toast-stack > .toast")].map((t) => t.textContent);

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.runAllTimers(); vi.useRealTimers(); document.body.innerHTML = ""; });

describe("toasts", () => {
  it("stack in one container instead of sitting on top of each other", () => {
    showToast("one"); showToast("two", "error");
    expect(texts()).toEqual(["one", "two"]);
    expect(document.querySelectorAll("body > .toast").length).toBe(0);
  });

  it("show at most three at a time and bring the rest in as places free up, in order", () => {
    for (const m of ["1", "2", "3", "4", "5"]) showToast(m);
    expect(texts()).toEqual(["1", "2", "3"].slice(0, TOAST_MAX_VISIBLE));
    vi.advanceTimersByTime(TOAST_SHOW_MS + 300);
    expect(texts()).toEqual(["4", "5"]);
    vi.advanceTimersByTime(TOAST_SHOW_MS + 300);
    expect(texts()).toEqual([]);
  });
});
