/* @vitest-environment jsdom */
import { describe, it, expect, vi, afterEach } from "vitest";
import { accentColors, getIconBitmap, getIconSvg, preloadAllIcons } from "../icon-cache";
import { _resetCanvasColorCache } from "../canvas/theme-colors";

describe("getIconSvg — brand icons", () => {
  it("renders a saturated brand color as a literal fill, stroke none", () => {
    const svg = getIconSvg("discord");
    expect(svg).toContain('fill="#5865F2"');
    expect(svg).toContain('stroke="none"');
  });

  it("renders github/notion's fill as a CSS var so it stays legible across a live theme switch", () => {
    expect(getIconSvg("github")).toContain('fill="var(--brand-github)"');
    expect(getIconSvg("notion")).toContain('fill="var(--brand-notion)"');
  });

  it("leaves existing monochrome icons on the stroke/currentColor path, unaffected by the brand-fill branch", () => {
    const svg = getIconSvg("file");
    expect(svg).toContain('stroke="currentColor"');
    expect(svg).toContain('fill="none"');
  });

  it("returns an empty string for a type with no icon entry at all", () => {
    expect(getIconSvg("not_a_real_node_type")).toBe("");
  });

  it("renders Slack/SendGrid as ordinary monochrome icons, not a brand fill (no safe verified brand mark exists for either)", () => {
    const slack = getIconSvg("slack");
    const sendgrid = getIconSvg("sendgrid");
    expect(slack).toContain('stroke="currentColor"');
    expect(slack).not.toContain("var(--brand-");
    expect(sendgrid).toContain('stroke="currentColor"');
    expect(sendgrid).not.toContain("var(--brand-");
  });
});

describe("preloadAllIcons", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("decodes each SVG through an <img> loaded from a data: URL, which the production CSP allows (blob: is blocked)", async () => {
    const bitmap = { width: 16 } as ImageBitmap;
    const srcs: string[] = [];
    class FakeImage {
      decode = vi.fn(async () => {});
      set src(v: string) { srcs.push(v); }
    }
    vi.stubGlobal("Image", FakeImage);
    vi.stubGlobal("createImageBitmap", vi.fn(async (src: unknown) => {
      if (src instanceof Blob) throw new DOMException("The source image could not be decoded.", "InvalidStateError");
      return bitmap;
    }));
    document.documentElement.removeAttribute("data-theme");
    document.documentElement.removeAttribute("style");
    _resetCanvasColorCache();

    await preloadAllIcons();

    expect(getIconBitmap("webhook", accentColors()[0])).toBe(bitmap);
    expect(srcs.length).toBeGreaterThan(0);
    expect(srcs.every((u) => u.startsWith("data:image/svg+xml"))).toBe(true);
  });
});
