import type { Canvas } from "./Canvas";
import { CanvasNode, NODE_WIDTH, NODE_HEADER } from "./Node";

export class ContextMenu {
  private canvas: Canvas;

  constructor(canvas: Canvas) {
    this.canvas = canvas;
  }

  show(cx: number, cy: number, node: CanvasNode): void {
    document.getElementById("canvas-ctx-menu")?.remove();

    const c = this.canvas;
    const isMultiSelect = c.selectedNodes.size > 1;

    const menu = document.createElement("div");
    menu.id = "canvas-ctx-menu";
    menu.className = "ctx-menu";
    menu.setAttribute("role", "menu");

    let dismiss: (ev: MouseEvent) => void;
    const closeMenu = () => {
      menu.remove();
      document.removeEventListener("mousedown", dismiss, true);
    };

    const addItem = (label: string, icon: string, danger: boolean, action: () => void) => {
      const item = document.createElement("button");
      item.className = "ctx-menu-item" + (danger ? " ctx-menu-item--danger" : "");
      item.setAttribute("role", "menuitem");
      item.innerHTML = `<span class="ctx-menu-icon">${icon}</span><span>${label}</span>`;
      item.addEventListener("mousedown", (e) => { e.preventDefault(); closeMenu(); action(); });
      menu.appendChild(item);
    };

    const addSep = () => {
      const s = document.createElement("div");
      s.className = "ctx-menu-sep";
      menu.appendChild(s);
    };

    addItem(
      isMultiSelect ? `Duplicate (${c.selectedNodes.size})` : "Duplicate",
      `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>`,
      false,
      () => { c.dupSelected(); c.onCanvasChanged?.(); }
    );

    if (!isMultiSelect) {
      addItem(
        "Rename",
        `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>`,
        false,
        () => this.startInlineRename(node)
      );
    }

    addItem(
      node.disabled ? "Enable" : "Disable",
      node.disabled
        ? `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="20 6 9 17 4 12"/></svg>`
        : `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`,
      false,
      () => {
        const ids = isMultiSelect ? [...c.selectedNodes] : [node.data.id];
        for (const id of ids) {
          const n = c.nodes.get(id);
          if (n) n.disabled = !n.disabled;
        }
        c.onCanvasChanged?.();
      }
    );

    if (!isMultiSelect) {
      addItem(
        "Run from here",
        `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg>`,
        false,
        () => { c.onRunNode?.(node.data.id); }
      );
    }

    addSep();

    addItem(
      isMultiSelect ? `Delete (${c.selectedNodes.size})` : "Delete",
      `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/><path d="M9 6V4h6v2"/></svg>`,
      true,
      () => { c.deleteSelected(); }
    );

    document.body.appendChild(menu);
    const mw = menu.offsetWidth || 180;
    const mh = menu.offsetHeight || 160;
    const left = cx + mw > window.innerWidth  - 8 ? cx - mw : cx;
    const top  = cy + mh > window.innerHeight - 8 ? cy - mh : cy;
    menu.style.left = `${left}px`;
    menu.style.top  = `${top}px`;

    dismiss = (ev: MouseEvent) => {
      if (!menu.contains(ev.target as Node)) {
        closeMenu();
      }
    };
    setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);
  }

  startInlineRename(node: CanvasNode): void {
    document.getElementById("canvas-rename-input")?.remove();

    const c = this.canvas;
    const r  = c.el.getBoundingClientRect();
    const sx = node.data.position.x * c.zoom + c.panX + r.left;
    const sy = node.data.position.y * c.zoom + c.panY + r.top;
    const sw = NODE_WIDTH  * c.zoom;
    const sh = NODE_HEADER * c.zoom;

    const inp = document.createElement("input");
    inp.id = "canvas-rename-input";
    inp.type = "text";
    inp.value = node.data.name;
    inp.className = "canvas-rename-input";
    inp.style.left = `${sx + 30 * c.zoom}px`;
    inp.style.top = `${sy + sh / 2 - 11}px`;
    inp.style.width = `${Math.max(sw - 50 * c.zoom, 80)}px`;
    document.body.appendChild(inp);
    inp.focus(); inp.select();

    let settled = false;
    let dismiss: (ev: MouseEvent) => void;
    const commit = () => {
      if (settled) return;
      settled = true;
      document.removeEventListener("mousedown", dismiss, true);
      const v = inp.value.trim();
      if (v) { node.data.name = v; c.onCanvasChanged?.(); }
      inp.remove();
    };
    dismiss = (ev: MouseEvent) => { if (ev.target !== inp) commit(); };
    setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);
    inp.addEventListener("blur", commit);
    inp.addEventListener("keydown", (e) => {
      if (e.key === "Enter")  { e.preventDefault(); commit(); }
      if (e.key === "Escape") {
        settled = true;
        document.removeEventListener("mousedown", dismiss, true);
        inp.remove();
      }
    });
  }
}
