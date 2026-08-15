import { describe, it, expect } from "vitest";
import { summarizeMemory, formatBytes, pushMemSample, classifyProcessMem } from "../mem-summary";
import type { RunBreakdown } from "../ipc/memory";

const MB = 1024 * 1024;

function run(overrides: Partial<RunBreakdown> = {}): RunBreakdown {
  return { workflow_id: "wf_1", started_at_ms: 0, live_bytes: 0, nodes: [], ...overrides };
}

describe("pushMemSample", () => {
  it("normal case: appends a reading, growing the window", () => {
    const h = pushMemSample([1 * MB, 2 * MB], 3 * MB);
    expect(h).toEqual([1 * MB, 2 * MB, 3 * MB]);
  });

  it("edge case: once at the 60-sample cap, the oldest reading drops instead of growing forever", () => {
    const full = Array.from({ length: 60 }, (_, i) => i);
    const h = pushMemSample(full, 999);
    expect(h.length).toBe(60);
    expect(h[0]).toBe(1); // sample 0 dropped
    expect(h[59]).toBe(999);
  });
});

describe("classifyProcessMem", () => {
  it("normal case: a reading close to the recent median is active", () => {
    const history = [290, 292, 288, 291, 289].map(mb => mb * MB);
    expect(classifyProcessMem(history, 293 * MB)).toBe("active");
  });

  it("edge case (the bug this guards against): a spike is never flagged before enough samples exist to judge it against", () => {
    const history = [100 * MB, 100 * MB]; // only 2 samples, below the 5-sample minimum
    expect(classifyProcessMem(history, 900 * MB)).toBe("active");
  });
});

describe("summarizeMemory", () => {
  it("normal case: sums a run's own bytes plus every node's bytes, and reports the largest single row for bar scaling", () => {
    const data: RunBreakdown[] = [
      run({
        live_bytes: 2 * MB,
        nodes: [
          { node_id: "n1", node_type_id: "http", started_at_ms: 0, live_bytes: 10 * MB },
          { node_id: "n2", node_type_id: "code", started_at_ms: 0, live_bytes: 3 * MB },
        ],
      }),
    ];
    const s = summarizeMemory(data);
    expect(s.totalBytes).toBe(15 * MB);
    expect(s.maxRowBytes).toBe(10 * MB); // the http node row, not the run-total
    expect(s.level).toBe("active"); // 15MB total is below the 50MB warn threshold, see boundary test below
  });

  it("edge case (the bug this guards against): empty snapshot is idle, not a fabricated zero total", () => {
    const s = summarizeMemory([]);
    expect(s.level).toBe("idle");
    expect(s.totalBytes).toBe(0);
    expect(s.maxRowBytes).toBe(1); // never 0 — callers divide by this
  });

  it("edge case: every row genuinely 0 bytes still yields a safe (non-zero) maxRowBytes", () => {
    const data: RunBreakdown[] = [run({ live_bytes: 0, nodes: [{ node_id: "n1", node_type_id: "http", started_at_ms: 0, live_bytes: 0 }] })];
    const s = summarizeMemory(data);
    expect(s.maxRowBytes).toBe(1);
    expect(s.level).toBe("active");
  });

  it("threshold boundaries: active <= 50MB, warn in (50MB, 150MB], high > 150MB", () => {
    expect(summarizeMemory([run({ live_bytes: 50 * MB })]).level).toBe("active");
    expect(summarizeMemory([run({ live_bytes: 50 * MB + 1 })]).level).toBe("warn");
    expect(summarizeMemory([run({ live_bytes: 150 * MB })]).level).toBe("warn");
    expect(summarizeMemory([run({ live_bytes: 150 * MB + 1 })]).level).toBe("high");
  });

  it("sums across multiple concurrent runs, not just the first", () => {
    const data: RunBreakdown[] = [run({ workflow_id: "a", live_bytes: 10 * MB }), run({ workflow_id: "b", live_bytes: 20 * MB })];
    expect(summarizeMemory(data).totalBytes).toBe(30 * MB);
  });
});

describe("formatBytes", () => {
  it("normal case: MB tier at/above 1 MB", () => {
    expect(formatBytes(1.5 * MB)).toBe("1.5 MB");
    expect(formatBytes(150 * MB)).toBe("150.0 MB");
  });

  it("edge case (the bug this guards against): small-but-real readings below 1MB no longer read as 0.0", () => {
    expect(formatBytes(50 * 1024)).toBe("50.0 KB"); // was "0.0 MB" pre-fix
    expect(formatBytes(512)).toBe("0.5 KB");
  });

  it("edge case: genuinely 0 bytes", () => {
    expect(formatBytes(0)).toBe("0.0 KB");
  });

  it("boundary: clearly-under-1MB stays KB, exactly 1MB is MB", () => {
    expect(formatBytes(1_040_000)).toBe("1015.6 KB"); // 1015.625 KB, nowhere near the rollover edge
    expect(formatBytes(MB)).toBe("1.0 MB");
  });

  it("boundary (the rounding bug this guards against): a value whose raw KB rounds up to 1024.0 rolls over to MB instead", () => {
    // 1,048,566 bytes = 1023.990234375 KB unrounded — rounds to 1024.0 at
    // 1 decimal, which must NOT display as "1024.0 KB". Values from
    // 1,048,535 up to MB-1 (1,048,575) all round to the same 1024.0 KB
    // pre-guard; 1,048,566 is just a representative point in that range.
    expect(formatBytes(1_048_566)).toBe("1.0 MB");
    expect(formatBytes(MB - 1)).toBe("1.0 MB");
  });

  // (performance Monitor Redesign): GB/TB tier extension. 
  // same guard pattern as the KB/MB tests above, repeated one and two tiers up.
  const GB = MB * 1024;
  const TB = GB * 1024;

  it("normal case: GB tier at/above 1 GB", () => {
    expect(formatBytes(1.5 * GB)).toBe("1.5 GB");
    expect(formatBytes(500 * GB)).toBe("500.0 GB");
  });

  it("normal case: TB tier at/above 1 TB", () => {
    expect(formatBytes(2.5 * TB)).toBe("2.5 TB");
  });

  it("boundary: clearly-under-1GB stays MB, exactly 1GB is GB", () => {
    expect(formatBytes(GB)).toBe("1.0 GB");
  });

  it("boundary (rounding bug, one tier up): a value whose raw MB rounds up to 1024.0 rolls over to GB instead", () => {
    // GB - 1 = 1,073,741,823 bytes = 1023.99999... MB unrounded — rounds
    // to 1024.0 at 1 decimal, must roll over to "1.0 GB", not "1024.0 MB".
    expect(formatBytes(GB - 1)).toBe("1.0 GB");
  });

  it("boundary (rounding bug, two tiers up): a value whose raw GB rounds up to 1024.0 rolls over to TB instead", () => {
    expect(formatBytes(TB - 1)).toBe("1.0 TB");
  });
});

