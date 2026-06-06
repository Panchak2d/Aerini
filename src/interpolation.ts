import type { Canvas } from "./canvas/Canvas";
import { escapeHtml as escHtml } from "./utils";

// Fires when the user types {{ in any input/textarea inside the popover.
// Shows a dropdown of available node output paths from the current canvas.

let _interpCanvas: Canvas | null = null;

export function initInterpolationAutocomplete(canvas: Canvas): void {
  _interpCanvas = canvas;

  document.addEventListener("input", (e) => {
    const el = e.target as HTMLElement;
    if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) return;
    if (!el.closest(".node-popover, #right-panel")) return;
    handleInterpolationInput(el);
  });

  document.addEventListener("keydown", (e) => {
    const dropdown = document.getElementById("interp-dropdown");
    if (!dropdown || dropdown.classList.contains("hidden")) return;
    if (e.key === "Escape") { dropdown.classList.add("hidden"); return; }
    const items  = dropdown.querySelectorAll<HTMLElement>(".interp-item");
    const active = dropdown.querySelector<HTMLElement>(".interp-item.active");
    const idx    = active ? [...items].indexOf(active) : -1;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      items[Math.min(idx + 1, items.length - 1)]?.classList.add("active");
      active?.classList.remove("active");
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      items[Math.max(idx - 1, 0)]?.classList.add("active");
      active?.classList.remove("active");
    } else if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      (active ?? items[0])?.click();
    }
  });

  document.addEventListener("mousedown", (e) => {
    const dd = document.getElementById("interp-dropdown");
    if (dd && !dd.contains(e.target as Node)) dd.classList.add("hidden");
  }, true);
}

function handleInterpolationInput(el: HTMLInputElement | HTMLTextAreaElement): void {
  const val      = el.value;
  const cursor   = el.selectionStart ?? val.length;
  const before   = val.slice(0, cursor);
  const triggerIdx = before.lastIndexOf("{{");
  if (triggerIdx === -1) {
    document.getElementById("interp-dropdown")?.classList.add("hidden");
    return;
  }

  const query   = before.slice(triggerIdx + 2).toLowerCase();
  const paths   = buildInterpolationPaths();
  const matches = paths.filter(p => p.toLowerCase().includes(query));

  showInterpolationDropdown(el, matches, (chosen) => {
    const after    = val.slice(cursor);
    const newVal   = val.slice(0, triggerIdx) + "{{" + chosen + "}}" + after;
    el.value = newVal;
    el.dispatchEvent(new Event("input", { bubbles: true }));
    const newCursor = triggerIdx + 2 + chosen.length + 2;
    el.setSelectionRange(newCursor, newCursor);
    document.getElementById("interp-dropdown")?.classList.add("hidden");
  });
}

function buildInterpolationPaths(): string[] {
  if (!_interpCanvas) return [];
  const paths: string[] = [];
  for (const node of _interpCanvas.nodes.values()) {
    const name = node.data.name;
    paths.push(`${name}`);
    paths.push(`${name}.output`);
    paths.push(`${name}.result`);
    paths.push(`${name}.content`);
    paths.push(`${name}.value`);
  }
  return paths;
}

function showInterpolationDropdown(
  anchor: HTMLElement,
  paths: string[],
  onSelect: (path: string) => void
): void {
  let dd = document.getElementById("interp-dropdown");
  if (!dd) {
    dd = document.createElement("div");
    dd.id = "interp-dropdown";
    document.body.appendChild(dd);
  }

  if (!paths.length) { dd.classList.add("hidden"); return; }

  const rect = anchor.getBoundingClientRect();
  dd.style.cssText = `position:fixed;left:${rect.left}px;top:${rect.bottom + 2}px;z-index:600;max-height:180px;overflow-y:auto;`;
  dd.className = "interp-dropdown";
  dd.innerHTML = "";

  for (const p of paths.slice(0, 12)) {
    const item   = document.createElement("div");
    item.className = "interp-item";
    const isAlias    = p.includes(" (");
    const insertValue = isAlias ? p.slice(0, p.indexOf(" (")) : p;
    const hint       = isAlias ? p.slice(p.indexOf(" (")) : "";
    item.innerHTML = `<span class="interp-path">${escHtml(insertValue)}</span>${hint ? `<span class="interp-hint">${escHtml(hint)}</span>` : ""}`;
    item.addEventListener("mousedown", (e) => { e.preventDefault(); onSelect(insertValue); });
    dd.appendChild(item);
  }
  dd.classList.remove("hidden");
}
