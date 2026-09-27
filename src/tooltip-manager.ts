// Tooltip manager — WCAG 1.4.13 compliant
// Replaces the CSS-only ::after tooltip with a JS-driven div[role="tooltip"].
// Criteria satisfied:
//   1. Dismissible  — Escape hides without moving focus
//   2. Hoverable    — real DOM element; pointer can move over it
//   3. Persistent   — no auto-dismiss timer while pointer/focus is on trigger

const TT_ID = "aria-tt";
const SHOW_DELAY_MS = 500;
let activeEl:  HTMLElement | null = null;
let hideTimer: ReturnType<typeof setTimeout> | null = null;
let showTimer: ReturnType<typeof setTimeout> | null = null;
let escBound = false;
const bound = new WeakSet<HTMLElement>();

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

function cancelShow(): void {
  if (showTimer !== null) {
    clearTimeout(showTimer);
    showTimer = null;
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
  if (bound.has(el)) return;
  bound.add(el);
  // Several call sites also set the native `title` attribute (pre-dating
  // this system, or copy-pasted from it) — that pops the browser's own
  // OS-styled tooltip at the same time as this one, showing two stacked
  // boxes for one trigger. This tooltip fully replaces `title`, so any
  // leftover is dropped here once, centrally, rather than trusting every
  // call site (current or future) to never set it.
  if (el.hasAttribute("title")) el.removeAttribute("title");
  el.addEventListener("mouseenter", () => {
    cancelHide();
    cancelShow();
    showTimer = setTimeout(() => { showTimer = null; showFor(el); }, SHOW_DELAY_MS);
  });
  el.addEventListener("mouseleave", () => { cancelShow(); scheduleHide(); });
  el.addEventListener("focus",      () => { cancelShow(); cancelHide(); showFor(el); });
  el.addEventListener("blur",       () => scheduleHide());
}

export function initTooltips(): void {
  document.querySelectorAll<HTMLElement>("[data-tooltip]").forEach(bindTrigger);

  if (!escBound) {
    escBound = true;
    document.addEventListener("keydown", (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      cancelShow();
      if (activeEl) scheduleHide(true);
    });

    // The sweep above only runs once, at boot. A [data-tooltip] element
    // added afterward — lazy panels, popover forms, anything rendered
    // post-boot — would otherwise stay unbound forever. Watching here,
    // once, centrally, means a dynamic-render call site never has to
    // remember to re-invoke tooltip binding itself.
    new MutationObserver(mutations => {
      // Guards against a callback firing after its window/realm is gone
      // (e.g. jsdom teardown between test files scheduling one last
      // observer tick) — nothing useful to do for a torn-down document
      // anyway. Checked once per invocation, not per node.
      if (typeof HTMLElement === "undefined") return;
      for (const m of mutations) {
        if (m.type === "attributes" && m.target instanceof HTMLElement) {
          bindTrigger(m.target);
          continue;
        }
        m.addedNodes.forEach(node => {
          if (!(node instanceof HTMLElement)) return;
          if (node.hasAttribute("data-tooltip")) bindTrigger(node);
          node.querySelectorAll<HTMLElement>("[data-tooltip]").forEach(bindTrigger);
        });
        // A trigger removed while its tooltip is showing (e.g. a popover
        // closing under a still-hovered/focused button) would otherwise
        // orphan the tooltip on screen forever — nothing left in the DOM to
        // fire the mouseleave/blur that normally hides it. Catching removal
        // here, centrally, covers every call site; hideTooltipFor (below)
        // stays for the narrower case where the trigger survives but its
        // data-tooltip attribute doesn't.
        m.removedNodes.forEach(node => {
          if (!(node instanceof HTMLElement) || !activeEl) return;
          if (node === activeEl || node.contains(activeEl)) doHide();
        });
      }
    }).observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["data-tooltip"],
    });
  }
}

// Called by always-on.ts when data-tooltip is removed from an element while
// a tooltip might already be showing for it.
export function hideTooltipFor(el: HTMLElement): void {
  if (activeEl === el) doHide();
}
