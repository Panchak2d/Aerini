export interface VisibleViewport {
  width: number;
  height: number;
}

/** #output-drawer and #chat-panel are `position: fixed` overlays (workspace.css,
 * chat.css): they sit on top of #canvas without shrinking its box, so the
 * canvas rect alone overstates the area the user can actually see. Returns the
 * size of the unobstructed region of `r`, measured from its top-left corner.
 *
 * The chat panel is docked to the right edge and spans the canvas's full
 * height, so it only trims width. Its width comes from offsetWidth, not
 * getBoundingClientRect(): the slide-in animation translates the panel and a
 * rect read mid-animation would be off by the translation. */
export function visibleViewport(r: DOMRect): VisibleViewport {
  return { width: unobstructedWidth(r), height: unobstructedHeight(r) };
}

/** Regions of `r` (each measured from its top-left corner) that fit-to-screen may use.
 * The minimap is an absolutely positioned overlay in the bottom-right corner, so
 * the free area is not one rectangle: either everything above it or everything
 * left of it. Both are returned and the caller keeps whichever fits the content
 * larger. Without an overlapping minimap this is just `visibleViewport(r)`. */
export function fitViewports(r: DOMRect): VisibleViewport[] {
  const vp = visibleViewport(r);
  const wrap = document.getElementById("minimap-wrap");
  if (!wrap) return [vp];
  const m = wrap.getBoundingClientRect();
  if (!(m.width > 0 && m.height > 0)) return [vp];
  const left = m.left - r.left;
  const top = m.top - r.top;
  if (left >= vp.width || top >= vp.height || m.right <= r.left || m.bottom <= r.top) return [vp];
  return [
    { width: vp.width, height: Math.max(0, top) },
    { width: Math.max(0, left), height: vp.height },
  ];
}

function unobstructedWidth(r: DOMRect): number {
  const chat = document.getElementById("chat-panel");
  const chatWidth = chat ? chat.offsetWidth : 0;
  if (chatWidth <= 0) return r.width;
  const chatLeft = window.innerWidth - chatWidth;
  return Math.max(0, Math.min(r.right, chatLeft) - r.left);
}

function unobstructedHeight(r: DOMRect): number {
  const drawer = document.getElementById("output-drawer");
  if (!drawer || drawer.classList.contains("hidden")) return r.height;
  const dr = drawer.getBoundingClientRect();
  if (dr.right <= r.left || dr.left >= r.right) return r.height;
  return Math.max(0, Math.min(r.bottom, dr.top) - r.top);
}

export interface SavedViewport {
  panX: number;
  panY: number;
  zoom: number;
}

export function parseSavedViewport(raw: string, minZoom: number, maxZoom: number): SavedViewport | null {
  let v: unknown;
  try { v = JSON.parse(raw); } catch { return null; }
  if (typeof v !== "object" || v === null) return null;
  const { panX, panY, zoom } = v as Record<string, unknown>;
  if (typeof panX !== "number" || typeof panY !== "number" || typeof zoom !== "number") return null;
  if (!Number.isFinite(panX) || !Number.isFinite(panY) || !Number.isFinite(zoom)) return null;
  return { panX, panY, zoom: Math.min(maxZoom, Math.max(minZoom, zoom)) };
}
