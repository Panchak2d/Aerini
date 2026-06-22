const LS_ZONE  = "aerini_active_zone_v2";
const LS_WIDTH = "aerini_sidebar_w_v2";
const MIN_W = 220;
const MAX_W = 420;
const DEF_W = 260;

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
  const savedZone = lsGet<string>(LS_ZONE, "workflows");
  switchZone(savedZone, false);
}

function applySavedWidth(): void {
  setWidth(lsGet<number>(LS_WIDTH, DEF_W), false);
}

function setWidth(w: number, save: boolean): void {
  const clamped = Math.max(MIN_W, Math.min(MAX_W, w));
  document.documentElement.style.setProperty("--sidebar-w", `${clamped}px`);
  if (save) lsSet(LS_WIDTH, clamped);
}

function bindResizeHandle(): void {
  const handle = document.getElementById("sidebar-resize-handle");
  if (!handle) return;

  handle.addEventListener("mousedown", (e) => {
    e.preventDefault();
    handle.classList.add("dragging");
    const startX = e.clientX;
    const startW = lsGet<number>(LS_WIDTH, DEF_W);
    document.body.style.transition = "none";

    const onMove = (me: MouseEvent) => setWidth(startW + me.clientX - startX, false);

    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      handle.classList.remove("dragging");
      document.body.style.transition = "";
      const w = parseInt(
        getComputedStyle(document.documentElement).getPropertyValue("--sidebar-w"), 10
      );
      lsSet(LS_WIDTH, w);
    };

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  });
}

let _current = "nodes";

function switchZone(key: string, animate: boolean): void {
  const prev = _current;
  _current = key;

  document.querySelectorAll<HTMLElement>(".activity-btn[data-zone]").forEach(btn => {
    btn.classList.toggle("active", btn.dataset.zone === key);
  });

  document.querySelectorAll<HTMLElement>(".sidebar-zone").forEach(zone => {
    const zk = zone.id.replace("zone-", "");
    if (zk === key) {
      zone.classList.remove("leaving");
      zone.classList.add("active");
    } else if (zk === prev && animate) {
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
      if (z !== _current) switchZone(z, true);
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
      item.style.display = q && !name.includes(q) ? "none" : "";
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
    dd.style.cssText = `top:${rect.bottom + 4}px;left:${rect.left}px;`;

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
    dd.style.cssText = `top:${rect.bottom + 4}px;left:${rect.left}px;`;

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
