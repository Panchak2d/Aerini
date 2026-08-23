import type { CanvasNode } from "../canvas/Node";

// ── Extension API ─────────────────────────────────────────────────────────────

export interface ExtensionContext {
  node:     CanvasNode;
  body:     HTMLElement;
  canvasEl: HTMLCanvasElement;
  onChange: () => void;
  creds:    Array<{ id: string; name: string; cred_type: string }>;
  /** Re-opens the popover for this node — used when a field change alters which
   *  other fields should be visible (e.g. schedule mode selector). */
  rerender: () => void;
  /** True once the generic "Configuration" section header has already been
   *  rendered for this node. Lets an `afterFields` hook (e.g. plugin-config's
   *  generic JSON editor) know whether it still needs to render its own
   *  section header. Optional — most extensions don't need it, and two
   *  existing tests construct `ExtensionContext` object literals directly
   *  without it; a missing value reads as falsy, same as `false`. */
  hasConfigSection?: boolean;
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

let _fieldIdCounter = 0;

export function mkField(label: string, factory: () => HTMLElement, hint?: string, required?: boolean): HTMLElement {
  const fieldId = `nf-${++_fieldIdCounter}`;

  const g = document.createElement("div");
  g.className = "field-group";

  const l = document.createElement("label");
  l.className = "field-label";
  l.htmlFor = fieldId;
  l.textContent = label;
  if (required) {
    const star = document.createElement("span");
    star.className = "req-star";
    star.textContent = " *";
    star.setAttribute("aria-hidden", "true");
    l.appendChild(star);
    l.setAttribute("aria-label", `${label} (required)`);
  }
  g.appendChild(l);

  const control = factory();

  // Connect the label to the first focusable control inside the returned element.
  // This makes clicking the label focus/activate the input, including checkboxes.
  const focusable = (control.matches("input,select,textarea")
    ? control
    : control.querySelector<HTMLElement>("input,select,textarea"));
  if (focusable) focusable.id = fieldId;

  g.appendChild(control);

  if (hint) {
    const h = document.createElement("div");
    h.className = "field-hint";
    h.id = `${fieldId}-hint`;
    h.textContent = hint;
    // Let the input announce the hint via aria-describedby.
    if (focusable) focusable.setAttribute("aria-describedby", `${fieldId}-hint`);
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

let _cselIdCounter = 0;

// previously each `openDropdown` call added its own
// `document`-level "mousedown" listener, removed only if that exact
// dropdown's own outside-click handler happened to fire. Any other close
// path (Escape, the popover being torn down, a different node's popover
// opening) left that listener attached to `document` forever, referencing
// a detached `wrap`. Fix: one shared, capture-phase listener bound once
// for the whole module, backed by a live registry of currently-open
// instances — closing an entry (however it closes) removes it from the
// registry; a `wrap` that got disconnected from the DOM without going
// through `closeDropdown()` at all is swept on the very next mousedown
// anywhere, instead of leaking for the rest of the app session.
interface OpenDropdownEntry { wrap: HTMLElement; close: () => void; }
const openDropdownRegistry = new Set<OpenDropdownEntry>();
let delegatedOutsideClickBound = false;

function ensureDelegatedOutsideClickListener(): void {
  if (delegatedOutsideClickBound) return;
  delegatedOutsideClickBound = true;
  document.addEventListener("mousedown", (e) => {
    for (const entry of Array.from(openDropdownRegistry)) {
      if (!entry.wrap.isConnected || !entry.wrap.contains(e.target as Node)) {
        entry.close();
      }
    }
  }, true);
}

export function mkCustomSelect(
  options: string[],
  current: string,
  onChange: (value: string) => void,
): HTMLElement {
  const id = `csel-${++_cselIdCounter}`;
  const listId = `${id}-list`;

  const wrap = document.createElement("div");
  wrap.className = "csel-wrap";

  const trigger = document.createElement("button");
  trigger.type = "button";
  trigger.className = "csel-trigger";
  trigger.setAttribute("role", "combobox");
  trigger.setAttribute("aria-haspopup", "listbox");
  trigger.setAttribute("aria-expanded", "false");
  trigger.setAttribute("aria-controls", listId);

  const labelEl = document.createElement("span");
  labelEl.className = "csel-label";
  labelEl.textContent = current || options[0] || "";

  const arrow = document.createElement("span");
  arrow.className = "csel-arrow";
  arrow.setAttribute("aria-hidden", "true");
  arrow.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="6 9 12 15 18 9"/></svg>`;

  trigger.appendChild(labelEl);
  trigger.appendChild(arrow);
  wrap.appendChild(trigger);

  const dropdown = document.createElement("div");
  dropdown.className = "csel-dropdown hidden";
  dropdown.id = listId;
  dropdown.setAttribute("role", "listbox");

  let activeIdx = options.indexOf(current);
  if (activeIdx < 0) activeIdx = 0;

  const optionEls: HTMLElement[] = [];

  for (let i = 0; i < options.length; i++) {
    const opt = options[i];
    const item = document.createElement("div");
    item.className = "csel-option";
    item.id = `${id}-opt-${i}`;
    item.setAttribute("role", "option");
    item.setAttribute("aria-selected", opt === current ? "true" : "false");
    if (opt === current) item.classList.add("selected");
    item.textContent = opt;

    item.addEventListener("mousedown", (e) => {
      e.preventDefault();
      e.stopPropagation();
      commitSelection(i);
      closeDropdown();
      trigger.focus();
    });

    dropdown.appendChild(item);
    optionEls.push(item);
  }

  wrap.appendChild(dropdown);

  let isOpen = false;
  const dropdownEntry: OpenDropdownEntry = { wrap, close: () => closeDropdown() };

  const commitSelection = (idx: number) => {
    const opt = options[idx];
    if (!opt) return;
    labelEl.textContent = opt;
    optionEls.forEach((el, i) => {
      el.classList.toggle("selected", i === idx);
      el.setAttribute("aria-selected", i === idx ? "true" : "false");
    });
    activeIdx = idx;
    trigger.setAttribute("aria-activedescendant", optionEls[idx].id);
    onChange(opt);
  };

  const openDropdown = () => {
    if (isOpen) return;
    isOpen = true;
    dropdown.classList.remove("hidden");
    trigger.classList.add("open");
    trigger.setAttribute("aria-expanded", "true");

    const tr = trigger.getBoundingClientRect();
    const dropH = Math.min(options.length * 33 + 8, 200);
    const spaceBelow = window.innerHeight - tr.bottom - 6;
    dropdown.style.top  = (spaceBelow >= dropH || spaceBelow > tr.top)
      ? `${tr.bottom + 4}px`
      : `${tr.top - dropH - 4}px`;
    dropdown.style.left  = `${tr.left}px`;
    dropdown.style.width = `${tr.width}px`;

    // Scroll active option into view.
    optionEls[activeIdx]?.scrollIntoView({ block: "nearest" });
    trigger.setAttribute("aria-activedescendant", optionEls[activeIdx]?.id ?? "");

    openDropdownRegistry.add(dropdownEntry);
    ensureDelegatedOutsideClickListener();
  };

  const closeDropdown = () => {
    if (!isOpen) return;
    isOpen = false;
    dropdown.classList.add("hidden");
    trigger.classList.remove("open");
    trigger.setAttribute("aria-expanded", "false");
    trigger.removeAttribute("aria-activedescendant");
    openDropdownRegistry.delete(dropdownEntry);
  };

  trigger.addEventListener("click", (e) => {
    e.stopPropagation();
    isOpen ? closeDropdown() : openDropdown();
  });

  trigger.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (isOpen) {
        commitSelection(activeIdx);
        closeDropdown();
      } else {
        openDropdown();
      }
    }
    if (e.key === "Escape") { closeDropdown(); }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!isOpen) { openDropdown(); return; }
      const next = e.key === "ArrowDown"
        ? Math.min(activeIdx + 1, options.length - 1)
        : Math.max(activeIdx - 1, 0);
      if (next !== activeIdx) {
        // Immediately commit on arrow key — matches native <select> behaviour.
        commitSelection(next);
        optionEls[next]?.scrollIntoView({ block: "nearest" });
      }
    }
  });

  return wrap;
}
