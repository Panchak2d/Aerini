/* @vitest-environment jsdom */
import { describe, it, expect, beforeEach } from "vitest";
import { bindSettingsModal, focusableIn, trapTarget } from "../settings-modal";

const flush = () => new Promise<void>(resolve => setTimeout(resolve, 0));

const DOM = `
  <button id="btn-open">Open</button>
  <div id="m" class="hidden">
    <div class="settings-box">
      <button id="btn-close"></button>
      <div class="settings-layout">
        <nav class="settings-rail">
          <button class="settings-rail-item" data-section="a">A</button>
          <button class="settings-rail-item" data-section="b">B</button>
        </nav>
        <div class="settings-body">
          <div class="settings-group" data-section="a">
            <select id="seg" data-segmented>
              <option value="x">X</option>
              <option value="y">Y</option>
              <option value="z">Z</option>
            </select>
          </div>
          <div class="settings-group" data-section="b" hidden>
            <input id="b-input" />
            <div id="hidden-row" hidden><input id="hidden-input" /></div>
          </div>
        </div>
      </div>
    </div>
  </div>
`;

const $ = (id: string) => document.getElementById(id)!;
const modal = () => $("m");
const railItem = (section: string) =>
  document.querySelector<HTMLButtonElement>(`.settings-rail-item[data-section="${section}"]`)!;
const group = (section: string) =>
  document.querySelector<HTMLElement>(`.settings-group[data-section="${section}"]`)!;

async function open(): Promise<void> {
  $("btn-open").focus();
  modal().classList.remove("hidden");
  await flush();
}

async function close(): Promise<void> {
  modal().classList.add("hidden");
  await flush();
}

beforeEach(() => {
  document.body.innerHTML = DOM;
  bindSettingsModal("m", "btn-open");
});

describe("trapTarget", () => {
  const mk = () => [document.createElement("button"), document.createElement("button"), document.createElement("button")];

  it("wraps Tab from the last item to the first and Shift+Tab from the first to the last", () => {
    const [a, b, c] = mk();
    expect(trapTarget([a, b, c], c, false)).toBe(a);
    expect(trapTarget([a, b, c], a, true)).toBe(c);
  });

  it("leaves movement inside the dialog to the browser, and pulls stray focus in", () => {
    const [a, b, c] = mk();
    expect(trapTarget([a, b, c], b, false)).toBeNull();
    expect(trapTarget([a, b, c], document.body, false)).toBe(a);
    expect(trapTarget([a, b, c], document.body, true)).toBe(c);
    expect(trapTarget([], a, false)).toBeNull();
  });
});

describe("bindSettingsModal", () => {
  it("shows only the first section initially and switches sections from the rail", () => {
    expect(group("a").hidden).toBe(false);
    expect(group("b").hidden).toBe(true);
    expect(railItem("a").getAttribute("aria-current")).toBe("true");

    railItem("b").click();

    expect(group("a").hidden).toBe(true);
    expect(group("b").hidden).toBe(false);
    expect(railItem("b").getAttribute("aria-current")).toBe("true");
    expect(railItem("a").hasAttribute("aria-current")).toBe(false);
  });

  it("builds a section select that stays in step with the rail", () => {
    const select = document.querySelector<HTMLSelectElement>(".settings-rail-select")!;
    expect(Array.from(select.options).map(o => o.value)).toEqual(["a", "b"]);

    select.value = "b";
    select.dispatchEvent(new Event("change"));
    expect(group("b").hidden).toBe(false);
    expect(railItem("b").getAttribute("aria-current")).toBe("true");

    railItem("a").click();
    expect(select.value).toBe("a");
  });

  it("resets to the first section and focuses the rail on every open", async () => {
    await open();
    railItem("b").click();
    await close();

    await open();

    expect(group("a").hidden).toBe(false);
    expect(document.activeElement).toBe(railItem("a"));
  });

  it("returns focus to the opener on close, and wraps Tab inside the dialog while open", async () => {
    await open();

    const last = $("b-input");
    railItem("b").click();
    last.focus();
    const tab = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    last.dispatchEvent(tab);
    expect(tab.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe($("btn-close"));

    await close();
    expect(document.activeElement).toBe($("btn-open"));
  });

  it("focusableIn leaves out controls in hidden sections and hidden rows", () => {
    railItem("b").click();
    const box = document.querySelector<HTMLElement>(".settings-box")!;
    const ids = focusableIn(box).map(el => el.id).filter(Boolean);

    expect(ids).toContain("b-input");
    expect(ids).not.toContain("hidden-input");
    expect(ids).not.toContain("seg");
  });

  it("segmented control writes through to the select and fires one change event", () => {
    const select = $("seg") as HTMLSelectElement;
    let changes = 0;
    select.addEventListener("change", () => { changes++; });
    const items = Array.from(document.querySelectorAll<HTMLButtonElement>(".settings-segmented-item"));
    expect(select.hidden).toBe(true);
    expect(items.map(i => i.getAttribute("aria-checked"))).toEqual(["true", "false", "false"]);

    items[2].click();
    items[2].click();

    expect(select.value).toBe("z");
    expect(changes).toBe(1);
    expect(items.map(i => i.getAttribute("aria-checked"))).toEqual(["false", "false", "true"]);
  });

  it("segmented control moves with arrow keys and wraps at the ends", () => {
    const select = $("seg") as HTMLSelectElement;
    const radiogroup = document.querySelector<HTMLElement>(".settings-segmented")!;
    const items = Array.from(radiogroup.querySelectorAll<HTMLButtonElement>(".settings-segmented-item"));

    items[0].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    expect(select.value).toBe("z");

    items[2].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    expect(select.value).toBe("x");
  });

  it("does nothing when the modal has no dialog box", () => {
    document.body.innerHTML = `<div id="bare" class="hidden"></div>`;
    expect(() => bindSettingsModal("bare", "none")).not.toThrow();
    expect(() => bindSettingsModal("missing", "none")).not.toThrow();
  });
});
