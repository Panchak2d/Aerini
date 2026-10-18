import { describe, it, expect } from "vitest";
import { RunStateMachine } from "../run-manager/state-machine";

describe("RunStateMachine cancel marks", () => {
  it("a cancelled run is reported once and only for its own id", () => {
    const s = new RunStateMachine();
    s.start("run_a");
    s.requestCancel();
    expect(s.consumeCancelRequest("run_a")).toBe(true);
    expect(s.consumeCancelRequest("run_a")).toBe(false);
  });

  it("a cancel whose run never consumed it does not discard the next run", () => {
    const s = new RunStateMachine();
    s.start("run_a");
    s.requestCancel();
    s.stop();
    s.start("run_b");
    expect(s.consumeCancelRequest("run_b")).toBe(false);
  });

  it("requesting a cancel while idle marks nothing", () => {
    const s = new RunStateMachine();
    s.requestCancel();
    s.start("run_c");
    expect(s.consumeCancelRequest("run_c")).toBe(false);
  });
});
