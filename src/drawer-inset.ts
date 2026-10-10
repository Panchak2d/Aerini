// --drawer-h is not the drawer's rendered height: a collapsed drawer is
// `height: auto`, the expand button sets an inline `70vh`, and min/max-height
// clamp both. Observing the element tracks the real box through drag-resize
// and the 180ms height transition alike.
export function bindDrawerInset(): void {
  const drawer = document.getElementById("output-drawer");
  if (!drawer || typeof ResizeObserver === "undefined") return;
  const sync = (): void => {
    document.documentElement.style.setProperty("--drawer-inset", `${drawer.offsetHeight}px`);
  };
  sync();
  new ResizeObserver(sync).observe(drawer);
}
