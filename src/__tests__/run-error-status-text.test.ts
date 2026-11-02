/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import type { Canvas } from "../canvas/Canvas";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === "run_workflow") throw new Error("Node type 'feed_parser' is not registered");
    return null;
  }),
  convertFileSrc: vi.fn((p: string) => p),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

vi.mock("../canvas/CanvasSerializer", async (orig) => ({
  ...(await orig<typeof import("../canvas/CanvasSerializer")>()),
  serialize: () => "{}",
}));

import { RunManager } from "../run-manager";

describe("RunManager — status bar error text", () => {
  it("shows an engine error as plain text, not HTML-escaped", async () => {
    document.body.innerHTML = `<div id="output-drawer" class="hidden"><div id="drawer-tabs"></div><div id="output-content"></div></div>`;
    const canvas = {
      nodes: new Map([["n1", {}]]),
      connectors: new Map(),
      resetAllStatus: vi.fn(),
    } as unknown as Canvas;
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const onStatus = vi.fn();
    const rm = new RunManager(canvas, onStatus, vi.fn());
    rm.setCurrentWorkflow("wf-1", "First");

    await rm.handleRun("wf-1", "First").catch(() => {});

    const messages = onStatus.mock.calls.map(c => String(c[0]));
    expect(messages.some(m => m.startsWith("Error:") && m.includes("'feed_parser'"))).toBe(true);
    expect(messages.some(m => m.includes("&#39;"))).toBe(false);
  });
});
