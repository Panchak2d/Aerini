/* @vitest-environment jsdom */
import { describe, it, expect } from "vitest";
import { getIconSvg } from "../icon-cache";

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
