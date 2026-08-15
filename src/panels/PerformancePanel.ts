import type { WorkflowManager } from "../workflow-manager";
import {
  getLivePerformance,
  getRecentPerformance,
  listenPerformanceLive,
  type PerformanceReport,
  type PerfStatus,
  type HistorySample,
} from "../ipc/performance";
import { formatBytes } from "../mem-summary";
import { isTauri } from "../utils";



const GRAPH_W = 100;
const GRAPH_H = 40;

export interface GraphMarkers {
  peakBytes?: number;
  peakAtMs?: number;
  baselineBytes?: number;
}

export function buildGraphSvg(history: HistorySample[], markers: GraphMarkers = {}): string {
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

  // Clamped: history-derived points are always in-range by construction,
  // but peak/baseline come from separate report fields — clamp defensively
  // rather than assume they can never drift outside [minB, maxB].
  const xOf = (atMs: number) => Math.min(GRAPH_W, Math.max(0, ((atMs - minAt) / spanAt) * GRAPH_W));
  const yOf = (bytes: number) => Math.min(GRAPH_H, Math.max(0, GRAPH_H - ((bytes - minB) / spanB) * GRAPH_H));

  const points = history
    .map(s => `${xOf(s.at_ms).toFixed(2)},${yOf(s.bytes).toFixed(2)}`)
    .join(" ");

  const areaPoints = `${xOf(minAt).toFixed(2)},${GRAPH_H} ${points} ${xOf(maxAt).toFixed(2)},${GRAPH_H}`;

  const baselineSvg = markers.baselineBytes === undefined ? "" :
    `<line class="perf-graph-baseline" x1="0" y1="${yOf(markers.baselineBytes).toFixed(2)}" x2="${GRAPH_W}" y2="${yOf(markers.baselineBytes).toFixed(2)}"/>`;

  const peakHtml = (markers.peakBytes === undefined || markers.peakAtMs === undefined) ? "" :
    `<div class="perf-graph-peak" style="left:${(xOf(markers.peakAtMs) / GRAPH_W * 100).toFixed(2)}%;top:${(yOf(markers.peakBytes) / GRAPH_H * 100).toFixed(2)}%"></div>`;

  return `<div class="perf-graph-inner">
    <div class="perf-graph-axis">
      <span>${formatBytes(maxB)}</span>
      <span>${formatBytes(minB)}</span>
    </div>
    <div class="perf-graph-plot">
      <svg viewBox="0 0 ${GRAPH_W} ${GRAPH_H}" preserveAspectRatio="none" class="perf-graph-svg">
        <line class="perf-graph-grid" x1="0" y1="${GRAPH_H * 0.25}" x2="${GRAPH_W}" y2="${GRAPH_H * 0.25}"/>
        <line class="perf-graph-grid" x1="0" y1="${GRAPH_H * 0.5}" x2="${GRAPH_W}" y2="${GRAPH_H * 0.5}"/>
        <line class="perf-graph-grid" x1="0" y1="${GRAPH_H * 0.75}" x2="${GRAPH_W}" y2="${GRAPH_H * 0.75}"/>
        <polygon class="perf-graph-area" points="${areaPoints}"/>
        ${baselineSvg}
        <polyline class="perf-graph-line" points="${points}"/>
      </svg>
      ${peakHtml}
    </div>
  </div>`;
}

// Matches BgJobsPanel's own duration convention exactly (seconds under a
// minute, rounded minutes above it) rather than introducing a second one.
function formatDuration(ms: number): string {
  const secs = Math.round(ms / 1000);
  return secs < 60 ? `${secs}s` : `${Math.round(secs / 60)}m`;
}

// PerfStatus ("running"/"success"/"failed") has no "stopped" analog in
// bg-job-status--*'s family ("running"/"done"/"failed"/"stopped") — this
// maps the two vocabularies onto the shared class family instead of
// forking a new one; "stopped" is simply never reached from here.
const STATUS_LABEL: Record<PerfStatus, string> = { running: "Running", success: "Success", failed: "Failed" };
const STATUS_CLASS: Record<PerfStatus, string> = { running: "running", success: "done", failed: "failed" };

export function initPerformancePanel(wfManager: WorkflowManager): { refresh: () => void } {
  const empty       = document.getElementById("perf-empty");
  const content      = document.getElementById("perf-content");
  const dot          = document.getElementById("perf-dot");
  const statusEl     = document.getElementById("perf-status");
  const idleNoteEl   = document.getElementById("perf-idle-note");
  const currentEl    = document.getElementById("perf-current");
  const peakEl       = document.getElementById("perf-peak");
  const baselineEl   = document.getElementById("perf-baseline");
  const averageEl    = document.getElementById("perf-average");
  const minimumEl    = document.getElementById("perf-minimum");
  const deltaEl      = document.getElementById("perf-delta");
  const durationEl   = document.getElementById("perf-duration");
  const samplesEl    = document.getElementById("perf-samples");
  const intervalEl   = document.getElementById("perf-interval");
  const graphEl      = document.getElementById("perf-graph");
  const tooltipEl    = document.getElementById("perf-graph-tooltip");
  if (!empty || !content || !dot || !statusEl || !idleNoteEl || !currentEl || !peakEl || !baselineEl ||
      !averageEl || !minimumEl || !deltaEl || !durationEl || !samplesEl || !intervalEl || !graphEl || !tooltipEl) {
    return { refresh: () => {} };
  }

  if (!isTauri()) {
    // No Rust backend to ask in plain-browser dev mode — same convention
    // initMemChip uses: an inert, explanatory empty state, not a panel
    // that silently never renders.
    empty.textContent = "Performance monitoring — desktop app only";
    return { refresh: () => {} };
  }

  // Backing store for the hover tooltip below — graphEl's own children are
  // fully replaced on every render(), so the mousemove listener is bound
  // once to the stable parent and reads this instead of stale closures.
  let lastHistory: HistorySample[] = [];

  function render(report: PerformanceReport | null): void {
    if (!report) {
      empty!.hidden = false;
      content!.hidden = true;
      return;
    }
    empty!.hidden = true;
    content!.hidden = false;
    lastHistory = report.history;

    const cls = STATUS_CLASS[report.status];
    dot!.className = `bg-job-dot bg-job-dot--${cls}`;
    statusEl!.className = `bg-job-status bg-job-status--${cls}`;
    statusEl!.textContent = STATUS_LABEL[report.status];
    // Not currently running — note this is showing a past run, not a live
    // one, same tone as initMemChip's own post-run fallback (statusbar-fields.ts).
    idleNoteEl!.hidden = report.status === "running";

    currentEl!.textContent  = formatBytes(report.final_bytes);
    peakEl!.textContent     = formatBytes(report.peak_bytes);
    baselineEl!.textContent = formatBytes(report.baseline_bytes);
    averageEl!.textContent  = formatBytes(report.average_bytes);
    minimumEl!.textContent  = formatBytes(report.minimum_bytes);
    // delta_bytes is signed (a run can end below its own baseline) —
    // formatBytes' tier-selection assumes a non-negative count (every
    // other caller only ever passes one), so a negative value must be
    // sign-split before going through it, not passed through directly.
    const sign = report.delta_bytes < 0 ? "-" : "+";
    deltaEl!.textContent = `${sign}${formatBytes(Math.abs(report.delta_bytes))}`;

    durationEl!.textContent = formatDuration(report.duration_ms);
    samplesEl!.textContent  = String(report.sample_count);
    intervalEl!.textContent = `${report.sampling_interval_ms}ms`;

    graphEl!.innerHTML = buildGraphSvg(report.history, {
      peakBytes: report.peak_bytes,
      peakAtMs: report.peak_at_ms,
      baselineBytes: report.baseline_bytes,
    });
  }

  graphEl.addEventListener("mousemove", (e) => {
    const plot = graphEl.querySelector<HTMLElement>(".perf-graph-plot");
    if (!plot || lastHistory.length < 2) return;
    const plotRect = plot.getBoundingClientRect();
    if (plotRect.width <= 0) return;
    const frac = Math.min(1, Math.max(0, (e.clientX - plotRect.left) / plotRect.width));
    const minAt = lastHistory[0].at_ms;
    const maxAt = lastHistory[lastHistory.length - 1].at_ms;
    const targetAt = minAt + frac * (maxAt - minAt);

    let nearest = lastHistory[0];
    let bestDiff = Math.abs(nearest.at_ms - targetAt);
    for (const s of lastHistory) {
      const diff = Math.abs(s.at_ms - targetAt);
      if (diff < bestDiff) { bestDiff = diff; nearest = s; }
    }

    tooltipEl!.textContent = `${formatDuration(nearest.at_ms - minAt)} — ${formatBytes(nearest.bytes)}`;
    tooltipEl!.hidden = false;
    // left is a % of #perf-graph itself (the tooltip's positioning
    // context — see index.html), not of .perf-graph-plot alone, so the
    // plot's own offset within that box (the axis-label column's width)
    // has to be added back in, in the same units, or the tooltip drifts
    // left of the cursor by exactly that column's width.
    const wrapRect = graphEl.getBoundingClientRect();
    if (wrapRect.width > 0) {
      const leftPx = (plotRect.left - wrapRect.left) + frac * plotRect.width;
      tooltipEl!.style.left = `${(leftPx / wrapRect.width * 100).toFixed(2)}%`;
    }
  });
  graphEl.addEventListener("mouseleave", () => { tooltipEl!.hidden = true; });

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
