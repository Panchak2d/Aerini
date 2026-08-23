// @vitest-environment jsdom

import { describe, it, expect } from "vitest";
import { getPluginIconSvg } from "../plugin-icon";

describe("getPluginIconSvg", () => {
  it("passes clean, allowlisted shape markup through, wrapped for render", () => {
    const out = getPluginIconSvg('<circle cx="12" cy="12" r="9" /><path d="M8 12h8" />');
    expect(out).toContain('<circle cx="12" cy="12" r="9">');
    expect(out).toContain('<path d="M8 12h8">');
    expect(out.startsWith('<svg class="icon-svg"')).toBe(true);
  });

  it("falls back to the generic glyph for undefined, empty, or whitespace-only input", () => {
    const fallback = getPluginIconSvg(undefined);
    expect(fallback).toContain("<rect");
    expect(getPluginIconSvg("")).toBe(fallback);
    expect(getPluginIconSvg("   ")).toBe(fallback);
  });

  it("strips disallowed tags, attributes, and any plugin-supplied <svg> wrapper while keeping allowed content", () => {
    const malicious =
      '<circle cx="5" cy="5" r="3" style="fill:red" class="x" onclick="alert(1)" />' +
      "<script>alert(1)</script>" +
      '<a href="javascript:alert(1)"><rect x="1" y="1" width="2" height="2" /></a>' +
      "<foreignObject><div>hi</div></foreignObject>" +
      '<use href="#x"/>';
    const out = getPluginIconSvg(malicious);

    expect(out).not.toContain("<script");
    expect(out).not.toContain("onclick");
    expect(out).not.toContain("style=");
    expect(out).not.toContain('class="x"');
    expect(out).not.toContain("<a ");
    expect(out).not.toContain("foreignObject");
    expect(out).not.toContain("<use");
    // allowed elements survive, reparented out of the stripped wrapper
    expect(out).toContain('<circle cx="5" cy="5" r="3">');
    expect(out).toContain('<rect x="1" y="1" width="2" height="2">');

    // a plugin-supplied <svg> wrapper is itself outside the allowlist -- it
    // must not survive as a nested tag inside the host's own wrapper.
    const nested = getPluginIconSvg('<svg viewBox="0 0 99 99"><circle cx="1" cy="1" r="1"/></svg>');
    expect(nested).toContain('<circle cx="1" cy="1" r="1">');
    expect((nested.match(/<svg/g) ?? []).length).toBe(1); // only the host's own wrapper
  });
});
