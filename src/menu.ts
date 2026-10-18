export type MenuVariant = "context" | "dropdown";

export interface MenuItem {
  label: string;
  /** Trusted SVG markup only (never user data). Context variant only. */
  icon?: string;
  /** CSS color for a leading dot. */
  swatch?: string;
  danger?: boolean;
  disabled?: boolean;
  /** Renders a leading check mark. */
  checked?: boolean;
  /** Marks the current choice with the existing `.active` style. */
  active?: boolean;
  className?: string;
  /** Replaces the default content; `label` still becomes the accessible name. */
  build?: (btn: HTMLButtonElement) => void;
  onSelect: () => void;
}

export type MenuEntry = MenuItem | "separator";

export interface MenuOptions {
  id: string;
  variant: MenuVariant;
  items: MenuEntry[];
  /** Element that gets focus back when the menu closes. */
  invoker: HTMLElement;
  /** Anchor below which the menu is placed; otherwise `at` is used. */
  anchor?: HTMLElement;
  at?: { x: number; y: number };
  /** With `anchor`: align the menu's right edge to the anchor's right edge. */
  alignEnd?: boolean;
  className?: string;
  orientation?: "vertical" | "horizontal";
}

export interface MenuHandle {
  el: HTMLElement;
  close: () => void;
}

const VIEWPORT_PAD = 8;

let openClose: (() => void) | null = null;

export function closeOpenMenu(): void {
  openClose?.();
}

function classes(variant: MenuVariant) {
  return variant === "context"
    ? { menu: "ctx-menu", item: "ctx-menu-item", danger: "ctx-menu-item--danger", sep: "ctx-menu-sep" }
    : { menu: "filter-dropdown", item: "filter-dropdown-item", danger: "filter-dropdown-item-danger", sep: "filter-dropdown-sep" };
}

function place(menu: HTMLElement, opts: MenuOptions): void {
  const w = menu.offsetWidth;
  const h = menu.offsetHeight;
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  let left: number;
  let top: number;
  if (opts.anchor) {
    const r = opts.anchor.getBoundingClientRect();
    left = opts.alignEnd ? r.right - w : r.left;
    top = r.bottom + 4;
    if (top + h > vh - VIEWPORT_PAD && r.top - 4 - h >= VIEWPORT_PAD) top = r.top - 4 - h;
  } else {
    left = opts.at?.x ?? 0;
    top = opts.at?.y ?? 0;
  }
  left = Math.max(VIEWPORT_PAD, Math.min(left, vw - w - VIEWPORT_PAD));
  top = Math.max(VIEWPORT_PAD, Math.min(top, vh - h - VIEWPORT_PAD));
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
}

export function openMenu(opts: MenuOptions): MenuHandle {
  closeOpenMenu();
  const cls = classes(opts.variant);
  const menu = document.createElement("div");
  menu.id = opts.id;
  menu.className = [cls.menu, opts.className].filter(Boolean).join(" ");
  menu.setAttribute("role", "menu");
  const horizontal = opts.orientation === "horizontal";
  if (horizontal) menu.setAttribute("aria-orientation", "horizontal");

  const buttons: HTMLButtonElement[] = [];
  let closed = false;

  const close = (restoreFocus = true): void => {
    if (closed) return;
    closed = true;
    if (openClose === closeThis) openClose = null;
    document.removeEventListener("mousedown", onOutside, true);
    menu.remove();
    if (opts.invoker instanceof HTMLButtonElement) opts.invoker.setAttribute("aria-expanded", "false");
    if (restoreFocus && opts.invoker.isConnected) opts.invoker.focus({ preventScroll: true });
  };
  const closeThis = (): void => close();

  const onOutside = (ev: MouseEvent): void => {
    if (menu.contains(ev.target as Node)) return;
    close(false);
  };

  const activate = (btn: HTMLButtonElement, item: MenuItem): void => {
    if (btn.getAttribute("aria-disabled") === "true") return;
    close();
    item.onSelect();
  };

  for (const entry of opts.items) {
    if (entry === "separator") {
      const sep = document.createElement("div");
      sep.className = cls.sep;
      sep.setAttribute("role", "separator");
      menu.appendChild(sep);
      continue;
    }
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = [cls.item, entry.danger ? cls.danger : "", entry.active ? "active" : "", entry.className ?? ""]
      .filter(Boolean).join(" ");
    btn.setAttribute("role", "menuitem");
    btn.tabIndex = -1;
    btn.setAttribute("aria-label", entry.label);
    if (entry.disabled) btn.setAttribute("aria-disabled", "true");
    if (entry.build) {
      entry.build(btn);
    } else {
      if (entry.swatch) {
        const dot = document.createElement("span");
        dot.className = "workflow-collection-swatch-inline";
        dot.style.background = entry.swatch;
        btn.appendChild(dot);
      }
      if (entry.icon) {
        const ic = document.createElement("span");
        ic.className = "ctx-menu-icon";
        ic.innerHTML = entry.icon;
        btn.appendChild(ic);
      }
      const text = document.createElement("span");
      text.textContent = entry.label;
      btn.appendChild(text);
      if (entry.checked) {
        const tick = document.createElement("span");
        tick.className = "menu-item-check";
        tick.setAttribute("aria-hidden", "true");
        tick.textContent = "✓";
        btn.appendChild(tick);
      }
    }
    btn.addEventListener("click", ev => { ev.preventDefault(); activate(btn, entry); });
    buttons.push(btn);
    menu.appendChild(btn);
  }

  const focusAt = (i: number): void => {
    if (!buttons.length) return;
    buttons[(i + buttons.length) % buttons.length].focus({ preventScroll: true });
  };

  menu.addEventListener("keydown", (e: KeyboardEvent) => {
    const idx = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const stop = (): void => { e.preventDefault(); e.stopPropagation(); };
    if (e.key === "ArrowDown" || (horizontal && e.key === "ArrowRight")) { stop(); focusAt(idx + 1); }
    else if (e.key === "ArrowUp" || (horizontal && e.key === "ArrowLeft")) { stop(); focusAt(idx < 0 ? -1 : idx - 1); }
    else if (e.key === "Home") { stop(); focusAt(0); }
    else if (e.key === "End") { stop(); focusAt(buttons.length - 1); }
    else if (e.key === "Escape") { stop(); close(); }
    else if (e.key === "Tab") { close(); }
    else if ((e.key === "Enter" || e.key === " ") && idx >= 0) {
      stop();
      const item = opts.items.filter((x): x is MenuItem => x !== "separator")[idx];
      activate(buttons[idx], item);
    }
  });

  document.body.appendChild(menu);
  place(menu, opts);
  if (opts.invoker instanceof HTMLButtonElement) {
    opts.invoker.setAttribute("aria-haspopup", "menu");
    opts.invoker.setAttribute("aria-expanded", "true");
  }
  openClose = closeThis;
  setTimeout(() => { if (!closed) document.addEventListener("mousedown", onOutside, true); }, 0);
  const firstEnabled = buttons.findIndex(b => b.getAttribute("aria-disabled") !== "true");
  focusAt(firstEnabled < 0 ? 0 : firstEnabled);

  return { el: menu, close: closeThis };
}
