import { describe, it, expect } from "vitest";
import {
  formatDuration,
  applyFilter,
  getStartAllTargets,
  getStopAllTargets,
  formatRowStatusCopy,
  formatNextRun,
  updatePeak,
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
