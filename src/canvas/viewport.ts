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
