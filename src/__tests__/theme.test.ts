// @vitest-environment jsdom
 
import { describe, it, expect, beforeEach } from "vitest";
import { THEMES, getStoredTheme, applyTheme, initTheme } from "../theme";

function installLocalStorageStub(): void {
  const store = new Map<string, string>();
  const stub: Pick<Storage, "getItem" | "setItem" | "removeItem" | "clear"> = {
    getItem: (key: string) => (store.has(key) ? store.get(key)! : null),
    setItem: (key: string, value: string) => { store.set(key, String(value)); },
    removeItem: (key: string) => { store.delete(key); },
    clear: () => { store.clear(); },
  };
  globalThis.localStorage = stub as Storage;
}

beforeEach(() => {
  installLocalStorageStub();
  document.documentElement.removeAttribute("data-theme");
});

describe("THEMES registry", () => {
  it("contains midnight (default) and paper, each with an id and label", () => {
    const ids = THEMES.map(t => t.id);
    expect(ids).toContain("midnight");
    expect(ids).toContain("paper");
    for (const t of THEMES) {
      expect(t.id).toBeTruthy();
      expect(t.label).toBeTruthy();
    }
  });
});

describe("applyTheme", () => {
  it("sets data-theme for a non-default theme and persists it", () => {
    applyTheme("paper");
    expect(document.documentElement.getAttribute("data-theme")).toBe("paper");
    expect(localStorage.getItem("aerini_theme")).toBe("paper");
  });

  it("removes data-theme for the default (midnight) rather than setting it literally", () => {
    document.documentElement.setAttribute("data-theme", "paper");
    applyTheme("midnight");
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
    expect(localStorage.getItem("aerini_theme")).toBe("midnight");
  });

  it("falls back to the default for an unrecognized id instead of writing an unmatched attribute value", () => {
    applyTheme("some-removed-future-theme");
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
    expect(localStorage.getItem("aerini_theme")).toBe("midnight");
  });
});

describe("getStoredTheme", () => {
  it("returns the default when nothing is stored", () => {
    expect(getStoredTheme()).toBe("midnight");
  });

  it("returns a previously-applied, known theme id", () => {
    applyTheme("paper");
    expect(getStoredTheme()).toBe("paper");
  });

  it("falls back to the default for an unrecognized stored value (e.g. hand-edited storage, or a removed theme)", () => {
    localStorage.setItem("aerini_theme", "not-a-real-theme");
    expect(getStoredTheme()).toBe("midnight");
  });
});

describe("initTheme", () => {
  it("re-applies a persisted non-default theme to <html>", () => {
    localStorage.setItem("aerini_theme", "paper");
    initTheme();
    expect(document.documentElement.getAttribute("data-theme")).toBe("paper");
  });

  it("leaves <html> without the attribute when nothing (or the default) was stored", () => {
    initTheme();
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
  });
});
