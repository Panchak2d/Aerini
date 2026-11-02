// @vitest-environment jsdom

import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import { initInterpolationAutocomplete } from "../interpolation";
import { setRunOutputs } from "../data-sources";

function typeInPopover(text: string, nodeId = "c"): void {
  const pop = document.createElement("div");
  pop.className = "node-popover";
  pop.dataset.nodeId = nodeId;
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

function shownPaths(): string[] {
  return [...document.querySelectorAll(".interp-path")].map(e => e.textContent ?? "");
}

function activeIndex(): number {
  const items = [...document.querySelectorAll(".interp-item")];
  return items.findIndex(i => i.classList.contains("active"));
}

function makeGraph(): Canvas {
  const node = (id: string, name: string, output_schema?: Record<string, unknown>) =>
    [id, { data: { id, name, output_schema } }] as const;
  const nodes = new Map([
    node("a", "A"),
    node("b", "Code (JS)", { properties: { total: { type: "number" } } }),
    node("c", "C"),
    node("d", "Downstream"),
    node("x", "Unconnected"),
  ]);
  const connectors = new Map([
    ["1", { data: { from_node: "a", to_node: "c" } }],
    ["2", { data: { from_node: "b", to_node: "c" } }],
    ["3", { data: { from_node: "c", to_node: "d" } }],
  ]);
  return { nodes, connectors } as unknown as Canvas;
}

describe("interpolation dropdown keyboard navigation", () => {
  beforeAll(() => {
    initInterpolationAutocomplete(makeGraph());
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

describe("interpolation dropdown contents", () => {
  beforeAll(() => { initInterpolationAutocomplete(makeGraph()); });
  beforeEach(() => { document.body.innerHTML = ""; setRunOutputs(null); });

  it("offers upstream nodes only, never downstream or unconnected ones", () => {
    typeInPopover("{{");
    const text = shownPaths().join("\n");
    expect(text).toContain("A.output");
    expect(text).not.toContain("Downstream");
    expect(text).not.toContain("Unconnected");
    expect(text).not.toContain("C.output");
  });

  it("lists schema fields and real nested keys from the last run, with no invented result/content/value", () => {
    setRunOutputs({ a: { latest: { title: "Hi" } } });
    typeInPopover("{{");
    const paths = shownPaths();
    expect(paths).toContain("A.output.latest.title");
    expect(paths).toContain("Code (JS).output.total");
    expect(paths.some(p => /\.output\.(result|content|value)$/.test(p))).toBe(false);
  });

  it("inserts the full node name when it contains parentheses", () => {
    typeInPopover("{{Code");
    const item = [...document.querySelectorAll<HTMLElement>(".interp-item")]
      .find(i => i.textContent === "Code (JS).output")!;
    item.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    const input = document.querySelector<HTMLInputElement>(".node-popover input")!;
    expect(input.value).toBe("{{Code (JS).output}}");
  });

  it("shows nothing when the popover does not say which node is open", () => {
    const pop = document.createElement("div");
    pop.className = "node-popover";
    const input = document.createElement("input");
    pop.appendChild(input);
    document.body.appendChild(pop);
    input.value = "{{";
    input.setSelectionRange(2, 2);
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(document.querySelectorAll(".interp-item").length).toBe(0);
  });
});
