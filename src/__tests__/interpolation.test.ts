// @vitest-environment jsdom

import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import { initInterpolationAutocomplete } from "../interpolation";

function typeInPopover(text: string): void {
  const pop = document.createElement("div");
  pop.className = "node-popover";
  const input = document.createElement("input");
  pop.appendChild(input);
  document.body.appendChild(pop);
  input.value = text;
  input.setSelectionRange(text.length, text.length);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

function press(key: string): void {
  document.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}

function activeIndex(): number {
  const items = [...document.querySelectorAll(".interp-item")];
  return items.findIndex(i => i.classList.contains("active"));
}

describe("interpolation dropdown keyboard navigation", () => {
  beforeAll(() => {
    const nodes = new Map([
      ["a", { data: { name: "A" } }],
      ["b", { data: { name: "B" } }],
    ]);
    initInterpolationAutocomplete({ nodes } as unknown as Canvas);
  });

  beforeEach(() => {
    document.body.innerHTML = "";
    typeInPopover("{{");
  });

  it("moves the highlight down one item at a time", () => {
    press("ArrowDown");
    expect(activeIndex()).toBe(0);
    press("ArrowDown");
    expect(activeIndex()).toBe(1);
  });

  it("keeps the last item highlighted when ArrowDown is pressed at the end", () => {
    const last = document.querySelectorAll(".interp-item").length - 1;
    for (let i = 0; i <= last + 2; i++) press("ArrowDown");
    expect(activeIndex()).toBe(last);
  });

  it("keeps the first item highlighted when ArrowUp is pressed at the top", () => {
    press("ArrowDown");
    press("ArrowUp");
    expect(activeIndex()).toBe(0);
  });
});
