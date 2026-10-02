import type { BgJob, TriggerType } from "./run-manager";
import type { WorkflowSummary } from "./ipc/workflow";

export type RowStatus = "running" | "done" | "failed" | "stopped" | "idle";

export interface MonitorRow {
  id: string;
  name: string;
  status: RowStatus;
  startedAt?: number;
  finishedAt?: number;
  nextRunAt?: string | null;
  triggerType?: TriggerType;
  waiting?: boolean;
}

export function getStartAllTargets(rows: MonitorRow[]): MonitorRow[] {
  return rows.filter(r => r.status === "idle" || r.status === "stopped" || r.status === "failed");
}

export function getStopAllTargets(rows: MonitorRow[]): MonitorRow[] {
  return rows.filter(r => r.status === "running");
}

export function collectRows(jobs: BgJob[], idleWfs: WorkflowSummary[]): MonitorRow[] {
  const rows: MonitorRow[] = jobs.map(j => ({
    id: j.id,
    name: j.name,
    status: j.status,
    startedAt: j.startedAt,
    finishedAt: j.finishedAt,
    nextRunAt: j.nextRunAt,
    triggerType: j.triggerType,
    waiting: j.waiting,
  }));
  for (const wf of idleWfs) rows.push({ id: wf.id, name: wf.name, status: "idle" });
  return rows;
}

// A job counts as scheduled while it is armed (running) with a trigger that
// fires on its own: a timer, a webhook listener or a plugin. A job whose
// trigger type isn't known falls back to whether it has a next-run time.
export function isScheduledRow(r: MonitorRow): boolean {
  if (r.status !== "running") return false;
  return r.triggerType ? r.triggerType !== "manual" : !!r.nextRunAt;
}

// An armed webhook or plugin job between runs with no known next fire: it is
// waiting for a request or event, not executing. Elapsed time would mislead.
export function isListeningRow(r: MonitorRow): boolean {
  return r.status === "running" && r.waiting === true && !r.nextRunAt
    && (r.triggerType === "webhook" || r.triggerType === "plugin");
}

export function applyFilter(rows: MonitorRow[], status: string, query: string): MonitorRow[] {
  let out = rows;
  if (status === "running") out = out.filter(r => r.status === "running");
  else if (status === "success") out = out.filter(r => r.status === "done");
  else if (status === "failed") out = out.filter(r => r.status === "failed");
  else if (status === "stopped") out = out.filter(r => r.status === "stopped");
  else if (status === "idle") out = out.filter(r => r.status === "idle");
  else if (status === "scheduled") out = out.filter(isScheduledRow);
  if (query) out = out.filter(r => r.name.toLowerCase().includes(query));
  return out;
}

export function formatDuration(ms: number): string {
  const secs = Math.max(0, Math.round(ms / 1000));
  return secs < 60 ? `${secs}s` : `${Math.round(secs / 60)}m`;
}

// Tightened per-status row copy ("Failed 2m ago" / "Last run 12m ago"),
// reusing formatDuration — the file's one existing time-formatter — rather
// than adding a second one. `now` defaults to Date.now() but is an explicit
// param so this stays a pure, directly-testable function.
export function formatRowStatusCopy(row: MonitorRow, now: number = Date.now()): string {
  if (isListeningRow(row)) return "Listening";
  if (row.status === "running" && row.startedAt) return formatDuration(now - row.startedAt);
  if (row.finishedAt && row.status === "done")    return `Last run ${formatDuration(now - row.finishedAt)} ago`;
  if (row.finishedAt && row.status === "failed")  return `Failed ${formatDuration(now - row.finishedAt)} ago`;
  if (row.finishedAt && row.status === "stopped") return `Stopped ${formatDuration(now - row.finishedAt)} ago`;
  return "—";
}

/**
 * Pure peak-tracking step: given a fresh process-memory reading and the
 * previously-tracked peak, returns the peak that should be tracked next.
 * A failed read (`null`) never lowers or clears an existing peak — only
 * the "Reset Peak" control does that, by setting `_peakProcessBytes`
 * directly rather than through this function.
 */
export function updatePeak(reading: number | null, prevPeak: number | null): number | null {
  if (reading === null) return prevPeak;
  if (prevPeak === null || reading > prevPeak) return reading;
  return prevPeak;
}

// The only place nextRunAt gets formatted — not a second formatter alongside it.
export function formatNextRun(secsLeft: number): string {
  return secsLeft < 60
    ? `Next run ${secsLeft}s`
    : `Next run ${Math.floor(secsLeft / 60)}m ${secsLeft % 60}s`;
}
