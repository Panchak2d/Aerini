import DOMPurify from "dompurify";
import { wrapIconSvg } from "./icon-cache";

// The documented, public shape-element allowlist (see docs/development/plugin-authoring.md,
// "Icon and identity metadata"). Deliberately excludes "svg" itself -- an
// icon is inner shape markup only, no wrapper of its own.
const SHAPE_TAGS = ["path", "circle", "rect", "line", "ellipse", "polygon", "polyline", "g"];
const ALLOWED_ATTR = [
  "d", "cx", "cy", "r", "rx", "ry", "x", "y", "x1", "y1", "x2", "y2",
  "width", "height", "points", "transform",
];
const FORBID_TAGS = ["script", "style", "foreignObject", "image", "use", "a", "animate", "animateTransform", "set", "iframe"];
const FORBID_ATTR = ["style", "href", "xlink:href", "class", "id"];

// Original glyph -- authored for this fallback, not derived from Lucide or
// any other icon library's path data. A rounded body with a circular tab,
// in the same stroke-only visual language as NODE_SVG_INNER's entries.
const GENERIC_PLUGIN_ICON = `<rect x="4" y="8" width="16" height="12" rx="2" /><circle cx="12" cy="6" r="3" />`;

export function getPluginIconSvg(rawInner: string | undefined): string {
  const trimmed = (rawInner ?? "").trim();
  if (!trimmed) return wrapIconSvg(GENERIC_PLUGIN_ICON);

  // DOMPurify drops bare SVG shape elements outright when they aren't inside
  // a recognized <svg> root -- it has no foreign-content context to parse
  // them into and silently discards them, even with ALLOWED_TAGS set. Wrap
  // first so it parses correctly, then strip every <svg>/</svg> tag from the
  // result: ours, and any wrapper the plugin itself supplied (not on the
  // public allowlist above, so it's treated like any other disallowed tag --
  // stripped, content kept, same as DOMPurify's own KEEP_CONTENT default).
  const sanitized = DOMPurify.sanitize(`<svg>${trimmed}</svg>`, {
    ALLOWED_TAGS: [...SHAPE_TAGS, "svg"],
    ALLOWED_ATTR,
    FORBID_TAGS,
    FORBID_ATTR,
  });
  const clean = sanitized.replace(/<\/?svg[^>]*>/gi, "").trim();

  return clean ? wrapIconSvg(clean) : wrapIconSvg(GENERIC_PLUGIN_ICON);
}
