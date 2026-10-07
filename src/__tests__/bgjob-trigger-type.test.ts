// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { getBgJobs, updateBgJobStoreFromEvent } from "../run-manager";

const job = (id: string) => getBgJobs().find(j => j.id === id);

function event(id: string, over: Record<string, unknown> = {}) {
  return {
    workflow_id: id, workflow_name: id, status: "waiting", run_count: 0,
    last_run_at: null, next_run_at: null, last_error: null, ...over,
  };
}

describe("BgJob triggerType from scheduler events", () => {
  it("normal case: is set from the event's trigger_type and replaced by a later one", () => {
    updateBgJobStoreFromEvent(event("tt-set", { trigger_type: "webhook" }));
    expect(job("tt-set")?.triggerType).toBe("webhook");

    updateBgJobStoreFromEvent(event("tt-set", { trigger_type: "plugin" }));
    expect(job("tt-set")?.triggerType).toBe("plugin");
  });

  it("edge case: an event with a null or absent trigger_type keeps the known value", () => {
    updateBgJobStoreFromEvent(event("tt-keep", { trigger_type: "interval" }));

    updateBgJobStoreFromEvent(event("tt-keep", { status: "running", trigger_type: null }));
    expect(job("tt-keep")?.triggerType).toBe("interval");

    updateBgJobStoreFromEvent(event("tt-keep", { status: "running" }));
    expect(job("tt-keep")?.triggerType).toBe("interval");
  });
});
