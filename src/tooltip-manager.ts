// Tooltip manager — WCAG 1.4.13 compliant
// Replaces the CSS-only ::after tooltip with a JS-driven div[role="tooltip"].
// Criteria satisfied:
//   1. Dismissible  — Escape hides without moving focus
//   2. Hoverable    — real DOM element; pointer can move over it
//   3. Persistent   — no auto-dismiss timer while pointer/focus is on trigger

const TT_ID = "aria-tt";
let activeEl:  HTMLElement | null = null;
let hideTimer: ReturnType<typeof setTimeout> | null = null;
let escBound = false;

function getOrCreate(): HTMLElement {
  let el = document.getElementById(TT_ID);
  if (!el) {
    el = document.createElement("div");
    el.id = TT_ID;
    el.setAttribute("role", "tooltip");
    el.className = "aria-tooltip";
    el.hidden = true;
    el.addEventListener("mouseenter", cancelHide);
    el.addEventListener("mouseleave", () => scheduleHide());
    document.body.appendChild(el);
  }
  return el;
}

function cancelHide(): void {
  if (hideTimer !== null) {
    clearTimeout(hideTimer);
    hideTimer = null;
  }
}

function scheduleHide(immediate = false): void {
  cancelHide();
  hideTimer = setTimeout(doHide, immediate ? 0 : 150);
}

function doHide(): void {
  const el = document.getElementById(TT_ID);
  if (el) el.hidden = true;
  if (activeEl) {
    activeEl.removeAttribute("aria-describedby");
    activeEl = null;
  }
}

function positionTooltip(tt: HTMLElement, trigger: HTMLElement, side: string): void {
  const r = trigger.getBoundingClientRect();
  tt.style.top       = `${r.top + r.height / 2}px`;
  tt.style.transform = "translateY(-50%)";
  if (side === "left") {
    tt.style.left  = "";
    tt.style.right = `${window.innerWidth - r.left + 8}px`;
  } else {
    tt.style.right = "";
    tt.style.left  = `${r.right + 8}px`;
  }
}

function showFor(trigger: HTMLElement): void {
  const text = trigger.getAttribute("data-tooltip");
  if (!text) return;
  const side = trigger.getAttribute("data-tooltip-side") ?? "right";
  const tt   = getOrCreate();
  tt.textContent = text;
  tt.hidden = false;
  trigger.setAttribute("aria-describedby", TT_ID);
  activeEl = trigger;
  positionTooltip(tt, trigger, side);
}

function bindTrigger(el: HTMLElement): void {
  el.addEventListener("mouseenter", () => { cancelHide(); showFor(el); });
  el.addEventListener("mouseleave", () => scheduleHide());
  el.addEventListener("focus",      () => { cancelHide(); showFor(el); });
  el.addEventListener("blur",       () => scheduleHide());
}

export function initTooltips(): void {
  document.querySelectorAll<HTMLElement>("[data-tooltip]").forEach(bindTrigger);

  if (!escBound) {
    escBound = true;
    document.addEventListener("keydown", (e: KeyboardEvent) => {
      if (e.key === "Escape" && activeEl) scheduleHide(true);
    });
  }
}

// Called by always-on.ts when data-tooltip is removed from an element while
// a tooltip might already be showing for it.
export function hideTooltipFor(el: HTMLElement): void {
  if (activeEl === el) doHide();
}
