/* @vitest-environment jsdom */

import { readFileSync } from "node:fs";
import { describe, it, expect, vi, beforeAll, beforeEach } from "vitest";

const mocks = vi.hoisted(() => ({ openUrl: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: mocks.openUrl }));

import { bindExternalLinks, EXTERNAL_URL_SCOPE } from "../external-links";

const toast = vi.fn();
let preventedBeforeWindow = false;

function click(href: string, attrs: Record<string, string> = {}): boolean {
  const a = document.createElement("a");
  a.setAttribute("href", href);
  for (const [k, v] of Object.entries(attrs)) a.setAttribute(k, v);
  document.body.append(a);
  preventedBeforeWindow = false;
  a.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
  return preventedBeforeWindow;
}

beforeAll(() => {
  window.addEventListener("click", (e) => {
    preventedBeforeWindow = e.defaultPrevented;
    e.preventDefault();
  });
});

beforeEach(() => {
  mocks.openUrl.mockReset().mockResolvedValue(undefined);
  toast.mockReset();
  document.body.replaceChildren();
});

describe("bindExternalLinks outside Tauri", () => {
  it("leaves every link to the browser", () => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    bindExternalLinks(toast);
    expect(click("https://github.com/Panchak2d/aerini", { target: "_blank" })).toBe(false);
    expect(mocks.openUrl).not.toHaveBeenCalled();
  });
});

describe("bindExternalLinks in Tauri", () => {
  beforeAll(() => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    bindExternalLinks(toast);
  });

  it("opens allowlisted https links through openUrl, cancels native navigation and reports a failed open", async () => {
    for (const href of [
      "https://github.com/Panchak2d/aerini/releases/latest",
      "https://www.patreon.com/14702118/join",
      "https://www.youtube.com/watch?v=abc123",
    ]) {
      expect(click(href, { target: "_blank" })).toBe(true);
      expect(mocks.openUrl).toHaveBeenLastCalledWith(href);
    }
    expect(click("https://ngrok.com", { target: "_blank" })).toBe(true);
    expect(mocks.openUrl).toHaveBeenLastCalledWith("https://ngrok.com/");

    mocks.openUrl.mockRejectedValue(new Error("no browser"));
    click("https://ngrok.com/", { target: "_blank" });
    await vi.waitFor(() => expect(toast).toHaveBeenCalledWith("Couldn't open the link in your browser.", "error"));
  });

  it("blocks other hosts, lookalikes and non-https links, and leaves in-app, download and non-web links alone", () => {
    for (const href of [
      "https://evil.example/",
      "https://github.com/Panchak2dx/aerini",
      "https://www.patreon.com.evil.example/x",
      "http://github.com/Panchak2d/aerini",
    ]) {
      expect(click(href, { target: "_blank" })).toBe(true);
    }
    expect(toast).toHaveBeenCalledTimes(4);

    expect(click("#section")).toBe(false);
    expect(click("/relative")).toBe(false);
    expect(click("http://asset.localhost/file.png", { download: "file.png" })).toBe(false);
    expect(click("mailto:someone@example.com")).toBe(false);
    expect(mocks.openUrl).not.toHaveBeenCalled();
    expect(toast).toHaveBeenCalledTimes(4);
  });
});

describe("EXTERNAL_URL_SCOPE", () => {
  it("matches the opener scope granted in capabilities/default.json", () => {
    const caps = JSON.parse(readFileSync("src-tauri/capabilities/default.json", "utf8")) as {
      permissions: Array<string | { identifier: string; allow?: Array<{ url: string }> }>;
    };
    const grants = caps.permissions.filter(
      (p): p is { identifier: string; allow?: Array<{ url: string }> } =>
        typeof p === "object" && p.identifier.startsWith("opener:"),
    );
    expect(grants.map((g) => g.identifier)).toEqual(["opener:allow-open-url"]);
    expect(grants[0].allow?.map((e) => e.url)).toEqual([...EXTERNAL_URL_SCOPE]);
  });
});
