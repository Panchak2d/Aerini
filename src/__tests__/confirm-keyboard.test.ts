// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";

function setupDom(): void {
  document.body.innerHTML = `
    <button id="opener">Run</button>
    <div id="confirm-modal" class="hidden">
      <div class="confirm-backdrop"></div>
      <div class="confirm-message" id="confirm-message"></div>
      <button id="confirm-cancel">Cancel</button>
      <button id="confirm-alt" class="hidden"></button>
      <button id="confirm-ok">Continue</button>
    </div>`;
}

const key = (k: string, init: KeyboardEventInit = {}) =>
  document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));

const flush = () => new Promise(r => setTimeout(r, 0));

beforeEach(() => {
  vi.resetModules();
  setupDom();
});

describe("showConfirm keyboard behavior", () => {
  it("Escape cancels and returns focus to the element that opened the dialog", async () => {
    const { showConfirm } = await import("../confirm");
    const opener = document.getElementById("opener") as HTMLButtonElement;
    opener.focus();
    const result = showConfirm("Sure?", true);
    await flush();
    expect(document.activeElement).toBe(document.getElementById("confirm-cancel"));
    key("Escape");
    expect(await result).toBe(false);
    expect(document.getElementById("confirm-modal")!.classList.contains("hidden")).toBe(true);
    expect(document.activeElement).toBe(opener);
  });

  it("a non-danger prompt opens on the confirm button", async () => {
    const { showConfirm } = await import("../confirm");
    const result = showConfirm("Go?", false, "Go", "neutral");
    await flush();
    expect(document.activeElement).toBe(document.getElementById("confirm-ok"));
    document.getElementById("confirm-ok")!.click();
    expect(await result).toBe(true);
  });

  it("keeps global shortcuts from seeing keys while open, and Tab stays inside the dialog", async () => {
    const { showConfirm } = await import("../confirm");
    const seen = vi.fn();
    window.addEventListener("keydown", seen);
    const result = showConfirm("Sure?", true);
    await flush();
    key("Enter", { ctrlKey: true });
    expect(seen).not.toHaveBeenCalled();
    document.getElementById("confirm-ok")!.focus();
    key("Tab");
    expect(document.activeElement).toBe(document.getElementById("confirm-cancel"));
    key("Escape");
    await result;
    window.removeEventListener("keydown", seen);
  });

  it("showChoice returns ok / alt / cancel, and Esc is cancel, never the alt action", async () => {
    const { showChoice } = await import("../confirm");
    const alt = document.getElementById("confirm-alt")!;
    const cancel = document.getElementById("confirm-cancel")!;

    const a = showChoice("Pick", { okLabel: "Open", altLabel: "Run Once" });
    await flush();
    expect(alt.textContent).toBe("Run Once");
    expect(alt.classList.contains("hidden")).toBe(false);
    alt.click();
    expect(await a).toBe("alt");

    const b = showChoice("Pick", { okLabel: "Open", altLabel: "Run Once" });
    await flush();
    key("Escape");
    expect(await b).toBe("cancel");

    const c = showChoice("Pick", { okLabel: "Open", altLabel: "Run Once", cancelLabel: "Never mind" });
    await flush();
    expect(cancel.textContent).toBe("Never mind");
    document.getElementById("confirm-ok")!.click();
    expect(await c).toBe("ok");
  });

  it("a plain showConfirm after showChoice hides the alt button and restores Cancel", async () => {
    const { showChoice, showConfirm } = await import("../confirm");
    const first = showChoice("Pick", { okLabel: "Open", altLabel: "Run Once", cancelLabel: "Never mind" });
    await flush();
    document.getElementById("confirm-cancel")!.click();
    await first;

    const second = showConfirm("Sure?", true);
    await flush();
    expect(document.getElementById("confirm-alt")!.classList.contains("hidden")).toBe(true);
    expect(document.getElementById("confirm-cancel")!.textContent).toBe("Cancel");
    document.getElementById("confirm-cancel")!.click();
    expect(await second).toBe(false);
  });
});
