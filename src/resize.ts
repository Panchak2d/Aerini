const DRAWER_MIN_H     = 120;   // matches #output-drawer's own CSS min-height
const DRAWER_MAX_H_PCT = 0.7;   // matches #output-drawer's own CSS max-height: 70vh
const DRAWER_DEFAULT_H = 260;   // matches --drawer-h's default in variables.css
const DRAWER_STEP      = 24;    // px per arrow-key press

export function bindDrawerResize(): void {
  const handle = document.getElementById("drawer-resize-handle")!;
  const drawer = document.getElementById("output-drawer")!;
  let startY = 0, startH = 0, dragging = false;

  // Bare div has no native resize semantics — make it a real, labeled,
  // keyboard-reachable separator instead of a mouse-only drag target.
  handle.tabIndex = 0;
  handle.setAttribute("role", "separator");
  handle.setAttribute("aria-orientation", "horizontal");
  handle.setAttribute("aria-label", "Resize output drawer");

  const setHeight = (h: number) => {
    const clamped = Math.max(DRAWER_MIN_H, Math.min(window.innerHeight * DRAWER_MAX_H_PCT, h));
    drawer.style.height = `${clamped}px`;
    document.documentElement.style.setProperty("--drawer-h", `${clamped}px`);
  };

  handle.addEventListener("mousedown", e => {
    dragging = true; startY = e.clientY; startH = drawer.offsetHeight;
    handle.classList.add("dragging");
    document.body.classList.add("resizing-v");
    e.preventDefault();
  });
  window.addEventListener("mousemove", e => {
    if (!dragging) return;
    setHeight(startH + (startY - e.clientY));
  });
  window.addEventListener("mouseup", () => {
    if (dragging) {
      dragging = false;
      handle.classList.remove("dragging");
      document.body.classList.remove("resizing-v");
    }
  });

  // Double-click: reset to the default height (mirrors the sidebar handle's
  // own dblclick-to-default convention).
  handle.addEventListener("dblclick", () => setHeight(DRAWER_DEFAULT_H));

  // Arrow keys resize while the handle has focus — same direction as
  // dragging (Up = taller, Down = shorter, since dragging the handle
  // upward is what grows the drawer).
  handle.addEventListener("keydown", e => {
    if (e.key === "ArrowUp")        { e.preventDefault(); setHeight(drawer.offsetHeight + DRAWER_STEP); }
    else if (e.key === "ArrowDown") { e.preventDefault(); setHeight(drawer.offsetHeight - DRAWER_STEP); }
  });
}
