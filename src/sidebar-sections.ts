import { enterMonitorMode, exitMonitorMode } from "./monitor-mode";
import { mountMonitorPanel, unmountMonitorPanel } from "./panels/MonitorPanel";

const LS_ZONE  = "aerini_active_zone_v2";
const LS_WIDTH = "aerini_sidebar_w_v2";
const MIN_W = 180;   // matches the mockup's own clamp
const MAX_W = 420;
const DEF_W = 260;
const SIDEBAR_STEP = 24; // px per arrow-key press — same value as resize.ts's DRAWER_STEP

function lsGet<T>(key: string, def: T): T {
  try { const v = localStorage.getItem(key); return v ? JSON.parse(v) as T : def; }
  catch { return def; }
}
function lsSet(key: string, val: unknown): void {
  try { localStorage.setItem(key, JSON.stringify(val)); } catch {}
}

export function initSidebarSections(): void {
  applySavedWidth();
  bindActivityBar();
  bindResizeHandle();
  resetTransientState();
  const savedZone = lsGet<string>(LS_ZONE, "workflows");
  switchZone(savedZone, false);
  if (savedZone === "monitor") { enterMonitorMode(); mountMonitorPanel(); }
}

/**
 * Resets every piece of DOM state that user interaction can mutate at runtime
 * but that the HTML source-of-truth never re-asserts.
 *
 * Safe to call unconditionally — every operation is idempotent:
 * adding "hidden" to an already-hidden element, removing a class that is not
 * present, removing a non-existent element, and writing a style to its current
 * value are all no-ops.
 *
 * Issues addressed:
 *   1. Search bars — hidden class stripped by toggle, stays stripped through
 *      a hot bundle reload. The visible result is a blank styled input in the
 *      sidebar (the originally reported bug).
 *   2. Stale input values — bar is hidden but typed text remains; the workflow
 *      list is filtered with no visible indication why.
 *   3. Orphaned dropdowns — sort/filter dropdowns are appended to document.body
 *      and removed only by their own dismiss listener. If that listener is lost
 *      the element is stuck: visible, interactive, and broken.
 *   4. Sort button stale state — data-sort and active class persist; visual
 *      says "Z → A" while WorkflowManager.sortMode has reset to "updated_desc".
 *   5. Filter button stale state — same mismatch for bg runs data-filter/active.
 *   6. Resize handle dragging class — added on mousedown, removed on mouseup.
 *      If a reload fires mid-drag the mouseup handler is gone; "dragging" stays.
 *   7. Body transition stuck — set to "none" on resize mousedown, cleared on
 *      mouseup for the same reason. A stuck "none" silently kills all CSS
 *      transitions app-wide until the next full page load.
 *   8. Body resizing-h class — same mousedown/mouseup pair as #6;
 *      a stuck class here forces `cursor: col-resize !important` app-wide,
 *      not just a broken transition.
 */
function resetTransientState(): void {
  // 1 + 2 — Search bars and stale input values
  const searches = [
    { bar: "wf-search-bar", btn: "btn-wf-search-toggle", input: "wf-search" },
    { bar: "bg-search-bar", btn: "btn-bg-search-toggle", input: "bg-search" },
  ] as const;

  for (const ids of searches) {
    document.getElementById(ids.bar)?.classList.add("hidden");
    document.getElementById(ids.btn)?.classList.remove("active");
    const inp = document.getElementById(ids.input) as HTMLInputElement | null;
    if (inp && inp.value !== "") {
      inp.value = "";
      // If the input listener is already bound this re-fires the filter with an
      // empty query, immediately showing all items. If not yet bound the value
      // is "" so the listener starts from a clean state.
      inp.dispatchEvent(new Event("input"));
    }
  }

  // Workflow list items are hidden via inline style.display — reset them
  // directly so the list is not silently filtered after the input is cleared.
  document.querySelectorAll<HTMLElement>(".workflow-item").forEach(el => {
    el.style.display = "";
  });

  // 3 — Orphaned dropdowns
  document.getElementById("sort-dropdown")?.remove();
  document.getElementById("filter-dropdown")?.remove();

  // 4 — Sort button: WorkflowManager.sortMode resets to "updated_desc" on
  // module re-evaluation, so the button must match.
  const sortBtn = document.getElementById("btn-wf-sort");
  if (sortBtn) {
    sortBtn.dataset.sort = "updated_desc";
    sortBtn.classList.remove("active");
  }

  // 5 — Filter button: same logic for bg runs filter.
  const filterBtn = document.getElementById("btn-bg-filter");
  if (filterBtn) {
    filterBtn.dataset.filter = "all";
    filterBtn.classList.remove("active");
  }

  // 6 — Resize handle dragging class
  document.getElementById("sidebar-resize-handle")?.classList.remove("dragging");

  // 8 — Body resizing-h class
  document.body.classList.remove("resizing-h");

  // 7 — Body transition: only clear the specific value we set; do not touch
  // any transition the rest of the app may have intentionally placed.
  if (document.body.style.transition === "none") {
    document.body.style.transition = "";
  }
}

function applySavedWidth(): void {
  setWidth(lsGet<number>(LS_WIDTH, DEF_W), false);
}

function setWidth(w: number, save: boolean): void {
  const clamped = Math.max(MIN_W, Math.min(MAX_W, w));
  document.documentElement.style.setProperty("--sidebar-w", `${clamped}px`);
  if (save) lsSet(LS_WIDTH, clamped);
}

export function bindResizeHandle(): void {
  const handle = document.getElementById("sidebar-resize-handle");
  if (!handle) return;

  // Bare div has no native resize semantics — same fix resize.ts applied to
  // the drawer handle: a real, labeled, keyboard-reachable separator instead
  // of a mouse-only drag target.
  handle.tabIndex = 0;
  handle.setAttribute("role", "separator");
  handle.setAttribute("aria-orientation", "vertical");
  handle.setAttribute("aria-label", "Resize sidebar");

  handle.addEventListener("mousedown", (e) => {
    e.preventDefault();
    handle.classList.add("dragging");
    document.body.classList.add("resizing-h");
    const startX = e.clientX;
    const startW = lsGet<number>(LS_WIDTH, DEF_W);
    document.body.style.transition = "none";

    const onMove = (me: MouseEvent) => setWidth(startW + me.clientX - startX, false);

    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      handle.classList.remove("dragging");
      document.body.classList.remove("resizing-h");
      document.body.style.transition = "";
      const w = parseInt(
        getComputedStyle(document.documentElement).getPropertyValue("--sidebar-w"), 10
      );
      lsSet(LS_WIDTH, w);
    };

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  });

  // Double-click: reset to the default width — persisted immediately (unlike
  // resize.ts's drawer reset, sidebar width IS persisted across reloads via
  // LS_WIDTH, so a non-persisted reset would silently revert on next launch).
  handle.addEventListener("dblclick", () => setWidth(DEF_W, true));

  // Arrow keys resize while the handle has focus — Right = wider, Left =
  // narrower, matching the drag direction (moving the pointer right is what
  // grows startW + delta above). Persisted on every press, not just on
  // mouseup: onMove's own startW is read from LS_WIDTH (not the live
  // --sidebar-w value), so an un-persisted keyboard resize would leave a
  // subsequent drag silently ignoring it and snapping back to the last
  // persisted width.
  handle.addEventListener("keydown", (e) => {
    const cur = parseInt(document.documentElement.style.getPropertyValue("--sidebar-w"), 10) || DEF_W;
    if (e.key === "ArrowRight")     { e.preventDefault(); setWidth(cur + SIDEBAR_STEP, true); }
    else if (e.key === "ArrowLeft") { e.preventDefault(); setWidth(cur - SIDEBAR_STEP, true); }
  });
}

let _current = "nodes";

function switchZone(key: string, animate: boolean): void {
  const prev = _current;
  _current = key;

  document.querySelectorAll<HTMLElement>(".activity-btn[data-zone]").forEach(btn => {
    const active = btn.dataset.zone === key;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-selected", String(active));
  });

  // Deactivate by querying the DOM's actual .active state, not the tracked
  // `prev` variable — `prev` is only used to decide whether the outgoing
  // zone animates out. If `prev` is ever stale (e.g. a zone is marked
  // active in the static HTML before this module's first switchZone call
  // runs), relying on `prev` alone would leave that zone's `active` class
  // never cleared, producing two simultaneously visible zones.
  document.querySelectorAll<HTMLElement>(".sidebar-zone").forEach(zone => {
    const zk = zone.id.replace("zone-", "");
    if (zk === key) {
      zone.classList.remove("leaving");
      zone.classList.add("active");
    } else if (zone.classList.contains("active") && zk === prev && animate) {
      zone.classList.remove("active");
      zone.classList.add("leaving");
      setTimeout(() => zone.classList.remove("leaving"), 160);
    } else {
      zone.classList.remove("active", "leaving");
    }
  });

  lsSet(LS_ZONE, key);
}

function bindActivityBar(): void {
  document.querySelectorAll<HTMLElement>(".activity-btn[data-zone]").forEach(btn => {
    btn.addEventListener("click", () => {
      const z = btn.dataset.zone ?? "nodes";
      if (z === _current) return;
      const wasMonitor = _current === "monitor";
      switchZone(z, true);
      if (z === "monitor") { enterMonitorMode(); mountMonitorPanel(); }
      else if (wasMonitor) { exitMonitorMode(); unmountMonitorPanel(); }
    });
  });
}

export function activateZone(key: string): void {
  if (key !== _current) switchZone(key, true);
}

export function getCurrentZone(): string {
  return _current;
}

export function updateActivityBadge(zone: "workflows" | "bgruns", count: number): void {
  const badge = document.querySelector<HTMLElement>(
    `.activity-btn[data-zone="${zone}"] .activity-badge`
  );
  if (!badge) return;
  if (count === 0) {
    badge.textContent = "";
    badge.classList.add("activity-badge--hidden");
  } else {
    badge.textContent = count > 99 ? "99+" : String(count);
    badge.classList.remove("activity-badge--hidden");
  }
}

export function updateBgRunningState(hasRunning: boolean): void {
  document.querySelector<HTMLElement>('.activity-btn[data-zone="bgruns"]')
    ?.classList.toggle("has-running", hasRunning);
}

export function bindSectionSearchToggles(): void {
  bindSearchToggle("btn-wf-search-toggle", "wf-search-bar", "wf-search");
  bindSearchToggle("btn-bg-search-toggle", "bg-search-bar", "bg-search");
}

function bindSearchToggle(btnId: string, barId: string, inputId: string): void {
  const btn   = document.getElementById(btnId);
  const bar   = document.getElementById(barId);
  const input = document.getElementById(inputId) as HTMLInputElement | null;
  if (!btn || !bar || !input) return;
  btn.addEventListener("click", () => {
    const hidden = bar.classList.toggle("hidden");
    btn.classList.toggle("active", !hidden);
    btn.setAttribute("aria-expanded", String(!hidden));
    if (!hidden) input.focus();
    else { input.value = ""; input.dispatchEvent(new Event("input")); }
  });
}

export function bindWorkflowSectionControls(onSort: (mode: string) => void): void {
  const search = document.getElementById("wf-search") as HTMLInputElement | null;
  const sort   = document.getElementById("btn-wf-sort");

  search?.addEventListener("input", () => {
    const q = search.value.toLowerCase().trim();
    document.querySelectorAll<HTMLElement>(".workflow-item").forEach(item => {
      const name = item.querySelector(".workflow-item-name")?.textContent?.toLowerCase() ?? "";
      const tags = item.dataset.wfTags ?? "";
      item.style.display = q && !name.includes(q) && !tags.includes(q) ? "none" : "";
    });
  });

  const SORT_OPTIONS = [
    { value: "updated_desc", label: "Newest first" },
    { value: "updated_asc",  label: "Oldest first" },
    { value: "name_asc",     label: "A → Z" },
    { value: "name_desc",    label: "Z → A" },
  ];

  sort?.addEventListener("click", (e) => {
    e.stopPropagation();
    document.getElementById("sort-dropdown")?.remove();

    const cur = sort.dataset.sort ?? "updated_desc";
    const rect = sort.getBoundingClientRect();

    const dd = document.createElement("div");
    dd.id = "sort-dropdown";
    dd.className = "filter-dropdown";
    dd.style.top = `${rect.bottom + 4}px`;
    dd.style.left = `${rect.left}px`;

    SORT_OPTIONS.forEach(opt => {
      const btn = document.createElement("button");
      btn.className = "filter-dropdown-item" + (opt.value === cur ? " active" : "");
      btn.textContent = opt.label;
      btn.addEventListener("mousedown", (ev) => {
        ev.preventDefault();
        sort.dataset.sort = opt.value;
        sort.title = opt.label;
        sort.classList.toggle("active", opt.value !== "updated_desc");
        onSort(opt.value);
        dd.remove();
      });
      dd.appendChild(btn);
    });

    document.body.appendChild(dd);
    const dismiss = (ev: MouseEvent) => {
      if (!dd.contains(ev.target as Node) && ev.target !== sort) {
        dd.remove(); document.removeEventListener("mousedown", dismiss, true);
      }
    };
    setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);
  });
}

export function bindBgRunsFilter(onFilter: (status: string, query: string) => void): void {
  const search = document.getElementById("bg-search") as HTMLInputElement | null;
  const filter = document.getElementById("btn-bg-filter");

  const state = () => ({
    status: filter?.dataset.filter ?? "all",
    query:  search?.value.toLowerCase().trim() ?? "",
  });

  search?.addEventListener("input", () => { const s = state(); onFilter(s.status, s.query); });

  const FILTER_OPTIONS = [
    { value: "all",     label: "All runs" },
    { value: "running", label: "Running" },
    { value: "done",    label: "Done" },
    { value: "failed",  label: "Failed" },
  ];

  filter?.addEventListener("click", (e) => {
    e.stopPropagation();
    document.getElementById("filter-dropdown")?.remove();

    const cur  = filter.dataset.filter ?? "all";
    const rect = filter.getBoundingClientRect();

    const dd = document.createElement("div");
    dd.id = "filter-dropdown";
    dd.className = "filter-dropdown";
    dd.style.top = `${rect.bottom + 4}px`;
    dd.style.left = `${rect.left}px`;

    FILTER_OPTIONS.forEach(opt => {
      const btn = document.createElement("button");
      btn.className = "filter-dropdown-item" + (opt.value === cur ? " active" : "");
      btn.textContent = opt.label;
      btn.addEventListener("mousedown", (ev) => {
        ev.preventDefault();
        filter.dataset.filter = opt.value;
        filter.title = opt.label;
        filter.classList.toggle("active", opt.value !== "all");
        const s = state();
        onFilter(opt.value, s.query);
        dd.remove();
      });
      dd.appendChild(btn);
    });

    document.body.appendChild(dd);
    const dismiss = (ev: MouseEvent) => {
      if (!dd.contains(ev.target as Node) && ev.target !== filter) {
        dd.remove(); document.removeEventListener("mousedown", dismiss, true);
      }
    };
    setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);
  });
}
