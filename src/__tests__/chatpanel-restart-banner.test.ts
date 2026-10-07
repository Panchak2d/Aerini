/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), convertFileSrc: vi.fn((p: string) => p) }));
vi.mock("../ipc/workflow", () => ({
  startScheduledWorkflow: vi.fn().mockResolvedValue(undefined),
  stopScheduledWorkflow: vi.fn().mockResolvedValue(undefined),
  getScheduledJobs: vi.fn().mockResolvedValue([]),
  getScheduledJob: vi.fn().mockResolvedValue(null),
  parseSchedulerError: (raw: string) => ({ error_kind: "other", message: raw }),
  clearChatSession: vi.fn(),
  getSetting: vi.fn(),
  setSetting: vi.fn(),
}));
vi.mock("../ipc/chat", () => ({
  listChatSessionMeta: vi.fn().mockResolvedValue([]),
  loadChatMessages: vi.fn().mockResolvedValue([]),
  appendChatMessage: vi.fn(),
  saveChatSessionMeta: vi.fn(),
  saveChatSession: vi.fn(),
  deleteChatSession: vi.fn(),
}));
vi.mock("../confirm", () => ({ showConfirm: vi.fn() }));

import { ChatPanel } from "../panels/ChatPanel";
import { RunningSync } from "../running-sync";
import { showConfirm } from "../confirm";
import { stopScheduledWorkflow } from "../ipc/workflow";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowManager } from "../workflow-manager";
import type { RunManager } from "../run-manager";

const HTML = `
  <div id="chat-panel">
    <div class="chat-panel-header">
      <div id="chat-session-wrap"><button id="chat-session-btn"><span id="chat-session-label"></span></button><div id="chat-session-menu"></div></div>
      <button id="btn-chat-clear"></button><button id="btn-chat-close"></button>
    </div>
    <div class="chat-status hidden" id="chat-status"><p id="chat-status-text"></p><button id="btn-chat-start"></button></div>
    <div class="chat-messages" id="chat-messages"></div>
    <div class="chat-input-row">
      <button id="chat-attach-btn" disabled></button><textarea id="chat-input" disabled></textarea><button id="chat-send-btn" disabled></button>
    </div>
    <div id="chat-branding-footer"></div>
  </div>`;

const stored = (port: number) => JSON.stringify({ nodes: [{ id: "hook", node_type_id: "webhook", config: { port } }], edges: [] });
const flush = () => new Promise(r => setTimeout(r, 0));

type Internals = { schedulerRunning: boolean; syncReadiness(): void; applyReadinessFix(c: () => boolean): void; handleRestart(): Promise<void> };

async function setup(opts: { hookPort?: number; autoSaveFailed?: boolean } = {}) {
  document.body.innerHTML = HTML;
  const hook = { data: { id: "hook", name: "Webhook", node_type_id: "webhook", config: { port: opts.hookPort ?? 3456 } } };
  const canvas = { nodes: new Map([["hook", hook]]), connectors: new Map() } as unknown as Canvas;
  const wfManager = {
    currentId: "wf1", chatSettings: {}, autoSaveFailed: opts.autoSaveFailed ?? false,
    flushAutoSave: vi.fn().mockResolvedValue(true),
    prepareForBgRun: vi.fn().mockResolvedValue({ id: "wf1", name: "wf", json: "{}" }),
  } as unknown as WorkflowManager;
  const sync = new RunningSync(async () => stored(3456));
  sync.observe("wf1", "waiting");
  await flush();
  const panel = new ChatPanel(canvas, wfManager, vi.fn(), { addRunStateListener: vi.fn() } as unknown as RunManager, sync);
  const p = panel as unknown as Internals;
  p.schedulerRunning = true;
  return { p, sync, wfManager };
}

const text = () => document.querySelector(".chat-readiness")?.textContent ?? "";

describe("ChatPanel restart banner", () => {
  beforeEach(() => { vi.clearAllMocks(); });

  it("shows no Restart when only wiring changed on a running workflow", async () => {
    const { p } = await setup();
    p.syncReadiness();
    expect(text()).not.toContain("Restart");
  });

  it("offers Restart when the entry node's config differs from what the job started with", async () => {
    const { p } = await setup({ hookPort: 4000 });
    p.syncReadiness();
    expect(text()).toContain("trigger changed");
    expect(document.querySelector(".chat-readiness button")?.textContent).toBe("Restart");
  });

  it("warns, without a button, when autosave failed", async () => {
    const { p } = await setup({ autoSaveFailed: true });
    p.syncReadiness();
    expect(text()).toContain("aren't saved");
    expect(document.querySelector(".chat-readiness button")).toBeNull();
  });

  it("flushes autosave after a one-click fix and does not ask for a restart", async () => {
    const { p, wfManager } = await setup();
    p.applyReadinessFix(() => true);
    await flush();
    expect(wfManager.flushAutoSave).toHaveBeenCalledTimes(1);
    expect(text()).not.toContain("Restart");
  });

  it("leaves a run in flight alone when the user declines the restart", async () => {
    const { p, sync, wfManager } = await setup({ hookPort: 4000 });
    sync.observe("wf1", "running");
    vi.mocked(showConfirm).mockResolvedValue(false);
    await p.handleRestart();
    expect(stopScheduledWorkflow).not.toHaveBeenCalled();
    expect(wfManager.prepareForBgRun).not.toHaveBeenCalled();
  });

  it("saves before stopping, so a failed save keeps the job running", async () => {
    const { p, wfManager } = await setup({ hookPort: 4000 });
    vi.mocked(wfManager.prepareForBgRun).mockResolvedValue(null);
    await p.handleRestart();
    expect(stopScheduledWorkflow).not.toHaveBeenCalled();
  });
});
