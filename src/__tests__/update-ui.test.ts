// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("../ipc/update", () => ({
  checkForUpdate: vi.fn(),
  installUpdate: vi.fn(),
  cancelUpdateDownload: vi.fn(),
  takeUpdateNotice: vi.fn(),
  listenUpdateProgress: vi.fn(),
}));
vi.mock("../confirm", () => ({ showConfirm: vi.fn() }));

import { applyView, stateFromCheck, viewFor } from "../update-ui";
import type { UpdateEls, UpdateState } from "../update-ui";
import type { UpdateCheckResult } from "../ipc/update";

const RELEASES = "https://github.com/Panchak2d/aerini/releases/latest";

function mountRow(): UpdateEls {
  document.body.innerHTML = `
    <div id="update-check-status"></div>
    <div id="update-progress-wrap" hidden><progress id="update-progress" max="100"></progress><span id="update-percent"></span></div>
    <button id="btn-cancel-update" hidden></button>
    <button id="btn-install-update" hidden></button>
    <button id="btn-check-updates"></button>`;
  const q = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
  return {
    status: q("update-check-status"),
    progressWrap: q("update-progress-wrap"),
    progress: q("update-progress"),
    percent: q("update-percent"),
    checkBtn: q<HTMLButtonElement>("btn-check-updates"),
    installBtn: q<HTMLButtonElement>("btn-install-update"),
    cancelBtn: q<HTMLButtonElement>("btn-cancel-update"),
  };
}

function show(els: UpdateEls, state: UpdateState): void {
  applyView(els, viewFor(state));
}

const check = (over: Partial<UpdateCheckResult>): UpdateCheckResult => ({
  current_version: "0.4.1",
  available: true,
  latest_version: "0.5.0",
  install_support: { kind: "supported", admin_prompt: false },
  release_url: "https://github.com/Panchak2d/aerini/releases/tag/v0.5.0",
  ...over,
});

describe("update row rendering", () => {
  let els: UpdateEls;
  beforeEach(() => { els = mountRow(); });

  it("maps each state to the right buttons, progress bar and release link", () => {
    show(els, stateFromCheck(check({})));
    expect(els.status.textContent).toContain("v0.5.0 is available");
    expect(els.installBtn.hidden).toBe(false);
    expect(els.cancelBtn.hidden).toBe(true);
    expect(els.status.querySelector("a")?.getAttribute("href")).toBe(
      "https://github.com/Panchak2d/aerini/releases/tag/v0.5.0",
    );

    show(els, { kind: "downloading", latest: "0.5.0", downloaded: 25, total: 100, cancelling: false });
    expect(els.progressWrap.hidden).toBe(false);
    expect(els.progress.getAttribute("value")).toBe("25");
    expect(els.percent.textContent).toBe("25%");
    expect(els.installBtn.hidden).toBe(true);
    expect(els.cancelBtn.hidden).toBe(false);
    expect(els.checkBtn.disabled).toBe(true);

    show(els, { kind: "downloading", latest: "0.5.0", downloaded: 5, total: null, cancelling: false });
    expect(els.progress.hasAttribute("value")).toBe(false);

    show(els, { kind: "installing", latest: "0.5.0" });
    expect(els.cancelBtn.hidden).toBe(true);

    show(els, stateFromCheck(check({ available: false, latest_version: null })));
    expect(els.status.textContent).toBe("You're up to date (v0.4.1).");
    expect(els.progressWrap.hidden).toBe(true);
    expect(els.checkBtn.disabled).toBe(false);
  });

  it("never turns version, message or URL strings into markup or a non-GitHub link", () => {
    const payload = `<img src=x onerror="window.__pwned=1">`;
    show(els, { kind: "manual", latest: payload, releaseUrl: "javascript:alert(1)", message: payload });
    expect(els.status.querySelector("img")).toBeNull();
    expect(els.status.textContent).toContain(payload);
    expect(els.status.querySelector("a")?.getAttribute("href")).toBe(RELEASES);

    show(els, { kind: "error", message: payload, releaseUrl: "https://evil.example/x" });
    expect(els.status.querySelector("img")).toBeNull();
    expect(els.status.querySelector("a")?.getAttribute("href")).toBe(RELEASES);
  });

  it("offers only a manual download, with the reason, when the install cannot be done in-app", () => {
    show(els, stateFromCheck(check({
      install_support: {
        kind: "manual",
        reason: "macos_disk_image",
        message: "Aerini is running from a disk image. Move it to Applications and reopen it first.",
      },
    })));
    expect(els.installBtn.hidden).toBe(true);
    expect(els.status.textContent).toContain("Move it to Applications");
    expect(els.status.querySelector("a")?.textContent).toBe("Download from GitHub");
  });
});
