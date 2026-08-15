import type { RunBreakdown } from "./ipc/memory";

export type MemLevel = "idle" | "active" | "warn" | "high";

export interface MemSummary {
  level: MemLevel;
  totalBytes: number;
  /** Largest single row (a run's own overhead, or one node) in this
   *  snapshot — used to scale the popover's bar widths. Always >= 1, even
   *  when every row is legitimately 0 bytes, so callers never divide by 0. */
  maxRowBytes: number;
}

const MB = 1024 * 1024;

// Thresholds are a UI judgment call, not a measured/verified fact — tune
// freely, nothing else depends on these exact numbers.
const WARN_BYTES = 50 * MB;
const HIGH_BYTES = 150 * MB;

/** `data.length === 0` is the real idle state (backend disables tracking
 *  entirely while no run is in flight) — never fabricate a total for it. */
export function summarizeMemory(data: RunBreakdown[]): MemSummary {
  if (data.length === 0) return { level: "idle", totalBytes: 0, maxRowBytes: 1 };

  let total = 0;
  let maxRow = 0;
  for (const run of data) {
    total += run.live_bytes;
    maxRow = Math.max(maxRow, run.live_bytes);
    for (const node of run.nodes) {
      total += node.live_bytes;
      maxRow = Math.max(maxRow, node.live_bytes);
    }
  }

  const level: MemLevel = total > HIGH_BYTES ? "high" : total > WARN_BYTES ? "warn" : "active";
  return { level, totalBytes: total, maxRowBytes: Math.max(maxRow, 1) };
}

export type ProcessMemLevel = "active" | "warn" | "high";

const HISTORY_CAP = 60; // ~60s of samples at MonitorPanel's 1s poll tick
const MIN_SAMPLES_FOR_BASELINE = 5;
const ELEVATED_RATIO = 1.5;
const HIGH_RATIO = 3;

/** Appends a reading to a capped rolling window, dropping the oldest
 *  sample once full. Pure — caller owns and reassigns the array. */
export function pushMemSample(history: number[], bytes: number): number[] {
  const next = history.length >= HISTORY_CAP ? history.slice(1) : history.slice();
  next.push(bytes);
  return next;
}

function median(nums: number[]): number {
  const sorted = [...nums].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

/**
 * Classifies a process-RSS reading against the median of prior readings
 * (history excludes the current one — push it in after calling this).
 * Relative, not a fixed-MB cutoff, because RSS baseline varies a lot per
 * install (workflow count, OS, webview overhead) — a fixed number either
 * false-alarms on a healthy install with more workflows loaded or misses
 * a real spike on a lean one. Ratio thresholds are a UI judgment call,
 * not a measured fact, same status as this file's WARN/HIGH_BYTES above.
 *
 * Known limitation: a slow leak raises the rolling median along with the
 * reading, so gradual drift over minutes can stay "active" — this catches
 * spikes within the ~60s window, not long-run creep. Peak (tracked
 * separately in MonitorPanel.ts) is the signal for that instead.
 */
export function classifyProcessMem(history: number[], current: number): ProcessMemLevel {
  if (history.length < MIN_SAMPLES_FOR_BASELINE) return "active";
  const base = median(history);
  if (base <= 0) return "active";
  const ratio = current / base;
  if (ratio > HIGH_RATIO) return "high";
  if (ratio > ELEVATED_RATIO) return "warn";
  return "active";
}

const KB = 1024;
const GB = MB * 1024;
const TB = GB * 1024;


export function formatBytes(bytes: number): string {
  if (bytes < MB) {
    // Guard the display boundary, not just the raw one: a value like
    // 1,048,566 bytes (1023.99 KB unrounded) is < 1 MB but rounds to
    // "1024.0" at 1 decimal, which would misleadingly display as
    // "1024.0 KB" instead of rolling over to "1.0 MB". Round first, then
    // re-check which tier that rounded value actually belongs in.
    const roundedKB = Math.round((bytes / KB) * 10) / 10;
    if (roundedKB < 1024) return `${roundedKB.toFixed(1)} KB`;
  }
  if (bytes < GB) {
    // Same guard, one tier up: e.g. 1,073,741,823 bytes (1024.0 MB
    // unrounded-to-1-decimal) is < 1 GB but must roll over to "1.0 GB",
    // not display as "1024.0 MB".
    const roundedMB = Math.round((bytes / MB) * 10) / 10;
    if (roundedMB < 1024) return `${roundedMB.toFixed(1)} MB`;
  }
  if (bytes < TB) {
    // Same guard, one tier further: rolls a GB value that rounds up to
    // 1024.0 over into TB instead of misdisplaying as "1024.0 GB".
    const roundedGB = Math.round((bytes / GB) * 10) / 10;
    if (roundedGB < 1024) return `${roundedGB.toFixed(1)} GB`;
  }
  return `${(bytes / TB).toFixed(1)} TB`;
}

