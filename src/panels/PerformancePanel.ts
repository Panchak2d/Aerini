import type { WorkflowManager } from "../workflow-manager";
import {
  getLivePerformance,
  getRecentPerformance,
  listenPerformanceLive,
  type PerformanceReport,
  type HistorySample,
} from "../ipc/performance";
import { formatBytes } from "../mem-summary";
import { isTauri } from "../utils";



const GRAPH_W = 100;
const GRAPH_H = 40;

export function buildGraphSvg(history: HistorySample[]): string {
  if (history.length < 2) {
    return `<div class="perf-graph-empty">Not enough samples yet.</div>`;
  }

  const minAt = history[0].at_ms;
  const maxAt = history[history.length - 1].at_ms;
  const spanAt = Math.max(1, maxAt - minAt); // guard: all samples sharing one timestamp

  let minB = history[0].bytes, maxB = history[0].bytes;
  for (const s of history) {
    if (s.bytes < minB) minB = s.bytes;
    if (s.bytes > maxB) maxB = s.bytes;
  }
  const spanB = Math.max(1, maxB - minB); 

 
  const points = history
    .map(s => {
      const x = ((s.at_ms - minAt) / spanAt) * GRAPH_W;
      const y = GRAPH_H - ((s.bytes - minB) / spanB) * GRAPH_H;
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");

  return `<svg viewBox="0 0 ${GRAPH_W} ${GRAPH_H}" preserveAspectRatio="none" class="perf-graph-svg">
    <polyline points="${points}" />
  </svg>`;
}

export function initPerformancePanel(wfManager: WorkflowManager): { refresh: () => void } {
  const empty      = document.getElementById("perf-empty");
  const content     = document.getElementById("perf-content");
  const dot         = document.getElementById("perf-dot");
  const statusEl    = document.getElementById("perf-status");
  const currentEl   = document.getElementById("perf-current");
  const peakEl      = document.getElementById("perf-peak");
  const baselineEl  = document.getElementById("perf-baseline");
  const deltaEl     = document.getElementById("perf-delta");
  const graphEl     = document.getElementById("perf-graph");
  if (!empty || !content || !dot || !statusEl || !currentEl || !peakEl || !baselineEl || !deltaEl || !graphEl) {
    return { refresh: () => {} };
  }

  if (!isTauri()) {
    // No Rust backend to ask in plain-browser dev mode — same convention
    // initMemChip uses: an inert, explanatory empty state, not a panel
    // that silently never renders.
    empty.textContent = "Performance monitoring — desktop app only";
    return { refresh: () => {} };
  }

  function render(report: PerformanceReport | null): void {
    if (!report) {
      empty!.hidden = false;
      content!.hidden = true;
      return;
    }
    empty!.hidden = true;
    content!.hidden = false;

    dot!.dataset.status = report.status;
    statusEl!.textContent =
      report.status === "running" ? "Running" : report.status === "success" ? "Success" : "Failed";

    currentEl!.textContent  = formatBytes(report.final_bytes);
    peakEl!.textContent     = formatBytes(report.peak_bytes);
    baselineEl!.textContent = formatBytes(report.baseline_bytes);
    // delta_bytes is signed (a run can end below its own baseline) —
    // formatBytes' tier-selection assumes a non-negative count (every
    // other caller only ever passes one), so a negative value must be
    // sign-split before going through it, not passed through directly.
    const sign = report.delta_bytes < 0 ? "-" : "+";
    deltaEl!.textContent = `${sign}${formatBytes(Math.abs(report.delta_bytes))}`;

    graphEl!.innerHTML = buildGraphSvg(report.history);
  }

  async function load(): Promise<void> {
    const id = wfManager.currentId;
    const live = await getLivePerformance(id).catch(() => null);
    if (wfManager.currentId !== id) return; // switched workflow mid-fetch
    if (live) { render(live); return; }

    const recent = await getRecentPerformance(id).catch(() => null);
    if (wfManager.currentId !== id) return; // switched workflow mid-fetch
    render(recent);
  }

  load();
  listenPerformanceLive(reports => {
    const mine = reports.find(r => r.workflow_id === wfManager.currentId);
    if (mine) render(mine);
  }).catch(() => {});

  return { refresh: load };
}
