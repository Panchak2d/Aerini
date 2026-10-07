import type { BgJob } from "./run-manager";
import type { WorkflowSummary } from "./ipc/workflow";

export type RowStatus = "running" | "done" | "failed" | "stopped" | "idle";

export interface MonitorRow {
  id: string;
  name: string;
  status: RowStatus;
  startedAt?: number;
  finishedAt?: number;
  nextRunAt?: string | null;
  alwaysOn?: boolean;
  /** "interval" | "cron" | "once" | "webhook" | "manual" | "plugin"; absent when unknown. */
  triggerType?: string | null;
}

const BACKGROUND_TRIGGER_TYPES = new Set(["interval", "cron", "once", "webhook", "plugin"]);

/** Trigger types that wait for an outside event rather than a clock. */
export function isEventTriggerType(triggerType: string | null | undefined): boolean {
  return triggerType === "webhook" || triggerType === "plugin";
}

/** Reads the `kind` tag from a stored TriggerKind JSON string; null if it is not valid JSON or has no tag. */
export function triggerTypeFromKindJson(json: string | null | undefined): string | null {
  if (!json) return null;
  try {
    const kind = (JSON.parse(json) as { kind?: unknown }).kind;
    return typeof kind === "string" ? kind : null;
  } catch {
    return null;
  }
}

/** True for an armed Webhook or plugin trigger that is waiting for an event and has no next-fire time to count down to. */
export function isListening(row: Pick<MonitorRow, "status" | "triggerType" | "nextRunAt">): boolean {
  return row.status === "running" && isEventTriggerType(row.triggerType) && !row.nextRunAt;
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
    alwaysOn: j.alwaysOn,
    triggerType: j.triggerType,
  }));
  for (const wf of idleWfs) rows.push({ id: wf.id, name: wf.name, status: "idle" });
  return rows;
}

export function applyFilter(rows: MonitorRow[], status: string, query: string): MonitorRow[] {
  let out = rows;
  if (status === "running") out = out.filter(r => r.status === "running");
  else if (status === "success") out = out.filter(r => r.status === "done");
  else if (status === "failed") out = out.filter(r => r.status === "failed");
  else if (status === "stopped") out = out.filter(r => r.status === "stopped");
  else if (status === "idle") out = out.filter(r => r.status === "idle");
  else if (status === "scheduled") out = out.filter(r => !!r.nextRunAt || r.alwaysOn === true
    || (r.status === "running" && !!r.triggerType && BACKGROUND_TRIGGER_TYPES.has(r.triggerType)));
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
  if (isListening(row)) return "Listening";
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

/**
 * Next-fire time to keep for a job after a scheduler event. A null in the
 * event keeps the previous value only while the job continues running (the
 * scheduler sends a null completion event just before the real next time);
 * a job that stopped, finished, failed, or is starting again after one of
 * those never keeps a stale countdown.
 */
export function resolveNextRunAt(
  eventNext: string | null,
  previous: { status: string; nextRunAt?: string | null } | undefined,
  newStatus: string,
): string | undefined {
  if (eventNext) return eventNext;
  if (newStatus !== "running") return undefined;
  if (!previous || previous.status !== "running") return undefined;
  return previous.nextRunAt ?? undefined;
}
