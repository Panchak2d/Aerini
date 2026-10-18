// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createTagEditor, openTagPopover, closeTagPopover } from "../tag-editor";
import { buildTagIndex, MAX_TAGS_PER_WORKFLOW } from "../workflow-tags";

const library = buildTagIndex([
  { name: "a", tags: ["Prod", "ai"] }, { name: "b", tags: ["Prod"] }, { name: "c", tags: ["chat"] },
]);

let root: HTMLElement;
const input = () => root.querySelector("input")!;
const chips = () => Array.from(root.querySelectorAll(".wf-tag-chip > span:first-child")).map(c => c.textContent);
const options = () => Array.from(root.querySelectorAll('[role="option"]')).map(o => o.firstChild!.textContent);
const type = (v: string) => { input().value = v; input().dispatchEvent(new Event("input")); };
const press = (k: string) => input().dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));

beforeEach(() => {
  document.body.innerHTML = `<div id="root"></div><button id="inv"></button>`;
  root = document.getElementById("root")!;
});
afterEach(() => closeTagPopover());

describe("createTagEditor", () => {
  const mk = (initial: Array<{ label: string; have: number }> = [], total = 1) =>
    createTagEditor({ root, initial, total, getLibrary: () => library });

  it("adds on Enter, adopting the library casing for a known tag", () => {
    const ed = mk();
    type("prod");
    press("Enter");
    expect(chips()).toEqual(["Prod"]);
    expect(ed.getEdit()).toEqual({ add: ["Prod"], remove: [] });
  });

  it("splits pasted comma-separated text into separate tags", () => {
    mk();
    type("x, y , z");
    press("Enter");
    expect(chips()).toEqual(["x", "y", "z"]);
  });

  it("removes the last chip with Backspace on an empty input", () => {
    const ed = mk([{ label: "a", have: 1 }, { label: "b", have: 1 }]);
    press("Backspace");
    expect(chips()).toEqual(["a"]);
    expect(ed.getEdit().remove).toEqual(["b"]);
  });

  it("offers library tags not already present and picks the active one with Enter", () => {
    mk([{ label: "Prod", have: 1 }]);
    type("");
    expect(options()).toEqual(["ai", "chat"]);
    expect(input().getAttribute("aria-expanded")).toBe("true");
    press("ArrowDown");
    expect(input().getAttribute("aria-activedescendant")).toBe(root.querySelector('[role="option"]')!.id);
    press("Enter");
    expect(chips()).toEqual(["Prod", "ai"]);
  });

  it("shows a message at the tag limit instead of dropping silently", () => {
    mk(Array.from({ length: MAX_TAGS_PER_WORKFLOW }, (_, i) => ({ label: `t${i}`, have: 1 })));
    type("one-more");
    press("Enter");
    expect(root.querySelector(".wf-tag-editor-msg")!.textContent).toContain(`up to ${MAX_TAGS_PER_WORKFLOW} tags`);
    expect(chips()).toHaveLength(MAX_TAGS_PER_WORKFLOW);
  });

  it("badges partial tags in a multi-edit and reports add and remove sets", () => {
    const ed = mk([{ label: "both", have: 2 }, { label: "one", have: 1 }], 2);
    expect(root.querySelector(".wf-tag-chip-count")!.textContent).toBe("1/2");
    type("one");
    press("Enter");
    root.querySelector<HTMLElement>(".wf-tag-chip-remove")!.click();
    expect(ed.getEdit()).toEqual({ add: ["one"], remove: ["both"] });
  });
});

describe("openTagPopover", () => {
  const open = (onApply: (e: unknown) => void) => openTagPopover({
    anchor: document.getElementById("inv")!, invoker: document.getElementById("inv")!,
    label: "Edit tags", initial: [{ label: "a", have: 1 }], total: 1, getLibrary: () => library, onApply,
  });
  const pop = () => document.querySelector<HTMLElement>(".wf-tag-popover");
  const typeIn = (v: string) => {
    const i = pop()!.querySelector("input")!;
    i.value = v;
    i.dispatchEvent(new Event("input"));
    i.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  };

  it("applies once on Done", () => {
    const apply = vi.fn();
    open(apply);
    typeIn("b");
    pop()!.querySelector<HTMLElement>(".wf-tag-popover-done")!.click();
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply).toHaveBeenCalledWith({ add: ["b"], remove: [] });
    expect(pop()).toBeNull();
  });

  it("discards on Escape and returns focus to the invoker", () => {
    const apply = vi.fn();
    open(apply);
    typeIn("b");
    pop()!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(apply).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(document.getElementById("inv"));
  });

  it("applies on an outside click and writes nothing when unchanged", async () => {
    const apply = vi.fn();
    open(apply);
    await new Promise(r => setTimeout(r, 0));
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(apply).not.toHaveBeenCalled();
    open(apply);
    typeIn("c");
    await new Promise(r => setTimeout(r, 0));
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(apply).toHaveBeenCalledTimes(1);
  });
});
