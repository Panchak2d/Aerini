import type { Canvas } from "./canvas/Canvas";
import type { WorkflowManager } from "./workflow-manager";
import { getMemoryBreakdown, listenMemoryBreakdown, type RunBreakdown } from "./ipc/memory";
import { getRecentPerformance } from "./ipc/performance";
import { summarizeMemory, formatBytes } from "./mem-summary";
import { isTauri } from "./utils";


export function initMemChip(wfManager: WorkflowManager): { refresh: () => void } {
  const chip   = document.getElementById("mem-chip")   as HTMLButtonElement | null;
  const dot    = chip?.querySelector<HTMLElement>(".mem-dot") ?? null;
  const valueEl = document.getElementById("mem-chip-value");
  const popover = document.getElementById("mem-popover");
  // Optional: only present while the Background Runs sidebar zone is in the
  // DOM (index.html static markup — always present in the real app, but
  // test doubles for this chip alone won't have it). Aggregate total for
  // every run that ISN'T the currently open workflow; kept separate from
  // the chip above so "this workflow's memory" and "everything else" never
  // get summed into one misleading number.
  const bgBadge = document.getElementById("bg-mem-badge");
  if (!chip || !dot || !valueEl || !popover) return { refresh: () => {} };

  if (!isTauri()) {
    // No Rust backend to ask in plain-browser dev mode — leave the chip as
    // an inert "—", not a dead button that throws on click.
    chip.disabled = true;
    chip.title = "Memory usage — desktop app only";
    chip.setAttribute("data-tooltip", "Memory usage — desktop app only");
    return { refresh: () => {} };
  }

  const cap = (s: string) => s.length ? s[0].toUpperCase() + s.slice(1) : s;

  // render()'s only caller of this always passes an entry from currentData,
  // which is filtered to workflow_id === wfManager.currentId by construction
  // -- so this is just the current workflow's name.
  function resolveName(): string {
    return wfManager.currentName || "Untitled";
  }

  function setOpen(open: boolean): void {
    popover!.hidden = !open;
    chip!.setAttribute("aria-expanded", open ? "true" : "false");
  }

  function render(data: RunBreakdown[]): void {
    const currentData = data.filter(r => r.workflow_id === wfManager.currentId);

    const { level, totalBytes, maxRowBytes } = summarizeMemory(currentData);
    chip!.dataset.level = level;
    valueEl!.textContent = level === "idle" ? "—" : formatBytes(totalBytes);

    if (bgBadge) {
      const bgTotal = summarizeMemory(data).totalBytes;
      bgBadge.classList.toggle("hidden", bgTotal <= 0);
      bgBadge.textContent = bgTotal > 0 ? formatBytes(bgTotal) : "";
      bgBadge.title = data.length
        ? `${data.length} run${data.length === 1 ? "" : "s"} running, using ${formatBytes(bgTotal)} total`
        : "";
      bgBadge.setAttribute("data-tooltip", bgBadge.title);
    }

    if (currentData.length === 0) {
      popover!.innerHTML = `<div class="mem-popover-empty">This workflow isn't currently running. Memory tracking is only active while a run is in flight.</div>`;

      const forId = wfManager.currentId;
      getRecentPerformance(forId).then(report => {
        // Discard a stale result: the user may have switched workflows,
        // or a newer render (a real run starting) may have already
        // updated the chip to a live, non-idle state since this fetch
        // began — never clobber either with an old fallback.
        if (!report || wfManager.currentId !== forId || chip!.dataset.level !== "idle") return;
        valueEl!.textContent = formatBytes(report.final_bytes);
        popover!.innerHTML = `
          <div class="mem-popover-head">Memory by run</div>
          <div class="mem-popover-empty">Last run: ${formatBytes(report.final_bytes)}</div>
          <div class="mem-popover-foot">This workflow isn't running right now — showing its last completed run.</div>`;
      }).catch(() => {});
      return;
    }

    // Static structure only — no dynamic value is templated into this
    // string. `.mem-row-name`/`.mem-run-name` are left empty and `.mem-row-bar i`
    // is left unstyled; both are filled in below via direct DOM/CSSOM writes.
    const row = (bytes: number) => `
      <div class="mem-row">
        <span class="mem-row-name"></span>
        <span class="mem-row-bar"><i></i></span>
        <span class="mem-row-val">${formatBytes(bytes)}</span>
      </div>`;

    const runsHtml = currentData.map(run => `
      <div class="mem-run">
        <div class="mem-run-name"></div>
        ${row(run.live_bytes)}
        ${run.nodes.map(n => row(n.live_bytes)).join("")}
      </div>`).join("");

    popover!.innerHTML = `
      <div class="mem-popover-head">Memory by run</div>
      ${runsHtml}
      <div class="mem-popover-foot">Live allocator data, updates ~2s while a run is active.</div>`;

    // Fill in the dynamic parts the template above intentionally left
    // empty. querySelectorAll returns elements in document order, which is
    // exactly the order `currentData`/`run.nodes` were iterated in to build
    // the HTML above, so a straight zip-by-index is safe — no per-row id
    // needed (there can be an arbitrary number of runs/nodes).
    const runEls = popover!.querySelectorAll<HTMLElement>(".mem-run");
    currentData.forEach((run, i) => {
      const runEl = runEls[i];
      const nameEl = runEl.querySelector<HTMLElement>(".mem-run-name");
      if (nameEl) { const n = resolveName(); nameEl.textContent = n; nameEl.title = n; }

      const rowLabels = ["Run overhead", ...run.nodes.map(n => cap(n.node_type_id))];
      const rowBytes  = [run.live_bytes, ...run.nodes.map(n => n.live_bytes)];
      const rowEls = runEl.querySelectorAll<HTMLElement>(".mem-row");
      rowLabels.forEach((label, j) => {
        const rowEl = rowEls[j];
        const nameSpan = rowEl.querySelector<HTMLElement>(".mem-row-name");
        if (nameSpan) { nameSpan.textContent = label; nameSpan.title = label; }
        const bar = rowEl.querySelector<HTMLElement>(".mem-row-bar i");
        if (bar) bar.style.width = `${Math.min(100, rowBytes[j] / maxRowBytes * 100)}%`;
      });
    });
  }

  getMemoryBreakdown().then(render).catch(() => {}); // initial paint
  listenMemoryBreakdown(render).catch(() => {});

  chip.addEventListener("click", (e) => {
    e.stopPropagation();
    setOpen(!!popover!.hidden);
  });
  document.addEventListener("click", (e) => {
    if (!popover!.hidden && !popover!.contains(e.target as Node) && e.target !== chip) setOpen(false);
  });
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") setOpen(false); });

  return { refresh: () => { getMemoryBreakdown().then(render).catch(() => {}); } };
}

export function initStatusBarFields(
  canvas: Canvas,
  wfManager: WorkflowManager,
  
  onRunStateDetected?: () => void
): { refreshMem: () => void } {
  const bar = document.getElementById("canvas-stat-bar");
  if (!bar) return { refreshMem: () => {} };

  // No wrapper div here — unlike #status-bar (which still has to share its
  // row with #status-text/.status-hint), #canvas-stat-bar *is* this row,
  // with nothing else mounted into it. Set its content directly.
  bar.innerHTML = `
    <span class="statusbar-field">Nodes <b id="stat-nodes">0</b></span>
    <span class="statusbar-sep"></span>
    <span class="statusbar-field" data-tooltip="Connections between nodes">Conn <b id="stat-conn">0</b></span>
    <span class="statusbar-sep"></span>
    <span class="statusbar-field">Selected <b id="stat-selected">0</b></span>
    <span class="statusbar-sep"></span>
    <span class="statusbar-field">Zoom <b id="stat-zoom">100%</b></span>
    <span class="statusbar-sep"></span>
    <span class="mem-chip-wrap">
      <button type="button" class="mem-chip" id="mem-chip" data-level="idle" aria-expanded="false" aria-haspopup="true" title="Memory usage for the open workflow — live while running, last run's total otherwise" data-tooltip="Memory usage for the open workflow">
        <span class="mem-dot" aria-hidden="true"></span>MEM <b id="mem-chip-value">—</b>
      </button>
      <div class="mem-popover" id="mem-popover" hidden></div>
    </span>
    <span class="statusbar-sep"></span>
    <span class="statusbar-state" id="statusbar-state" data-state="idle">
      <span class="statusbar-dot" aria-hidden="true"></span><b id="stat-run-state">Ready</b>
    </span>
  `;
  const mem = initMemChip(wfManager);

  const nodesEl   = document.getElementById("stat-nodes")!;
  const connEl    = document.getElementById("stat-conn")!;
  const selEl     = document.getElementById("stat-selected")!;
  const zoomEl    = document.getElementById("stat-zoom")!;
  const stateWrap = document.getElementById("statusbar-state")!;
  const stateEl   = document.getElementById("stat-run-state")!;

  let lastNodes = -1, lastConn = -1, lastSel = -1, lastZoom = -1, lastState = "";

  // 150ms, not 300ms: #run-dropdown-wrap's success/error data-run-state also
  // only holds for 300ms (toolbar.ts's onRunResult) before reverting to
  // "idle" — a poll period equal to that window's length risks a bad phase
  // alignment missing it entirely. Half the window guarantees at least one
  // tick lands inside it every time.
  setInterval(() => {
    const nNodes  = canvas.nodes.size;
    const nConn   = canvas.connectors.size;
    const nSel    = canvas.selectedNodes.size;
    const zoomPct = Math.round(canvas.zoom * 100);

    if (nNodes  !== lastNodes) { nodesEl.textContent = String(nNodes); lastNodes = nNodes; }
    if (nConn   !== lastConn)  { connEl.textContent  = String(nConn);  lastConn  = nConn; }
    if (nSel    !== lastSel)   { selEl.textContent   = String(nSel);   lastSel   = nSel; }
    if (zoomPct !== lastZoom)  { zoomEl.textContent  = `${zoomPct}%`;  lastZoom  = zoomPct; }

    const raw = document.getElementById("run-dropdown-wrap")?.getAttribute("data-run-state") ?? "idle";
    if (raw !== lastState) {
      lastState = raw;
      const state = raw === "success" ? "ok" : raw; // "success" (DOM attr) -> "ok" (--statusbar-* token name)
      const label = state === "running" ? "Running" : state === "ok" ? "Success" : state === "error" ? "Error" : "Ready";
      stateEl.textContent = label;
      stateWrap.dataset.state = state;
      mem.refresh();
      onRunStateDetected?.();
    }
  }, 150);

  return { refreshMem: mem.refresh };
}
