const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Tab stops inside `root`, in DOM order, skipping anything inside a `[hidden]` ancestor. */
export function focusableIn(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    el => !el.closest("[hidden]") && el.tabIndex >= 0,
  );
}

/**
 * Where focus must jump so Tab / Shift+Tab never leaves the dialog, or null
 * when the browser's default movement stays inside it.
 */
export function trapTarget(
  items: readonly HTMLElement[],
  active: Element | null,
  shift: boolean,
): HTMLElement | null {
  if (items.length === 0) return null;
  const first = items[0];
  const last = items[items.length - 1];
  if (!active || !items.includes(active as HTMLElement)) return shift ? last : first;
  if (shift && active === first) return last;
  if (!shift && active === last) return first;
  return null;
}

/**
 * Replaces a `<select data-segmented>` with a radio-style button group. The
 * select stays in the DOM, hidden, as the single source of truth: choosing a
 * segment sets its value and dispatches a bubbling `change`, so existing
 * listeners keep working. Returns a function that re-reads the select.
 */
function enhanceSegmented(select: HTMLSelectElement): () => void {
  const group = document.createElement("div");
  group.className = "settings-segmented";
  group.setAttribute("role", "radiogroup");
  const label = select.closest(".settings-row")?.querySelector(".settings-label")?.textContent;
  if (label) group.setAttribute("aria-label", label);

  const buttons = Array.from(select.options).map(opt => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "settings-segmented-item";
    btn.setAttribute("role", "radio");
    btn.dataset.value = opt.value;
    btn.textContent = opt.textContent;
    group.appendChild(btn);
    return btn;
  });

  const paint = (): void => {
    for (const btn of buttons) {
      const on = btn.dataset.value === select.value;
      btn.setAttribute("aria-checked", String(on));
      btn.tabIndex = on ? 0 : -1;
    }
  };

  const choose = (value: string): void => {
    if (select.value === value) return;
    select.value = value;
    select.dispatchEvent(new Event("change", { bubbles: true }));
    paint();
  };

  group.addEventListener("click", e => {
    const btn = (e.target as HTMLElement).closest<HTMLButtonElement>(".settings-segmented-item");
    const value = btn?.dataset.value;
    if (value !== undefined) choose(value);
  });

  group.addEventListener("keydown", e => {
    const step =
      e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 :
      e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
    if (step === 0) return;
    const current = buttons.findIndex(b => b.dataset.value === select.value);
    const next = buttons[(current + step + buttons.length) % buttons.length];
    e.preventDefault();
    choose(next.dataset.value ?? select.value);
    next.focus();
  });

  select.hidden = true;
  select.after(group);
  paint();
  return paint;
}

/**
 * Wires a settings dialog shell: section rail (with a select fallback on
 * narrow windows), one visible section at a time that resets scroll on
 * switch, segmented controls, a Tab focus trap while open, and focus return
 * to the opener on close. Open/close is observed from the modal's `hidden`
 * class, so every existing open and close path keeps working unchanged.
 */
export function bindSettingsModal(modalId: string, openerId: string): void {
  const modal = document.getElementById(modalId);
  const box = modal?.querySelector<HTMLElement>(".settings-box");
  if (!modal || !box) return;

  const layout = box.querySelector<HTMLElement>(".settings-layout");
  const body = box.querySelector<HTMLElement>(".settings-body");
  const railItems = Array.from(box.querySelectorAll<HTMLButtonElement>(".settings-rail-item"));
  const groups = Array.from(box.querySelectorAll<HTMLElement>(".settings-group[data-section]"));
  box.tabIndex = -1;

  const repaintSegmented = Array.from(
    box.querySelectorAll<HTMLSelectElement>("select[data-segmented]"),
  ).map(enhanceSegmented);

  let sectionSelect: HTMLSelectElement | null = null;
  if (layout && railItems.length > 0) {
    const select = document.createElement("select");
    select.className = "settings-rail-select";
    select.setAttribute("aria-label", "Settings section");
    for (const item of railItems) {
      const opt = document.createElement("option");
      opt.value = item.dataset.section ?? "";
      opt.textContent = item.textContent;
      select.appendChild(opt);
    }
    layout.insertBefore(select, layout.firstChild);
    select.addEventListener("change", () => showSection(select.value));
    sectionSelect = select;
  }

  const showSection = (id: string): void => {
    for (const group of groups) group.hidden = group.dataset.section !== id;
    for (const item of railItems) {
      if (item.dataset.section === id) item.setAttribute("aria-current", "true");
      else item.removeAttribute("aria-current");
    }
    if (sectionSelect) sectionSelect.value = id;
    if (body) body.scrollTop = 0;
  };

  const firstSection = railItems[0]?.dataset.section;
  if (firstSection) showSection(firstSection);

  for (const item of railItems) {
    item.addEventListener("click", () => {
      if (item.dataset.section) showSection(item.dataset.section);
    });
  }

  const onKeydown = (e: KeyboardEvent): void => {
    if (e.key !== "Tab") return;
    const target = trapTarget(focusableIn(box), document.activeElement, e.shiftKey);
    if (target) {
      e.preventDefault();
      target.focus();
    }
  };

  let opener: HTMLElement | null = null;
  let wasOpen = !modal.classList.contains("hidden");

  new MutationObserver(() => {
    const open = !modal.classList.contains("hidden");
    if (open === wasOpen) return;
    wasOpen = open;

    if (open) {
      const active = document.activeElement;
      opener = active instanceof HTMLElement && active !== document.body
        ? active
        : document.getElementById(openerId);
      modal.addEventListener("keydown", onKeydown);
      for (const repaint of repaintSegmented) repaint();
      if (firstSection) showSection(firstSection);
      (box.querySelector<HTMLElement>('.settings-rail-item[aria-current="true"]') ?? box).focus();
    } else {
      modal.removeEventListener("keydown", onKeydown);
      if (opener?.isConnected) opener.focus();
      opener = null;
    }
  }).observe(modal, { attributes: true, attributeFilter: ["class"] });
}
