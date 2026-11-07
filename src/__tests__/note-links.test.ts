// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";

const mocks = vi.hoisted(() => ({
  showConfirm: vi.fn(),
  isTauri: vi.fn(),
  openExternal: vi.fn(),
  showToast: vi.fn(),
}));

vi.mock("../confirm", () => ({ showConfirm: mocks.showConfirm }));
vi.mock("../utils", () => ({ isTauri: mocks.isTauri }));
vi.mock("../ipc/note-link", () => ({ openNoteLinkExternal: mocks.openExternal }));
vi.mock("../toast", () => ({ showToast: mocks.showToast }));

import { openNoteLink } from "../note-links";

let windowOpen: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.clearAllMocks();
  mocks.showConfirm.mockResolvedValue(true);
  mocks.isTauri.mockReturnValue(false);
  mocks.openExternal.mockResolvedValue(undefined);
  windowOpen = vi.fn();
  vi.stubGlobal("open", windowOpen);
});

describe("openNoteLink", () => {
  it.each([
    "javascript:alert(1)",
    "data:text/html,<b>x</b>",
    "file:///etc/passwd",
    "ftp://example.com/a",
    "https://user:pw@example.com/",
    "not a url",
  ])("ignores %s without showing a confirm", async (raw) => {
    await openNoteLink(raw);
    expect(mocks.showConfirm).not.toHaveBeenCalled();
    expect(windowOpen).not.toHaveBeenCalled();
    expect(mocks.openExternal).not.toHaveBeenCalled();
  });

  it("shows the punycode site and the full address in the confirm", async () => {
    await openNoteLink("https://bücher.example/path?q=1");
    expect(mocks.showConfirm).toHaveBeenCalledTimes(1);
    const message = mocks.showConfirm.mock.calls[0][0] as string;
    expect(message).toContain("Site: xn--bcher-kva.example");
    expect(message).toContain("Address: https://xn--bcher-kva.example/path?q=1");
  });

  it("opens nothing when the confirm is cancelled", async () => {
    mocks.showConfirm.mockResolvedValue(false);
    await openNoteLink("https://example.com/");
    expect(windowOpen).not.toHaveBeenCalled();
    expect(mocks.openExternal).not.toHaveBeenCalled();
  });

  it("opens a new tab without opener access in browser mode", async () => {
    await openNoteLink("https://example.com/a");
    expect(windowOpen).toHaveBeenCalledWith("https://example.com/a", "_blank", "noopener,noreferrer");
    expect(mocks.openExternal).not.toHaveBeenCalled();
  });

  it("hands the address to the native command in the desktop app", async () => {
    mocks.isTauri.mockReturnValue(true);
    await openNoteLink("https://example.com/a");
    expect(mocks.openExternal).toHaveBeenCalledWith("https://example.com/a");
    expect(windowOpen).not.toHaveBeenCalled();
  });

  it("shows a toast when the native command fails", async () => {
    mocks.isTauri.mockReturnValue(true);
    mocks.openExternal.mockRejectedValue(new Error("refused"));
    await openNoteLink("https://example.com/a");
    await Promise.resolve();
    await Promise.resolve();
    expect(mocks.showToast).toHaveBeenCalledWith("Couldn't open the link in your browser.", "error");
  });
});
