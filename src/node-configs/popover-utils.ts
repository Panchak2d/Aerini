import type { CanvasNode } from "../canvas/Node";

// ── Extension API ─────────────────────────────────────────────────────────────

export interface ExtensionContext {
  node:     CanvasNode;
  body:     HTMLElement;
  canvasEl: HTMLCanvasElement;
  onChange: () => void;
  creds:    Array<{ id: string; name: string }>;
  /** Re-opens the popover for this node — used when a field change alters which
   *  other fields should be visible (e.g. schedule mode selector). */
  rerender: () => void;
}

export interface NodeConfigExtension {
  /** Rendered inside "Configuration" section, before the generic field loop. */
  beforeFields?: (ctx: ExtensionContext) => void;
  /** When true, the generic cfgKeys loop is skipped entirely.
   *  The beforeFields hook is responsible for all field rendering. */
  replaceGenericFields?: boolean;
  /** Rendered after the "Configuration" section, before "Connection". */
  afterFields?: (ctx: ExtensionContext) => void;
  /** When true, the "Connection" section appears even without a schema credential key. */
  forceCredSection?: boolean;
  /** Rendered inside "Connection" section, before the generic credential picker. */
  credentialsHeader?: (ctx: ExtensionContext) => void;
  /** Rendered after the "Reliability" section (end of body). */
  afterReliability?: (ctx: ExtensionContext) => void;
}

// ── DOM helpers ───────────────────────────────────────────────────────────────

export function mk<T extends HTMLElement>(tag: string): T {
  return document.createElement(tag) as T;
}

export function mkSection(title: string): HTMLElement {
  const d = document.createElement("div");
  d.className = "popover-section-title";
  d.textContent = title;
  return d;
}

export function mkField(label: string, factory: () => HTMLElement, hint?: string): HTMLElement {
  const g = document.createElement("div");
  g.className = "field-group";
  const l = document.createElement("label");
  l.className = "field-label";
  l.textContent = label;
  g.appendChild(l);
  g.appendChild(factory());
  if (hint) {
    const h = document.createElement("div");
    h.className = "field-hint";
    h.textContent = hint;
    g.appendChild(h);
  }
  return g;
}

export const CRON_PRESETS = [
  { label: "Every minute",       value: "* * * * *" },
  { label: "Every hour",         value: "0 * * * *" },
  { label: "Daily at 9 am",      value: "0 9 * * *" },
  { label: "Weekdays at 9 am",   value: "0 9 * * 1-5" },
  { label: "Weekly (Mon 9 am)",  value: "0 9 * * 1" },
  { label: "Monthly (1st 9 am)", value: "0 9 1 * *" },
];

// ── Custom dropdown ───────────────────────────────────────────────────────────
// WebKitGTK on Linux renders <select> inside position:fixed elements inline
// (shows all options as a list) rather than as a native popup. This custom
// implementation avoids that entirely.

export function mkCustomSelect(
  options: string[],
  current: string,
  onChange: (value: string) => void,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "csel-wrap";

  const trigger = document.createElement("button");
  trigger.type = "button";
  trigger.className = "csel-trigger";

  const labelEl = document.createElement("span");
  labelEl.className = "csel-label";
  labelEl.textContent = current || options[0] || "";

  const arrow = document.createElement("span");
  arrow.className = "csel-arrow";
  arrow.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><polyline points="6 9 12 15 18 9"/></svg>`;

  trigger.appendChild(labelEl);
  trigger.appendChild(arrow);
  wrap.appendChild(trigger);

  const dropdown = document.createElement("div");
  dropdown.className = "csel-dropdown hidden";

  for (const opt of options) {
    const item = document.createElement("div");
    item.className = "csel-option";
    if (opt === current) item.classList.add("selected");
    item.textContent = opt;
    item.addEventListener("mousedown", (e) => {
      e.preventDefault();
      e.stopPropagation();
      labelEl.textContent = opt;
      dropdown.querySelectorAll(".csel-option").forEach(el => el.classList.remove("selected"));
      item.classList.add("selected");
      onChange(opt);
      closeDropdown();
    });
    dropdown.appendChild(item);
  }

  wrap.appendChild(dropdown);

  let isOpen = false;

  const openDropdown = () => {
    if (isOpen) return;
    isOpen = true;
    dropdown.classList.remove("hidden");
    trigger.classList.add("open");
    const tr = trigger.getBoundingClientRect();
    const dropH = Math.min(options.length * 33 + 8, 200);
    const spaceBelow = window.innerHeight - tr.bottom - 6;
    if (spaceBelow >= dropH || spaceBelow > tr.top) {
      dropdown.style.top  = `${tr.bottom + 4}px`;
    } else {
      dropdown.style.top  = `${tr.top - dropH - 4}px`;
    }
    dropdown.style.left  = `${tr.left}px`;
    dropdown.style.width = `${tr.width}px`;
    const onOutside = (e: MouseEvent) => {
      if (!wrap.contains(e.target as Node)) {
        closeDropdown();
        document.removeEventListener("mousedown", onOutside, true);
      }
    };
    setTimeout(() => document.addEventListener("mousedown", onOutside, true), 0);
  };

  const closeDropdown = () => {
    if (!isOpen) return;
    isOpen = false;
    dropdown.classList.add("hidden");
    trigger.classList.remove("open");
  };

  trigger.addEventListener("click", (e) => {
    e.stopPropagation();
    isOpen ? closeDropdown() : openDropdown();
  });

  trigger.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); isOpen ? closeDropdown() : openDropdown(); }
    if (e.key === "Escape") closeDropdown();
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!isOpen) openDropdown();
      const items = [...dropdown.querySelectorAll<HTMLElement>(".csel-option")];
      const cur   = dropdown.querySelector<HTMLElement>(".csel-option.selected");
      const idx   = cur ? items.indexOf(cur) : -1;
      const next  = e.key === "ArrowDown" ? items[idx + 1] : items[idx - 1];
      if (next) {
        next.classList.add("selected");
        cur?.classList.remove("selected");
        next.scrollIntoView({ block: "nearest" });
      }
    }
  });

  return wrap;
}
