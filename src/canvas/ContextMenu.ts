import type { Canvas } from "./Canvas";
import type { Connector } from "./Connector";
import { CanvasNode, NODE_WIDTH, NODE_HEADER } from "./Node";
import { buildCanvasMenu, buildSelectionMenu, buildWireMenu, type MenuApi } from "./context-menu-items";

interface MountOptions {
  label: string;
  /** Focus the first row on open (keyboard); otherwise the menu itself takes focus so the arrow keys work. */
  focusFirst?: boolean;
}

/** Ignored for this long after opening, so trailing inertial scroll from the gesture that led here does not close the menu at once. */
const WHEEL_GRACE_MS = 200;
const EDGE_GAP = 8;

export class ContextMenu {
  private canvas: Canvas;
  private closeActive: ((restoreFocus: boolean) => void) | null = null;

  constructor(canvas: Canvas) {
    this.canvas = canvas;
  }

  private mount(cx: number, cy: number, fill: (api: MenuApi) => void, opts: MountOptions): void {
    this.closeActive?.(false);
    document.getElementById("canvas-ctx-menu")?.remove();

    const menu = document.createElement("div");
    menu.id = "canvas-ctx-menu";
    menu.className = "ctx-menu";
    menu.tabIndex = -1;
    menu.setAttribute("role", "menu");
    menu.setAttribute("aria-label", opts.label);

    const actions = new Map<HTMLElement, () => void>();
    const stops: HTMLElement[] = [];
    let closed = false;
    let used = false;
    const openedAt = performance.now();

    const dismiss = (ev: MouseEvent) => { if (!menu.contains(ev.target as Node)) closeMenu(false); };
    const onWheel = (ev: WheelEvent) => {
      if (menu.contains(ev.target as Node) || performance.now() - openedAt < WHEEL_GRACE_MS) return;
      closeMenu(false);
    };
    const onWindowChange = () => closeMenu(false);

    const closeMenu = (restoreFocus: boolean) => {
      if (closed) return;
      closed = true;
      menu.remove();
      document.removeEventListener("mousedown", dismiss, true);
      document.removeEventListener("wheel", onWheel, true);
      window.removeEventListener("blur", onWindowChange);
      window.removeEventListener("resize", onWindowChange);
      if (this.closeActive === closeMenu) this.closeActive = null;
      if (restoreFocus) this.canvas.el.focus({ preventScroll: true });
    };

    const register = (el: HTMLElement, action: () => void) => {
      const run = () => {
        if (used) return;
        used = true;
        closeMenu(true);
        action();
      };
      actions.set(el, run);
      stops.push(el);
      el.tabIndex = -1;
      el.addEventListener("mousedown", (e) => { e.preventDefault(); run(); });
      el.addEventListener("click", (e) => { if (e.detail === 0) run(); });
    };

    fill({
      item: (label, icon, danger, action) => {
        const item = document.createElement("button");
        item.type = "button";
        item.className = "ctx-menu-item" + (danger ? " ctx-menu-item--danger" : "");
        item.setAttribute("role", "menuitem");
        const iconEl = document.createElement("span");
        iconEl.className = "ctx-menu-icon";
        iconEl.innerHTML = icon;
        const text = document.createElement("span");
        text.textContent = label;
        item.append(iconEl, text);
        register(item, action);
        menu.appendChild(item);
      },
      sep: () => {
        const sep = document.createElement("div");
        sep.className = "ctx-menu-sep";
        sep.setAttribute("role", "separator");
        menu.appendChild(sep);
      },
      swatches: (label, options, current, pick) => {
        const row = document.createElement("div");
        row.className = "ctx-menu-swatches";
        row.setAttribute("role", "group");
        row.setAttribute("aria-label", label);
        const title = document.createElement("span");
        title.className = "ctx-menu-swatch-label";
        title.textContent = label;
        row.appendChild(title);
        for (const o of options) {
          const b = document.createElement("button");
          b.type = "button";
          b.className = "ctx-menu-swatch" + (o.id === current ? " active" : "");
          b.style.background = o.css;
          b.title = o.label;
          b.setAttribute("role", "menuitemradio");
          b.setAttribute("aria-checked", String(o.id === current));
          b.setAttribute("aria-label", `${o.label} ${label.toLowerCase()}`);
          b.dataset.swatch = "1";
          register(b, () => pick(o.id));
          row.appendChild(b);
        }
        menu.appendChild(row);
      },
      choices: (label, options, current, pick) => {
        const row = document.createElement("div");
        row.className = "ctx-menu-swatches";
        row.setAttribute("role", "group");
        row.setAttribute("aria-label", label);
        const title = document.createElement("span");
        title.className = "ctx-menu-swatch-label";
        title.textContent = label;
        row.appendChild(title);
        for (const o of options) {
          const b = document.createElement("button");
          b.type = "button";
          b.className = "ctx-menu-choice" + (o.id === current ? " active" : "");
          b.textContent = o.label;
          b.title = o.title;
          b.setAttribute("role", "menuitemradio");
          b.setAttribute("aria-checked", String(o.id === current));
          b.setAttribute("aria-label", `${o.title} ${label.toLowerCase()}`);
          b.dataset.swatch = "1";
          register(b, () => pick(o.id));
          row.appendChild(b);
        }
        menu.appendChild(row);
      },
    });

    menu.addEventListener("keydown", (e) => {
      e.stopPropagation();
      const at = stops.indexOf(document.activeElement as HTMLElement);
      const focusAt = (i: number) => stops[(i + stops.length) % stops.length]?.focus();
      switch (e.key) {
        case "ArrowDown": e.preventDefault(); focusAt(at < 0 ? 0 : at + 1); break;
        case "ArrowUp":   e.preventDefault(); focusAt(at < 0 ? stops.length - 1 : at - 1); break;
        case "Home":      e.preventDefault(); focusAt(0); break;
        case "End":       e.preventDefault(); focusAt(stops.length - 1); break;
        case "ArrowRight":
        case "ArrowLeft": {
          const cur = stops[at];
          if (!cur?.dataset["swatch"]) break;
          e.preventDefault();
          const next = stops[at + (e.key === "ArrowRight" ? 1 : -1)];
          if (next?.dataset["swatch"]) next.focus();
          break;
        }
        case "Enter":
        case " ": {
          const run = actions.get(document.activeElement as HTMLElement);
          if (run) { e.preventDefault(); run(); }
          break;
        }
        case "Escape":
        case "Tab":
          e.preventDefault();
          closeMenu(true);
          break;
        default:
          break;
      }
    });
    menu.addEventListener("focusout", (e) => {
      const to = e.relatedTarget;
      if (to instanceof Node && !menu.contains(to)) closeMenu(false);
    });

    document.body.appendChild(menu);
    const mw = menu.offsetWidth || 180;
    const mh = menu.offsetHeight || 160;
    const left = cx + mw > window.innerWidth  - EDGE_GAP ? cx - mw : cx;
    const top  = cy + mh > window.innerHeight - EDGE_GAP ? cy - mh : cy;
    menu.style.left = `${Math.max(EDGE_GAP, Math.min(left, window.innerWidth  - mw - EDGE_GAP))}px`;
    menu.style.top  = `${Math.max(EDGE_GAP, Math.min(top,  window.innerHeight - mh - EDGE_GAP))}px`;

    this.closeActive = closeMenu;
    setTimeout(() => {
      if (closed) return;
      document.addEventListener("mousedown", dismiss, true);
      document.addEventListener("wheel", onWheel, { capture: true, passive: true });
    }, 0);
    window.addEventListener("blur", onWindowChange);
    window.addEventListener("resize", onWindowChange);

    (opts.focusFirst ? stops[0] ?? menu : menu).focus({ preventScroll: true });
  }

  showCanvas(cx: number, cy: number, world: { x: number; y: number }, focusFirst = false): void {
    this.mount(cx, cy, (api) => buildCanvasMenu(api, this.canvas, world), { label: "Canvas actions", focusFirst });
  }

  showWire(cx: number, cy: number, conn: Connector, world: { x: number; y: number }, focusFirst = false): void {
    this.mount(cx, cy, (api) => buildWireMenu(api, this.canvas, conn, world), { label: "Connection actions", focusFirst });
  }

  show(cx: number, cy: number, node: CanvasNode, focusFirst = false): void {
    this.mount(
      cx, cy,
      (api) => buildSelectionMenu(api, this.canvas, node, { rename: (n) => this.startInlineRename(n) }),
      { label: node.isNote ? (node.noteStyle === "frame" ? "Group frame actions" : "Note actions") : "Node actions", focusFirst },
    );
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
      if (v && v !== node.data.name) {
        const res = c.renameNode(node, v);
        if (res.message) c.onWarn?.(res.message);
      }
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
