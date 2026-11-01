/* @vitest-environment jsdom */
import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { accentColors, getIconBitmap, getIconSvg, preloadAllIcons } from "../icon-cache";
import { _resetCanvasColorCache } from "../canvas/theme-colors";
import { NODE_IDS } from "../node-ids";

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

  it("gives Feed Parser an ordinary monochrome icon", () => {
    const svg = getIconSvg(NODE_IDS.FEED_PARSER);
    expect(svg).toContain('stroke="currentColor"');
    expect(svg).toContain('<circle cx="5" cy="19" r="1"');
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

describe("icon loading on demand", () => {
  const flush = () => new Promise<void>((r) => setTimeout(r, 0));

  async function fresh() {
    vi.resetModules();
    const icons = await import("../icon-cache");
    const state = await import("../canvas/render-state");
    return { icons, state };
  }

  function stubDecoder(dpr = 1): { svgs: string[] } {
    const svgs: string[] = [];
    class FakeImage {
      svg = "";
      decode = vi.fn(async () => {});
      set src(v: string) { this.svg = decodeURIComponent(v); svgs.push(this.svg); }
    }
    vi.stubGlobal("Image", FakeImage);
    vi.stubGlobal("createImageBitmap", vi.fn(async (img: FakeImage) => ({ svg: img.svg }) as unknown as ImageBitmap));
    Object.defineProperty(window, "devicePixelRatio", { value: dpr, configurable: true });
    return { svgs };
  }

  beforeEach(() => { stubDecoder(); });
  afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

  it("an icon drawn before it is ready starts loading and requests a repaint when it arrives", async () => {
    const { icons, state } = await fresh();
    expect(icons.getIconBitmap("webhook", "#4d9eff")).toBeNull();
    state.consumeDirty();
    await flush();
    expect(icons.getIconBitmap("webhook", "#4d9eff")).not.toBeNull();
    expect(state.consumeDirty()).toBe(true);
  });

  it("zoomed in, a sharper raster replaces the base one once it has loaded", async () => {
    stubDecoder(2);
    const { icons } = await fresh();
    icons.getIconBitmap("webhook", "#4d9eff", 1);
    await flush();
    const base = icons.getIconBitmap("webhook", "#4d9eff", 3) as unknown as { svg: string };
    expect(base.svg).toContain('width="32"');
    await flush();
    const sharp = icons.getIconBitmap("webhook", "#4d9eff", 3) as unknown as { svg: string };
    expect(sharp.svg).toContain('width="128"');
  });

  it("preloadAllIcons works through the library in idle-callback slices", async () => {
    const idle: Array<() => void> = [];
    vi.stubGlobal("requestIdleCallback", (cb: () => void) => { idle.push(cb); return idle.length; });
    const { icons } = await fresh();
    const done = icons.preloadAllIcons().then(() => true);
    await flush();
    expect(idle.length).toBe(1);
    idle.shift()!();
    await flush();
    expect(idle.length).toBe(1);
    const color = icons.accentColors()[0];
    expect(icons.getIconBitmap("manual_trigger", color)).not.toBeNull();
    expect(icons.getIconBitmap("sendgrid", color)).toBeNull();
    let finished = false;
    void done.then(() => { finished = true; });
    while (!finished) {
      idle.shift()?.();
      await flush();
    }
    expect(icons.getIconBitmap("sendgrid", color)).not.toBeNull();
  });

  it("ensureIcons resolves when the icons are ready, and at the cap when a decode never finishes", async () => {
    const { icons } = await fresh();
    await icons.ensureIcons(["webhook"], 5000);
    expect(icons.getIconBitmap("webhook", icons.accentColors()[0])).not.toBeNull();

    stubDecoder();
    vi.stubGlobal("createImageBitmap", vi.fn(() => new Promise(() => {})));
    const stuck = await fresh();
    vi.useFakeTimers();
    let resolved = false;
    void stuck.icons.ensureIcons(["schedule"], 200).then(() => { resolved = true; });
    await vi.advanceTimersByTimeAsync(199);
    expect(resolved).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(resolved).toBe(true);
  });
});
