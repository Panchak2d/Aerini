// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from "vitest";
import { copyText, copyWithButtonFeedback } from "../utils";

function stubClipboard(writeText: ((t: string) => Promise<void>) | undefined) {
  Object.defineProperty(navigator, "clipboard", {
    value: writeText ? { writeText } : undefined,
    configurable: true,
  });
}

afterEach(() => { vi.useRealTimers(); });

describe("copyText / copyWithButtonFeedback", () => {
  it("labels the button 'Copied' and restores the original label, even after a double click", async () => {
    vi.useFakeTimers();
    const writeText = vi.fn().mockResolvedValue(undefined);
    stubClipboard(writeText);
    const btn = document.createElement("button");
    btn.textContent = "Copy error";
    await copyWithButtonFeedback(btn, "boom");
    expect(writeText).toHaveBeenCalledWith("boom");
    expect(btn.textContent).toBe("Copied");
    await copyWithButtonFeedback(btn, "boom");
    vi.advanceTimersByTime(2000);
    expect(btn.textContent).toBe("Copy error");
  });

  it("does not reject when the clipboard is denied or absent, and restores the label after showing failure", async () => {
    vi.useFakeTimers();
    stubClipboard(vi.fn().mockRejectedValue(new Error("denied")));
    await expect(copyText("x")).resolves.toBe(false);
    const btn = document.createElement("button");
    btn.textContent = "Copy error";
    await copyWithButtonFeedback(btn, "x");
    expect(btn.textContent).toBe("Copy failed");
    vi.advanceTimersByTime(2000);
    expect(btn.textContent).toBe("Copy error");

    stubClipboard(undefined);
    await expect(copyText("x")).resolves.toBe(false);
  });
});
