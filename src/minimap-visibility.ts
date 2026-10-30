import { storageGet, storageSet } from "./safe-storage";

export const MINIMAP_HIDDEN_KEY = "aerini_minimap_hidden";

function apply(hidden: boolean): void {
  document.getElementById("minimap-wrap")?.classList.toggle("minimap-hidden", hidden);
  document.getElementById("btn-minimap-toggle")?.setAttribute("aria-pressed", String(!hidden));
}

export function toggleMinimap(): void {
  const wrap = document.getElementById("minimap-wrap");
  if (!wrap) return;
  const hidden = !wrap.classList.contains("minimap-hidden");
  apply(hidden);
  storageSet(MINIMAP_HIDDEN_KEY, String(hidden));
}

export function restoreMinimapVisibility(): void {
  apply(storageGet(MINIMAP_HIDDEN_KEY) === "true");
}
