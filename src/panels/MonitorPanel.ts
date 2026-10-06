import {
  listWorkflows,
  stopScheduledWorkflow,
  startScheduledWorkflow,
  parseSchedulerError,
  type WorkflowSummary,
} from "../ipc/workflow";
import { getBgJobs } from "../run-manager";
import { isWorkflowRunning, type WorkflowManager } from "../workflow-manager";
import { getMemoryBreakdown, getProcessMemory, type RunBreakdown } from "../ipc/memory";
import { getRecentPerformance } from "../ipc/performance";
import { summarizeMemory, formatBytes, type MemSummary } from "../mem-summary";
import { isTauri } from "../utils";
import { showConfirm } from "../confirm";
import {
  getStartAllTargets,
  getStopAllTargets,
  collectRows,
  applyFilter,
  formatRowStatusCopy,
  updatePeak,
  formatNextRun,
  type MonitorRow,
} from "../monitor-helpers";

const POLL_MS = 1000;
// listWorkflows() is a real IPC round-trip; getBgJobs() just reads an
// in-memory map. Only re-fetch the workflow list every Nth tick so an open
// Monitor tab doesn't hit the backend once a second for data that rarely
// changes.
const WORKFLOW_REFRESH_EVERY = 5;

const FILTER_OPTIONS = [
  { value: "all", label: "All" },
  { value: "running", label: "Running" },
  { value: "success", label: "Success" },
  { value: "failed", label: "Failed" },
  { value: "stopped", label: "Stopped" },
  { value: "idle", label: "Idle" },
  { value: "scheduled", label: "Scheduled" },
] as const;

const OVERVIEW_STATUSES = [
  { value: "all", label: "All" },
  { value: "running", label: "Running" },
  { value: "idle", label: "Idle" },
  { value: "stopped", label: "Stopped" },
  { value: "failed", label: "Failed" },
] as const;

let _mounted = false;
let _pollTimer: ReturnType<typeof setInterval> | null = null;
let _tick = 0;
let _idleWorkflows: WorkflowSummary[] = [];
let _filterStatus = "all";
let _filterQuery = "";
let _lastSignature = "";

const _rowEls = new Map<string, { durationEl: HTMLElement; memEl: HTMLElement }>();
const _countdownIntervals = new Map<string, ReturnType<typeof setInterval>>();
const _lastKnownBytes = new Map<string, number>();
const _fetchingBytes = new Set<string>();
const _overviewRowEls = new Map<string, { row: HTMLElement; countEl: HTMLElement }>();
let _resourceEls: { memory: HTMLElement; peak: HTMLElement; workflowMem: HTMLElement; active: HTMLElement } | null = null;

// Peak process memory — module scope so it survives Monitor open/close
// (unmountMonitorPanel() below deliberately never resets this); the only
// way to clear it is the "Reset Peak" control in the Resources sidebar.
let _peakProcessBytes: number | null = null;

// Set once by app.ts after WorkflowManager is constructed (mountMonitorPanel()
// can run before that, on startup, if the last-used zone was Monitor) — same
// late-injection pattern as canvas/node-registry.ts's registerNodeDescriptors().
let _wfManager: WorkflowManager | null = null;
export function setMonitorWfManager(wfManager: WorkflowManager): void {
  _wfManager = wfManager;
}

let _lastAllRows: MonitorRow[] = [];
let _startAllInFlight = false;
let _stopAllInFlight = false;

export function mountMonitorPanel(): void {
  if (_mounted) return;
  const area = document.getElementById("monitor-area");
  if (!area) return;
  _mounted = true;

  if (!isTauri()) {
    area.innerHTML = `<div class="monitor-empty">Monitor is only available in the desktop app.</div>`;
    const zone = document.getElementById("zone-monitor");
    if (zone) zone.innerHTML = "";
    return;
  }

  buildShell(area);
  _tick = 0;
  void renderTick();
  _pollTimer = setInterval(() => void renderTick(), POLL_MS);
}

export function unmountMonitorPanel(): void {
  if (_pollTimer !== null) {
    clearInterval(_pollTimer);
    _pollTimer = null;
  }
  clearCountdowns();
  _rowEls.clear();
  _overviewRowEls.clear();
  _resourceEls = null;
  _lastSignature = "";
  _lastAllRows = [];
  _startAllInFlight = false;
  _stopAllInFlight = false;
  _mounted = false;
}

// ── Shell ──────────────────────────────────────────────────────────────

function buildShell(area: HTMLElement): void {
  _filterStatus = "all";
  _filterQuery = "";
  _lastAllRows = [];
  _canvasCardSignature = ""; // area.innerHTML below wipes the DOM; force the next tick to paint it

  area.innerHTML = `
    <div class="monitor-header" id="monitor-header">
      <div class="monitor-stat">
        <span class="monitor-stat-label">Memory</span>
        <span class="monitor-stat-val" id="monitor-mem-total">—</span>
      </div>
      <div class="monitor-stat">
        <span class="monitor-stat-label">Active runs</span>
        <span class="monitor-stat-val" id="monitor-active-count">0</span>
      </div>
      <div class="monitor-header-actions" id="monitor-header-actions">
        <button class="monitor-bulk-btn monitor-bulk-btn--start hidden" id="btn-monitor-start-all" type="button">Start All</button>
        <button class="monitor-bulk-btn monitor-bulk-btn--stop hidden" id="btn-monitor-stop-all" type="button">Stop All</button>
      </div>
    </div>
    <div class="monitor-canvas-card" id="monitor-canvas-card"></div>
    <div id="monitor-list"></div>
  `;
  bindHeaderActions();

  const zone = document.getElementById("zone-monitor");
  if (!zone) return;
  zone.innerHTML = `
    <div class="zone-header">
      <span class="zone-title">Monitor</span>
      <div class="zone-header-actions">
        <button class="zone-action-btn" id="btn-monitor-search-toggle" title="Search workflows" data-tooltip="Search workflows" aria-label="Search workflows" aria-expanded="false">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/></svg>
        </button>
        <button class="zone-action-btn" id="btn-monitor-filter" title="Filter: All" data-tooltip="Filter: All" aria-label="Filter workflows" data-filter="all">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3"/></svg>
        </button>
      </div>
    </div>
    <div class="zone-search hidden" id="monitor-search-bar">
      <input id="monitor-search" type="text" placeholder="Filter workflows…" aria-label="Filter workflows" autocomplete="off" spellcheck="false" />
    </div>
    <div class="zone-body-scroll">
      <div class="monitor-sidebar-section">
        <div class="monitor-sidebar-section-title">Overview</div>
        <div class="monitor-overview-list" id="monitor-overview-list"></div>
      </div>
      <div class="monitor-sidebar-section">
        <div class="monitor-sidebar-section-title">Resources</div>
        <div class="monitor-resources" id="monitor-resources"></div>
      </div>
    </div>
  `;
  bindZoneControls();
  buildOverviewSidebar();
  buildResourcesSidebar();
}

function bindZoneControls(): void {
  const searchToggle = document.getElementById("btn-monitor-search-toggle");
  const searchBar = document.getElementById("monitor-search-bar");
  const searchInput = document.getElementById("monitor-search") as HTMLInputElement | null;

  searchToggle?.addEventListener("click", () => {
    const hidden = searchBar?.classList.toggle("hidden");
    searchToggle.classList.toggle("active", !hidden);
    searchToggle.setAttribute("aria-expanded", String(!hidden));
    if (!hidden) {
      searchInput?.focus();
    } else if (searchInput) {
      searchInput.value = "";
      _filterQuery = "";
      _lastSignature = "";
    }
  });

  searchInput?.addEventListener("input", () => {
    _filterQuery = searchInput.value.toLowerCase().trim();
    _lastSignature = ""; // force a rebuild on the next tick
  });

  const filterBtn = document.getElementById("btn-monitor-filter");
  filterBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    document.getElementById("monitor-filter-dropdown")?.remove();

    const cur = filterBtn.dataset.filter ?? "all";
    const rect = filterBtn.getBoundingClientRect();
    const dd = document.createElement("div");
    dd.id = "monitor-filter-dropdown";
    dd.className = "filter-dropdown";
    dd.style.top = `${rect.bottom + 4}px`;
    dd.style.left = `${rect.left}px`;

    FILTER_OPTIONS.forEach(opt => {
      const btn = document.createElement("button");
      btn.className = "filter-dropdown-item" + (opt.value === cur ? " active" : "");
      btn.textContent = opt.label;
      btn.addEventListener("mousedown", (ev) => {
        ev.preventDefault();
        setFilterStatus(opt.value);
        dd.remove();
      });
      dd.appendChild(btn);
    });

    document.body.appendChild(dd);
    const dismiss = (ev: MouseEvent) => {
      if (!dd.contains(ev.target as Node) && ev.target !== filterBtn) {
        dd.remove();
        document.removeEventListener("mousedown", dismiss, true);
      }
    };
    setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);
  });
}

// ── Header: Start All / Stop All ────────────────────────────────────────

const MAX_START_ATTEMPTS = 10;

/**
 * Retries on the next port up when the configured one is already bound —
 * mirrors the fallback ChatPanel's own Start button already applies
 * (`startOnFreePort`). Without this, two workflows sharing a default port
 * left whichever one lost the race never actually running: this row (or
 * Start All) showed a normal-looking failure toast, but that workflow's
 * Chat panel then correctly — if confusingly — kept its input disabled,
 * since the backend genuinely never bound it.
 *
 * Unlike `startOnFreePort`, doesn't need to know the workflow's own
 * configured port up front — `port_conflict`'s own `port` field already
 * echoes back whatever was just tried, so each retry just adds one to it.
 *
 * Returns the original busy port if a fallback was needed, else `null`.
 */
export async function startWithPortFallback(id: string): Promise<number | null> {
  let portOverride: number | undefined;
  let busyPort: number | null = null;
  for (let i = 0; i < MAX_START_ATTEMPTS; i++) {
    try {
      await startScheduledWorkflow(id, portOverride);
      return busyPort;
    } catch (err) {
      const parsed = parseSchedulerError(String(err));
      if (parsed.error_kind !== "port_conflict" || i === MAX_START_ATTEMPTS - 1) throw err;
      busyPort ??= parsed.port;
      portOverride = parsed.port + 1;
    }
  }
  return busyPort;
}

function bindHeaderActions(): void {
  const startBtn = document.getElementById("btn-monitor-start-all") as HTMLButtonElement | null;
  const stopBtn = document.getElementById("btn-monitor-stop-all") as HTMLButtonElement | null;
  startBtn?.addEventListener("click", () => void handleStartAll(startBtn));
  stopBtn?.addEventListener("click", () => void handleStopAll(stopBtn));
}

function updateHeaderActions(allRows: MonitorRow[]): void {
  const startBtn = document.getElementById("btn-monitor-start-all") as HTMLButtonElement | null;
  const stopBtn = document.getElementById("btn-monitor-stop-all") as HTMLButtonElement | null;
  if (startBtn) {
    const hasTargets = getStartAllTargets(allRows).length > 0;
    startBtn.classList.toggle("hidden", !hasTargets);
    if (!_startAllInFlight) startBtn.disabled = !hasTargets;
  }
  if (stopBtn) {
    const hasTargets = getStopAllTargets(allRows).length > 0;
    stopBtn.classList.toggle("hidden", !hasTargets);
    if (!_stopAllInFlight) stopBtn.disabled = !hasTargets;
  }
}

async function handleStartAll(btn: HTMLButtonElement): Promise<void> {
  if (_startAllInFlight) return;
  const targets = getStartAllTargets(_lastAllRows);
  if (!targets.length) return;

  _startAllInFlight = true;
  btn.disabled = true;
  const label = btn.textContent;
  btn.textContent = "Starting…";
  try {
    const results = await Promise.allSettled(targets.map(r => startWithPortFallback(r.id)));
    results.forEach((res, i) => {
      if (res.status === "rejected") {
        showToast(`Could not start "${targets[i].name}": ${describeSchedulerError(res.reason)}`, "error");
      } else if (res.value !== null) {
        showToast(`Port ${res.value} was busy — "${targets[i].name}" started on a different port instead.`, "info");
      }
    });
  } finally {
    _startAllInFlight = false;
    btn.textContent = label ?? "Start All";
    updateHeaderActions(_lastAllRows);
  }
}

async function handleStopAll(btn: HTMLButtonElement): Promise<void> {
  if (_stopAllInFlight) return;
  const targets = getStopAllTargets(_lastAllRows);
  if (!targets.length) return;

  const ok = await showConfirm(
    `Stop all ${targets.length} running workflow${targets.length === 1 ? "" : "s"}?`,
    true,
    "Stop All",
  );
  if (!ok) return;

  _stopAllInFlight = true;
  btn.disabled = true;
  const label = btn.textContent;
  btn.textContent = "Stopping…";
  try {
    const results = await Promise.allSettled(targets.map(r => stopScheduledWorkflow(r.id)));
    results.forEach((res, i) => {
      if (res.status === "rejected") {
        showToast(`Could not stop "${targets[i].name}": ${describeSchedulerError(res.reason)}`, "error");
      }
    });
  } finally {
    _stopAllInFlight = false;
    btn.textContent = label ?? "Stop All";
    updateHeaderActions(_lastAllRows);
  }
}

// ── Filter state (shared by the dropdown and the Overview sidebar) ─────

function setFilterStatus(value: string): void {
  _filterStatus = value;
  _lastSignature = "";

  const filterBtn = document.getElementById("btn-monitor-filter");
  if (filterBtn) {
    const label = FILTER_OPTIONS.find(opt => opt.value === value)?.label ?? "All";
    filterBtn.dataset.filter = value;
    filterBtn.title = `Filter: ${label}`;
    filterBtn.setAttribute("data-tooltip", `Filter: ${label}`);
    filterBtn.classList.toggle("active", value !== "all");
  }
  syncOverviewActiveState();
}

// ── Sidebar: Overview ────────────────────────────────────────────────

function buildOverviewSidebar(): void {
  const list = document.getElementById("monitor-overview-list");
  if (!list) return;
  list.innerHTML = "";
  _overviewRowEls.clear();

  for (const { value, label } of OVERVIEW_STATUSES) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "monitor-overview-row";
    row.dataset.status = value;

    const labelEl = document.createElement("span");
    labelEl.className = "monitor-overview-label";
    labelEl.textContent = label;

    const countEl = document.createElement("span");
    countEl.className = "monitor-overview-count";
    countEl.textContent = "0";

    row.append(labelEl, countEl);
    row.addEventListener("click", () => setFilterStatus(value));
    list.appendChild(row);
    _overviewRowEls.set(value, { row, countEl });
  }

  syncOverviewActiveState();
}

function syncOverviewActiveState(): void {
  for (const [value, { row, countEl }] of _overviewRowEls) {
    const isActive = value === _filterStatus;
    row.classList.toggle("active", isActive);
    // Reuse monitor.css's existing per-status color tokens directly rather
    // than adding a new accent — "all" has no status-color equivalent, so
    // it falls back to .monitor-overview-row.active's neutral highlight.
    countEl.className = "monitor-overview-count" + (isActive && value !== "all" ? ` monitor-status--${value}` : "");
  }
}

function updateOverviewCounts(allRows: MonitorRow[]): void {
  for (const [value, { countEl }] of _overviewRowEls) {
    const count = value === "all" ? allRows.length : allRows.filter(r => r.status === value).length;
    countEl.textContent = String(count);
  }
}

// ── Sidebar: Resources ──────────────────────────────────────────────

function buildResourceRow(wrap: HTMLElement, label: string): HTMLElement {
  const row = document.createElement("div");
  row.className = "monitor-resource-row";

  const labelEl = document.createElement("span");
  labelEl.className = "monitor-resource-label";
  labelEl.textContent = label;

  const valEl = document.createElement("span");
  valEl.className = "monitor-resource-val";
  valEl.textContent = "—";

  row.append(labelEl, valEl);
  wrap.appendChild(row);
  return valEl;
}

// Peak row needs its own builder (not buildResourceRow) for the "Reset
// Peak" control — restores from the module-level peak on (re)build so a
// remount doesn't blank a peak that's still tracked.
function buildPeakResourceRow(wrap: HTMLElement): HTMLElement {
  const row = document.createElement("div");
  row.className = "monitor-resource-row";

  const labelEl = document.createElement("span");
  labelEl.className = "monitor-resource-label";
  labelEl.textContent = "Peak";

  const right = document.createElement("span");
  right.className = "monitor-resource-peak-right";

  const valEl = document.createElement("span");
  valEl.className = "monitor-resource-val";
  valEl.textContent = _peakProcessBytes === null ? "—" : formatBytes(_peakProcessBytes);

  const resetBtn = document.createElement("button");
  resetBtn.type = "button";
  resetBtn.className = "monitor-resource-reset";
  resetBtn.textContent = "Reset";
  resetBtn.title = "Reset peak memory";
  resetBtn.setAttribute("data-tooltip", "Reset peak memory");
  resetBtn.addEventListener("click", () => {
    _peakProcessBytes = null;
    valEl.textContent = "—";
  });

  right.append(valEl, resetBtn);
  row.append(labelEl, right);
  wrap.appendChild(row);
  return valEl;
}

function buildResourcesSidebar(): void {
  const wrap = document.getElementById("monitor-resources");
  if (!wrap) return;
  wrap.innerHTML = "";
  _resourceEls = {
    // Process-wide RSS (see ipc/memory.ts::getProcessMemory) — the
    // process is always running while the app is open, so unlike
    // workflowMem below this never flickers to "—" between runs.
    memory: buildResourceRow(wrap, "Memory"),
    peak: buildPeakResourceRow(wrap),
    // The per-run allocator total getMemoryBreakdown() already provided —
    // kept, but labelled distinctly from process-wide "Memory" above so
    // the two bytes figures (different metrics, different scopes) are
    // never shown under one ambiguous "Memory" label.
    workflowMem: buildResourceRow(wrap, "Workflow Mem"),
    active: buildResourceRow(wrap, "Active Runs"),
  };
}

function updateResources(processBytes: number | null, summary: MemSummary, activeCount: number): void {
  if (!_resourceEls) return;
  _resourceEls.memory.textContent = processBytes === null ? "—" : formatBytes(processBytes);
  _resourceEls.peak.textContent = _peakProcessBytes === null ? "—" : formatBytes(_peakProcessBytes);
  _resourceEls.workflowMem.textContent = summary.level === "idle" ? "—" : formatBytes(summary.totalBytes);
  _resourceEls.active.textContent = String(activeCount);
}

// ── Poll tick ──────────────────────────────────────────────────────────

async function renderTick(): Promise<void> {
  if (!_mounted) return;
  renderCanvasCard();

  const jobs = getBgJobs();
  const [memData, procMem] = await Promise.all([
    getMemoryBreakdown().catch(() => [] as RunBreakdown[]),
    getProcessMemory().catch(() => null),
  ]);
  if (!_mounted) return; // unmounted while the await above was in flight

  const processBytes = procMem === null ? null : procMem.current_bytes;
  _peakProcessBytes = updatePeak(processBytes, _peakProcessBytes);

  if (_tick % WORKFLOW_REFRESH_EVERY === 0) {
    const wfs = await listWorkflows().catch(() => [] as WorkflowSummary[]);
    if (!_mounted) return;
    _idleWorkflows = wfs.filter(wf => !isWorkflowRunning(wf.id));
  }
  _tick++;

  const allRows = collectRows(jobs, _idleWorkflows);
  _lastAllRows = allRows;
  const rows = applyFilter(allRows, _filterStatus, _filterQuery);
  renderList(rows, memData);
  updateOverviewCounts(allRows);
  updateHeaderActions(allRows);

  const memTotalEl = document.getElementById("monitor-mem-total");
  const activeEl = document.getElementById("monitor-active-count");
  const summary = summarizeMemory(memData);
  const activeCount = jobs.filter(j => j.status === "running").length;
  // Process-wide RSS for the header/sidebar "Memory" figure — see
  // buildResourcesSidebar's "workflowMem" comment for the separate
  // per-run allocator total. Never shows "—" between runs: this reading
  // is never [] the way getMemoryBreakdown() is at idle.
  if (memTotalEl) memTotalEl.textContent = processBytes === null ? "—" : formatBytes(processBytes);
  if (activeEl) activeEl.textContent = String(activeCount);
  updateResources(processBytes, summary, activeCount);
}

// ── Currently-open canvas workflow card ───────────────────────────────

let _canvasCardSignature = "";

function renderCanvasCard(): void {
  const card = document.getElementById("monitor-canvas-card");
  if (!card) return;

  const name = document.getElementById("workflow-name-label")?.textContent?.trim() || "Untitled";
  const running = !!document.getElementById("btn-run-main")?.classList.contains("btn-run-stop");

  const sig = `${name}:${running}`;
  if (sig === _canvasCardSignature) return; // nothing changed — skip the rebuild
  _canvasCardSignature = sig;

  card.innerHTML = "";
  const label = document.createElement("div");
  label.className = "monitor-canvas-card-label";
  label.textContent = "Currently open on canvas";

  const row = document.createElement("div");
  row.className = "monitor-row monitor-row--canvas";

  const status = document.createElement("span");
  status.className = `monitor-status monitor-status--${running ? "running" : "idle"}`;
  status.textContent = running ? "running" : "idle";

  const nameEl = document.createElement("span");
  nameEl.className = "bg-job-name";
  nameEl.textContent = name;
  nameEl.title = name;
  nameEl.setAttribute("data-tooltip", name);

  const btn = document.createElement("button");
  btn.className = "bg-job-action-btn bg-job-action-restart";
  btn.title = running ? "Stop this workflow" : "Run this workflow";
  btn.setAttribute("data-tooltip", running ? "Stop this workflow" : "Run this workflow");
  btn.innerHTML = running
    ? `<svg width="9" height="9" viewBox="0 0 24 24" fill="currentColor"><rect x="3" y="3" width="18" height="18" rx="2"/></svg>`
    : `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="23 4 23 10 17 10"/><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/></svg>`;
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    // Forwards to the toolbar's own Run/Stop button, which already owns the
    // live RunManager instance bound to this canvas — re-deriving that
    // binding here risks targeting the wrong run.
    document.getElementById("btn-run-main")?.click();
  });

  row.append(status, nameEl, btn);
  card.append(label, row);
}

// ── Row list ───────────────────────────────────────────────────────────

function renderList(rows: MonitorRow[], memData: RunBreakdown[]): void {
  const list = document.getElementById("monitor-list");
  if (!list) return;

  const sig = rows.map(r => `${r.id}:${r.status}:${r.name}:${r.nextRunAt ?? ""}:${r.triggerType ?? ""}`).join("|");
  if (sig !== _lastSignature) {
    _lastSignature = sig;
    rebuildList(list, rows);
  }
  for (const row of rows) updateRowLiveFields(row, memData);
}

function rebuildList(list: HTMLElement, rows: MonitorRow[]): void {
  clearCountdowns();
  list.innerHTML = "";
  _rowEls.clear();

  if (!rows.length) {
    const empty = document.createElement("div");
    empty.className = "monitor-empty";
    empty.textContent = _filterStatus !== "all" || _filterQuery
      ? "No workflows match the filter."
      : "No workflows yet. Save or run one to see it here.";
    list.appendChild(empty);
    return;
  }

  for (const row of rows) {
    const item = document.createElement("div");
    item.className = "monitor-row";

    const status = document.createElement("span");
    status.className = `monitor-status monitor-status--${row.status}`;
    status.textContent = row.status === "done" ? "success" : row.status;

    const nameEl = document.createElement("span");
    nameEl.className = "bg-job-name";
    nameEl.textContent = row.name;
    nameEl.title = row.name;
    nameEl.setAttribute("data-tooltip", row.name);

    const memEl = document.createElement("span");
    memEl.className = "monitor-row-mem";
    memEl.textContent = "—";

    const durEl = document.createElement("span");
    durEl.className = "monitor-row-duration";
    durEl.textContent = "—";

    const actions = document.createElement("span");
    actions.className = "bg-job-hover-actions";
    let countdownEl: HTMLElement | null = null;

    if (row.status === "running") {
      const stopBtn = document.createElement("button");
      stopBtn.className = "bg-job-action-btn bg-job-action-stop";
      stopBtn.title = "Stop workflow";
      stopBtn.setAttribute("data-tooltip", "Stop workflow");
      stopBtn.innerHTML = `<svg width="9" height="9" viewBox="0 0 24 24" fill="currentColor"><rect x="3" y="3" width="18" height="18" rx="2"/></svg>`;
      stopBtn.addEventListener("click", (e) => void handleStop(e, row, stopBtn));
      actions.appendChild(stopBtn);

      if (row.nextRunAt) {
        countdownEl = document.createElement("span");
        countdownEl.className = "monitor-row-countdown";
        startCountdown(row.id, row.nextRunAt, countdownEl);
      }
    } else if (row.status !== "idle") {
      const restartBtn = document.createElement("button");
      restartBtn.className = "bg-job-action-btn bg-job-action-restart";
      restartBtn.title = "Restart workflow";
      restartBtn.setAttribute("data-tooltip", "Restart workflow");
      restartBtn.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="23 4 23 10 17 10"/><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/></svg>`;
      restartBtn.addEventListener("click", (e) => void handleStart(e, row, restartBtn, "Restart failed"));
      actions.appendChild(restartBtn);
    } else {
      const startBtn = document.createElement("button");
      startBtn.className = "bg-job-action-btn bg-job-action-restart";
      startBtn.title = "Run in background";
      startBtn.setAttribute("data-tooltip", "Run in background");
      startBtn.innerHTML = `<svg width="9" height="9" viewBox="0 0 24 24" fill="currentColor"><polygon points="6 4 20 12 6 20 6 4"/></svg>`;
      startBtn.addEventListener("click", (e) => void handleStart(e, row, startBtn, "Could not start"));
      actions.appendChild(startBtn);
    }

    const openBtn = document.createElement("button");
    openBtn.className = "bg-job-action-btn bg-job-action-open";
    openBtn.title = "Open on canvas";
    openBtn.setAttribute("data-tooltip", "Open on canvas");
    openBtn.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg>`;
    openBtn.addEventListener("click", (e) => { e.stopPropagation(); handleOpen(row); });
    actions.appendChild(openBtn);

    item.append(status, nameEl, memEl, durEl);
    if (countdownEl) item.appendChild(countdownEl);
    item.appendChild(actions);
    list.appendChild(item);

    _rowEls.set(row.id, { durationEl: durEl, memEl });
  }
}

async function handleStop(e: MouseEvent, row: MonitorRow, btn: HTMLButtonElement): Promise<void> {
  e.stopPropagation();
  btn.disabled = true;
  const label = btn.title;
  btn.title = "Stopping…"; // reverted by rebuildList once the ~1s tick sees the status change; no separate state machine
  try {
    await stopScheduledWorkflow(row.id);
  } catch (err) {
    btn.disabled = false;
    btn.title = label;
    showToast(`Could not stop "${row.name}": ${describeSchedulerError(err)}`, "error");
  }
}

async function handleStart(e: MouseEvent, row: MonitorRow, btn: HTMLButtonElement, failPrefix: string): Promise<void> {
  e.stopPropagation();
  btn.disabled = true;
  const label = btn.title;
  btn.title = "Starting…"; // reverted by rebuildList once the ~1s tick sees the status change
  try {
    const busyPort = await startWithPortFallback(row.id);
    if (busyPort !== null) {
      showToast(`Port ${busyPort} was busy — "${row.name}" started on a different port instead.`, "info");
    }
  } catch (err) {
    btn.disabled = false;
    btn.title = label;
    showToast(`${failPrefix} for "${row.name}": ${describeSchedulerError(err)}`, "error");
  }
}

function handleOpen(row: MonitorRow): void {
  if (!_wfManager) {
    showToast(`Could not open "${row.name}": app is still starting up`, "error");
    return;
  }
  const wfManager = _wfManager;
  // Same navigation path the activity-bar tab click already uses: exits
  // Monitor mode and returns Workflows-zone chrome via the existing click
  // handler (sidebar-sections.ts) instead of calling monitor-mode.ts
  // directly, so this can't duplicate or drift from that module's own
  // return-navigation logic.
  document.getElementById("tab-workflows")?.click();
  void wfManager.handleLoad(row.id);
}

function describeSchedulerError(err: unknown): string {
  const parsed = parseSchedulerError(String(err));
  switch (parsed.error_kind) {
    case "not_schedulable":   return "no Schedule, Webhook, or trigger-plugin trigger on this workflow";
    case "workflow_not_found": return "workflow was deleted";
    case "already_running":   return "already running";
    case "port_conflict":     return `port ${parsed.port} is in use by "${parsed.held_by_workflow_name}"`;
    case "other":              return parsed.message;
  }
}

function updateRowLiveFields(row: MonitorRow, memData: RunBreakdown[]): void {
  const refs = _rowEls.get(row.id);
  if (!refs) return;

  refs.durationEl.textContent = formatRowStatusCopy(row);

  if (row.status === "running") {
    const run = memData.find(r => r.workflow_id === row.id);
    if (run) {
      const bytes = run.live_bytes + run.nodes.reduce((s, n) => s + n.live_bytes, 0);
      refs.memEl.textContent = formatBytes(bytes);
      return;
    }
  }

  const cached = _lastKnownBytes.get(row.id);
  if (cached !== undefined) {
    refs.memEl.textContent = formatBytes(cached);
  } else if (!_fetchingBytes.has(row.id)) {
    _fetchingBytes.add(row.id);
    getRecentPerformance(row.id)
      .then(report => { if (report) _lastKnownBytes.set(row.id, report.final_bytes); })
      .catch(() => {})
      .finally(() => _fetchingBytes.delete(row.id));
  }
}

function clearCountdowns(): void {
  _countdownIntervals.forEach(id => clearInterval(id));
  _countdownIntervals.clear();
}

function startCountdown(rowId: string, nextRunAt: string, el: HTMLElement): void {
  const target = new Date(nextRunAt).getTime();
  const update = () => {
    const secsLeft = Math.max(0, Math.round((target - Date.now()) / 1000));
    el.textContent = formatNextRun(secsLeft);
  };
  update();
  const id = setInterval(() => {
    if (!document.getElementById("monitor-list")?.contains(el)) { clearInterval(id); return; }
    update();
  }, 1000);
  _countdownIntervals.set(rowId, id);
}

let _toastCount = 0;
function showToast(msg: string, type: "success" | "error" | "info" = "info"): void {
  const colors = { success: "var(--green)", error: "var(--red)", info: "var(--blue)" };
  const t = document.createElement("div");
  t.className = "toast";
  t.style.background = colors[type];
  t.style.bottom = `${34 + _toastCount * 44}px`; // stack above any toast already showing instead of overlapping it — Start All/Stop All can now fire several at once
  t.textContent = msg;
  document.body.appendChild(t);
  _toastCount++;
  setTimeout(() => {
    t.style.opacity = "0";
    setTimeout(() => { t.remove(); _toastCount--; }, 300);
  }, 2800);
}
