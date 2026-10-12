// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";

function setupDom(): void {
  document.body.innerHTML = `
    <button id="opener">Run</button>
    <div id="confirm-modal" class="hidden">
      <div class="confirm-backdrop"></div>
      <div class="confirm-message" id="confirm-message"></div>
      <button id="confirm-cancel">Cancel</button>
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
});
