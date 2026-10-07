import { describe, it, expect } from "vitest";
import {
  formatDuration,
  applyFilter,
  getStartAllTargets,
  getStopAllTargets,
  formatRowStatusCopy,
  formatNextRun,
  updatePeak,
  isListening,
  isEventTriggerType,
  triggerTypeFromKindJson,
  resolveNextRunAt,
} from "../monitor-helpers";

describe("monitor-helpers.formatDuration", () => {
  it("normal case: sub-minute durations render as whole seconds", () => {
    expect(formatDuration(45_000)).toBe("45s");
  });

  it("edge case: durations at or past 60s render as rounded minutes, not seconds", () => {
    expect(formatDuration(60_000)).toBe("1m");
    expect(formatDuration(125_000)).toBe("2m");
  });

  it("edge case: a negative span (wall clock stepped backward) clamps to 0s instead of rendering a negative", () => {
    expect(formatDuration(-3_000)).toBe("0s");
  });
});

describe("monitor-helpers.applyFilter", () => {
  const rows = [
    { id: "1", name: "a", status: "running" as const },
    { id: "2", name: "b", status: "idle" as const },
    { id: "3", name: "c", status: "stopped" as const },
    { id: "4", name: "d", status: "failed" as const },
    { id: "5", name: "e", status: "done" as const },
  ];

  it("normal case: 'idle' isolates only idle rows", () => {
    expect(applyFilter(rows, "idle", "").map(r => r.id)).toEqual(["2"]);
  });

  it("edge case: 'stopped' isolates only stopped rows, excluding every other status", () => {
    expect(applyFilter(rows, "stopped", "").map(r => r.id)).toEqual(["3"]);
  });
});

describe("monitor-helpers.getStartAllTargets", () => {
  const rows = [
    { id: "1", name: "a", status: "running" as const },
    { id: "2", name: "b", status: "idle" as const },
    { id: "3", name: "c", status: "stopped" as const },
    { id: "4", name: "d", status: "failed" as const },
    { id: "5", name: "e", status: "done" as const },
  ];

  it("normal case: includes idle, stopped, and failed rows", () => {
    expect(getStartAllTargets(rows).map(r => r.id).sort()).toEqual(["2", "3", "4"]);
  });

  it("edge case: excludes running and done rows", () => {
    const ids = getStartAllTargets(rows).map(r => r.id);
    expect(ids).not.toContain("1");
    expect(ids).not.toContain("5");
  });
});

describe("monitor-helpers.getStopAllTargets", () => {
  it("normal case: isolates only running rows", () => {
    const rows = [
      { id: "1", name: "a", status: "running" as const },
      { id: "2", name: "b", status: "idle" as const },
      { id: "3", name: "c", status: "stopped" as const },
      { id: "4", name: "d", status: "failed" as const },
      { id: "5", name: "e", status: "done" as const },
    ];
    expect(getStopAllTargets(rows).map(r => r.id)).toEqual(["1"]);
  });
});

describe("monitor-helpers.formatRowStatusCopy", () => {
  const now = 1_000_000;

  it("normal case: a done row shows time since it finished, not run duration", () => {
    const row = { id: "1", name: "a", status: "done" as const, startedAt: now - 500_000, finishedAt: now - 125_000 };
    expect(formatRowStatusCopy(row, now)).toBe("Last run 2m ago");
  });

  it("normal case: a failed row is labelled distinctly from a done row", () => {
    const row = { id: "1", name: "a", status: "failed" as const, finishedAt: now - 45_000 };
    expect(formatRowStatusCopy(row, now)).toBe("Failed 45s ago");
  });

  it("edge case: no finishedAt/startedAt (idle) renders the placeholder, not a crash", () => {
    const row = { id: "1", name: "a", status: "idle" as const };
    expect(formatRowStatusCopy(row, now)).toBe("—");
  });
});

describe("monitor-helpers.updatePeak", () => {
  it("normal case: a reading above the tracked peak becomes the new peak", () => {
    expect(updatePeak(500, 300)).toBe(500);
    expect(updatePeak(500, null)).toBe(500);
  });

  it("edge case: a failed read or a lower reading never lowers the tracked peak", () => {
    expect(updatePeak(null, 300)).toBe(300);
    expect(updatePeak(200, 300)).toBe(300);
  });
});

describe("monitor-helpers.formatNextRun", () => {
  it("normal case: sub-minute countdowns render as whole seconds", () => {
    expect(formatNextRun(7)).toBe("Next run 7s");
  });

  it("edge case: countdowns at or past 60s render as minutes and seconds", () => {
    expect(formatNextRun(125)).toBe("Next run 2m 5s");
  });
});

describe("monitor-helpers trigger kinds", () => {
  it("triggerTypeFromKindJson reads the kind tag and returns null for bad input", () => {
    expect(triggerTypeFromKindJson('{"kind":"plugin","type_id":"p","config":"{}"}')).toBe("plugin");
    expect(triggerTypeFromKindJson("not json")).toBeNull();
    expect(triggerTypeFromKindJson('{"secs":5}')).toBeNull();
    expect(triggerTypeFromKindJson(undefined)).toBeNull();
  });

  it("isEventTriggerType is true only for webhook and plugin", () => {
    expect(isEventTriggerType("webhook")).toBe(true);
    expect(isEventTriggerType("plugin")).toBe(true);
    expect(isEventTriggerType("interval")).toBe(false);
    expect(isEventTriggerType(null)).toBe(false);
  });
});

describe("monitor-helpers Listening", () => {
  const now = 1_000_000;
  it("shows Listening for a running webhook or plugin row without a next-fire time", () => {
    for (const triggerType of ["webhook", "plugin"]) {
      const row = { id: "1", name: "a", status: "running" as const, startedAt: now - 90_000, triggerType };
      expect(isListening(row)).toBe(true);
      expect(formatRowStatusCopy(row, now)).toBe("Listening");
    }
  });

  it("keeps the elapsed time when a plugin reports a next-fire time, and for schedule rows", () => {
    const plugin = { id: "1", name: "a", status: "running" as const, startedAt: now - 90_000, triggerType: "plugin", nextRunAt: "2030-01-01T00:00:00Z" };
    expect(isListening(plugin)).toBe(false);
    expect(formatRowStatusCopy(plugin, now)).toBe("2m");
    const sched = { id: "2", name: "b", status: "running" as const, startedAt: now - 5_000, triggerType: "interval" };
    expect(formatRowStatusCopy(sched, now)).toBe("5s");
  });

  it("a stopped webhook row is not Listening", () => {
    expect(isListening({ status: "stopped", triggerType: "webhook" })).toBe(false);
  });
});

describe("monitor-helpers Scheduled filter", () => {
  const rows = [
    { id: "w", name: "hook", status: "running" as const, triggerType: "webhook" },
    { id: "p", name: "plug", status: "running" as const, triggerType: "plugin" },
    { id: "s", name: "sched", status: "running" as const, triggerType: "cron", nextRunAt: "2030-01-01T00:00:00Z" },
    { id: "x", name: "stopped hook", status: "stopped" as const, triggerType: "webhook" },
    { id: "i", name: "idle", status: "idle" as const },
  ];
  it("lists running Schedule, Webhook, and plugin rows, with or without a next-fire time", () => {
    expect(applyFilter(rows, "scheduled", "").map(r => r.id)).toEqual(["w", "p", "s"]);
  });
});

describe("monitor-helpers.resolveNextRunAt", () => {
  it("uses the event value when present", () => {
    expect(resolveNextRunAt("T2", { status: "running", nextRunAt: "T1" }, "running")).toBe("T2");
  });
  it("keeps the previous value across a null event while the job stays running", () => {
    expect(resolveNextRunAt(null, { status: "running", nextRunAt: "T1" }, "running")).toBe("T1");
  });
  it("drops it when the job stops or finishes", () => {
    expect(resolveNextRunAt(null, { status: "running", nextRunAt: "T1" }, "stopped")).toBeUndefined();
    expect(resolveNextRunAt(null, { status: "running", nextRunAt: "T1" }, "done")).toBeUndefined();
  });
  it("does not revive a stale value when a stopped job starts again", () => {
    expect(resolveNextRunAt(null, { status: "stopped", nextRunAt: "T1" }, "running")).toBeUndefined();
  });
});
