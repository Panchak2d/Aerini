import { describe, it, expect, vi } from "vitest";
import { triggerFingerprint, fingerprintFromJson, canvasTriggerFingerprint, RunningSync } from "../running-sync";
import { syncSignal } from "../sync-chip";

const n = (id: string, type: string, config: unknown = {}) => ({ id, node_type_id: type, config });
const e = (from: string, to: string) => ({ from_node: from, to_node: to });
const flush = () => new Promise(r => setTimeout(r, 0));

describe("triggerFingerprint", () => {
  const nodes = [n("hook", "webhook", { port: 3456, path: "/a" }), n("ai", "ai_prompt", { model: "x" })];

  it("ignores wiring, downstream config, and key order of the entry config", () => {
    const base = triggerFingerprint(nodes, [e("hook", "ai")]);
    expect(triggerFingerprint(nodes, [])).not.toBeNull();
    const edited = [n("hook", "webhook", { path: "/a", port: 3456 }), n("ai", "ai_prompt", { model: "y" })];
    expect(triggerFingerprint(edited, [e("hook", "ai")])).toBe(base);
  });

  it("changes when the entry node's config or type changes", () => {
    const base = triggerFingerprint(nodes, [e("hook", "ai")]);
    expect(triggerFingerprint([n("hook", "webhook", { port: 4000, path: "/a" }), nodes[1]], [e("hook", "ai")])).not.toBe(base);
    expect(triggerFingerprint([n("hook", "schedule", {}), nodes[1]], [e("hook", "ai")])).not.toBe(base);
  });

  it("returns null with no nodes, skips notes, and ignores edges to missing nodes", () => {
    expect(triggerFingerprint([], [])).toBeNull();
    expect(triggerFingerprint([n("hook", "webhook")], [e("ghost", "hook")])).toBe(triggerFingerprint([n("hook", "webhook")], []));
  });
});

describe("fingerprintFromJson / canvasTriggerFingerprint", () => {
  it("matches the canvas form for the same graph and tolerates bad JSON", () => {
    const json = JSON.stringify({ nodes: [n("a", "webhook", { port: 1 })], edges: [] });
    const canvas = { nodes: new Map([["a", { data: n("a", "webhook", { port: 1 }) }]]), connectors: new Map() };
    expect(canvasTriggerFingerprint(canvas)).toBe(fingerprintFromJson(json));
    expect(fingerprintFromJson("{nope")).toBeNull();
  });
});

describe("RunningSync", () => {
  const stored = JSON.stringify({ nodes: [n("a", "webhook", { port: 1 })], edges: [] });
  const same = triggerFingerprint([n("a", "webhook", { port: 1 })], []);
  const changed = triggerFingerprint([n("a", "webhook", { port: 2 })], []);

  it("snapshots the stored row once and flags only a trigger difference", async () => {
    const load = vi.fn().mockResolvedValue(stored);
    const s = new RunningSync(load);
    s.observe("w", "running"); s.observe("w", "waiting");
    await flush();
    expect(load).toHaveBeenCalledTimes(1);
    expect(s.triggerChanged("w", same)).toBe(false);
    expect(s.triggerChanged("w", changed)).toBe(true);
  });

  it("keeps the baseline after a failed run but drops it on stop", async () => {
    const s = new RunningSync(async () => stored);
    s.observe("w", "running"); await flush();
    s.observe("w", "error");
    expect(s.isRunInFlight("w")).toBe(false);
    expect(s.triggerChanged("w", changed)).toBe(true);
    s.observe("w", "stopped");
    expect(s.triggerChanged("w", changed)).toBe(false);
  });

  it("tracks in-flight runs and survives a failed load", async () => {
    const s = new RunningSync(async () => { throw new Error("db"); });
    s.observe("w", "running"); await flush();
    expect(s.isRunInFlight("w")).toBe(true);
    s.observe("w", "waiting");
    expect(s.isRunInFlight("w")).toBe(false);
    expect(s.triggerChanged("w", changed)).toBe(false);
  });
});

describe("syncSignal", () => {
  it("prioritises a failed save over a trigger change and needs a live job for the save warning", () => {
    expect(syncSignal(true, true, true)).toBe("autosave_failed");
    expect(syncSignal(true, false, false)).toBeNull();
    expect(syncSignal(false, true, true)).toBe("trigger_changed");
  });
});
