export function bindDrawerResize(): void {
  const handle = document.getElementById("drawer-resize-handle")!;
  const drawer = document.getElementById("output-drawer")!;
  let startY = 0, startH = 0, dragging = false;
  handle.addEventListener("mousedown", e => {
    dragging = true; startY = e.clientY; startH = drawer.offsetHeight;
    handle.classList.add("dragging"); e.preventDefault();
  });
  window.addEventListener("mousemove", e => {
    if (!dragging) return;
    const newH = Math.max(120, Math.min(window.innerHeight * 0.7, startH + (startY - e.clientY)));
    drawer.style.height = `${newH}px`;
    document.documentElement.style.setProperty("--drawer-h", `${newH}px`);
  });
  window.addEventListener("mouseup", () => {
    if (dragging) { dragging = false; handle.classList.remove("dragging"); }
  });
}

export function bindPanelResize(): void {
  const handle = document.getElementById("panel-resize-handle")!;
  let startX = 0, startW = 0, dragging = false;
  handle.addEventListener("mousedown", e => {
    dragging = true; startX = e.clientX;
    startW = document.getElementById("right-panel")!.offsetWidth;
    handle.classList.add("dragging"); e.preventDefault();
  });
  window.addEventListener("mousemove", e => {
    if (!dragging) return;
    const newW = Math.max(240, Math.min(600, startW + (startX - e.clientX)));
    document.documentElement.style.setProperty("--panel-w", `${newW}px`);
  });
  window.addEventListener("mouseup", () => {
    if (dragging) { dragging = false; handle.classList.remove("dragging"); }
  });
}
