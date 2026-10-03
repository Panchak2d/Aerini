/* @vitest-environment jsdom */
import { describe, it, expect } from "vitest";
import {
  getBgJobs,
  hydrateBgJobsFromScheduler,
  updateBgJobStoreFromEvent,
} from "../run-manager";

const job = (id: string) => getBgJobs().find(j => j.id === id);

function event(id: string, over: Record<string, unknown> = {}) {
  return {
    workflow_id: id, workflow_name: id, status: "waiting", run_count: 0,
    last_run_at: null, next_run_at: null, last_error: null, ...over,
  };
}

describe("BgJob triggerType from scheduler events", () => {
  it("is set from the event, survives an event without it, and ignores an unrecognised value", () => {
    updateBgJobStoreFromEvent(event("ev-1", { trigger_type: "webhook" }));
    expect(job("ev-1")?.triggerType).toBe("webhook");

    updateBgJobStoreFromEvent(event("ev-1", { status: "running" }));
    updateBgJobStoreFromEvent(event("ev-1", { trigger_type: null }));
    expect(job("ev-1")?.triggerType).toBe("webhook");

    updateBgJobStoreFromEvent(event("ev-1", { trigger_type: "carrier-pigeon" }));
    expect(job("ev-1")?.triggerType).toBe("webhook");
  });

  it("a stopped event clears nextRunAt, and a later running event does not bring it back", () => {
    updateBgJobStoreFromEvent(event("ev-2", { trigger_type: "interval", next_run_at: "2026-10-02T10:00:00Z" }));
    expect(job("ev-2")?.nextRunAt).toBe("2026-10-02T10:00:00Z");

    updateBgJobStoreFromEvent(event("ev-2", { status: "stopped" }));
    expect(job("ev-2")?.nextRunAt).toBeUndefined();

    updateBgJobStoreFromEvent(event("ev-2", { status: "running" }));
    expect(job("ev-2")?.nextRunAt).toBeUndefined();
  });
});

describe("BgJob waiting from scheduler events", () => {
  it("follows each event: true on waiting, false on running, done and stopped, true again on the next waiting", () => {
    updateBgJobStoreFromEvent(event("wt-1", { status: "waiting", trigger_type: "webhook" }));
    expect(job("wt-1")?.waiting).toBe(true);

    updateBgJobStoreFromEvent(event("wt-1", { status: "running" }));
    expect(job("wt-1")?.waiting).toBe(false);
    expect(job("wt-1")?.status).toBe("running");

    updateBgJobStoreFromEvent(event("wt-1", { status: "waiting" }));
    expect(job("wt-1")?.waiting).toBe(true);
    expect(job("wt-1")?.status).toBe("running");

    updateBgJobStoreFromEvent(event("wt-1", { status: "stopped" }));
    expect(job("wt-1")?.waiting).toBe(false);
  });
});

describe("BgJob triggerType from hydrated scheduler rows", () => {
  const row = (id: string, trigger_kind: string, status = "active") => ({
    workflow_id: id, workflow_name: id, status, run_count: 0,
    last_run_at: null, last_error: null, next_run_at: null, trigger_kind,
  });

  it("reads the kind tag from the JSON-encoded trigger, and leaves it unset for malformed or tagless JSON", () => {
    hydrateBgJobsFromScheduler([
      row("hy-webhook", JSON.stringify({ kind: "webhook", port: 1, path: "/", method: "POST", secret: "<redacted>" })),
      row("hy-bad", "not json"),
      row("hy-null", "null"),
      row("hy-empty", "{}"),
    ]);
    expect(job("hy-webhook")?.triggerType).toBe("webhook");
    expect(job("hy-bad")?.triggerType).toBeUndefined();
    expect(job("hy-null")?.triggerType).toBeUndefined();
    expect(job("hy-empty")?.triggerType).toBeUndefined();
  });

  it("does not overwrite a live running job, and keeps its event-supplied triggerType", () => {
    updateBgJobStoreFromEvent(event("hy-live", { status: "running", trigger_type: "plugin" }));
    hydrateBgJobsFromScheduler([row("hy-live", JSON.stringify({ kind: "cron", expr: "* * * * *" }))]);
    expect(job("hy-live")?.triggerType).toBe("plugin");
  });

  it("leaves waiting unset for a row it creates and does not touch a live job's value", () => {
    hydrateBgJobsFromScheduler([row("hy-new", JSON.stringify({ kind: "webhook" }))]);
    expect(job("hy-new")?.waiting).toBeUndefined();

    updateBgJobStoreFromEvent(event("hy-wait", { status: "waiting", trigger_type: "webhook" }));
    hydrateBgJobsFromScheduler([row("hy-wait", JSON.stringify({ kind: "webhook" }))]);
    expect(job("hy-wait")?.waiting).toBe(true);
  });
});
