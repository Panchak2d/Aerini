/**
 * T1-9 (S10-1, CRITICAL): `localToIso` used to append ":00Z" to the raw
 * datetime-local value with no real timezone conversion — correct only for
 * UTC+0 users. These tests pin `process.env.TZ` (Node/V8 re-reads it per
 * `Date` construction, confirmed empirically) to non-UTC zones and assert
 * the stored UTC instant actually matches the intended local wall-clock time.
 */
import { describe, it, expect, afterEach } from "vitest";
import { isoToLocal, localToIso } from "../node-configs/schedule-config";

const ORIGINAL_TZ = process.env.TZ;

afterEach(() => {
  if (ORIGINAL_TZ === undefined) delete process.env.TZ;
  else process.env.TZ = ORIGINAL_TZ;
});

describe("localToIso", () => {
  it("converts a local wall-clock time to the correct UTC instant — UTC+5:30 (no DST)", () => {
    process.env.TZ = "Asia/Kolkata";
    // 09:00 local in India is 03:30 UTC.
    expect(localToIso("2025-06-15T09:00")).toBe("2025-06-15T03:30:00Z");
  });

  it("converts a local wall-clock time to the correct UTC instant — UTC-5 (America/New_York, EST)", () => {
    process.env.TZ = "America/New_York";
    // Jan 15 is outside DST (EST, UTC-5): 09:00 local -> 14:00 UTC.
    expect(localToIso("2025-01-15T09:00")).toBe("2025-01-15T14:00:00Z");
  });

  it("converts a local wall-clock time to the correct UTC instant — UTC-4 (America/New_York, EDT)", () => {
    process.env.TZ = "America/New_York";
    // Jun 15 is inside DST (EDT, UTC-4): 09:00 local -> 13:00 UTC.
    expect(localToIso("2025-06-15T09:00")).toBe("2025-06-15T13:00:00Z");
  });

  it("regression: does NOT reproduce the pre-fix bug of treating local time as UTC", () => {
    process.env.TZ = "Asia/Kolkata";
    // Pre-fix behavior would have returned "2025-06-15T09:00:00Z" (wrong —
    // that's 2:30pm IST, not 9am IST).
    expect(localToIso("2025-06-15T09:00")).not.toBe("2025-06-15T09:00:00Z");
  });

  it("is a no-op / empty for an empty string", () => {
    expect(localToIso("")).toBe("");
  });
});

describe("isoToLocal", () => {
  it("converts a stored UTC ISO string back to the correct local wall-clock value — UTC+5:30", () => {
    process.env.TZ = "Asia/Kolkata";
    expect(isoToLocal("2025-06-15T03:30:00Z")).toBe("2025-06-15T09:00");
  });

  it("converts a stored UTC ISO string back to the correct local wall-clock value — UTC-4 (EDT)", () => {
    process.env.TZ = "America/New_York";
    expect(isoToLocal("2025-06-15T13:00:00Z")).toBe("2025-06-15T09:00");
  });

  it("returns empty string for an unparseable value instead of emitting garbage", () => {
    expect(isoToLocal("not-a-date")).toBe("");
  });
});

describe("localToIso / isoToLocal round-trip", () => {
  it("is self-consistent across a non-UTC, non-DST-boundary timezone", () => {
    process.env.TZ = "Asia/Kolkata";
    const local = "2025-06-15T09:00";
    expect(isoToLocal(localToIso(local))).toBe(local);
  });

  it("is self-consistent across the US spring-forward DST boundary", () => {
    process.env.TZ = "America/New_York";
    // 2025-03-09 02:30 America/New_York does not exist (spring-forward gap);
    // pick an unambiguous time either side of the transition instead.
    const local = "2025-03-09T01:30";
    expect(isoToLocal(localToIso(local))).toBe(local);
  });
});
