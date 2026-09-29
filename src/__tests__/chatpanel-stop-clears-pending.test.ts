/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

vi.mock("../ipc/workflow", () => ({
  startScheduledWorkflow: vi.fn(),
  getScheduledJobs: vi.fn().mockResolvedValue([]),
  getScheduledJob: vi.fn().mockResolvedValue(null),
  parseSchedulerError: (raw: string) => {
    try { return JSON.parse(raw); } catch { return { error_kind: "other", message: raw }; }
  },
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

import { ChatPanel } from "../panels/ChatPanel";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowManager } from "../workflow-manager";
import type { RunManager } from "../run-manager";
import type { SchedulerStatusEvent } from "../ipc/events";

const CHAT_PANEL_HTML = `
  <div id="chat-panel">
    <div class="chat-panel-header">
      <div class="chat-session-wrap" id="chat-session-wrap">
        <button class="chat-session-btn" id="chat-session-btn"><span id="chat-session-label">Session</span></button>
        <div class="toolbar-dropdown-menu" id="chat-session-menu"></div>
      </div>
      <button class="drawer-btn" id="btn-chat-clear">Clear</button>
      <button class="drawer-btn" id="btn-chat-close">×</button>
    </div>
    <div class="chat-status hidden" id="chat-status" data-layout="banner">
      <p class="chat-status-text" id="chat-status-text">Start this workflow to begin chatting</p>
      <button class="btn-primary chat-status-btn" id="btn-chat-start">Start</button>
    </div>
    <div class="chat-messages" id="chat-messages"></div>
    <div class="chat-input-row">
      <button class="chat-attach-btn hidden" id="chat-attach-btn" disabled></button>
      <textarea id="chat-input" class="chat-input" rows="1" disabled></textarea>
      <button class="chat-send-btn" id="chat-send-btn" disabled></button>
    </div>
    <div class="chat-branding-footer" id="chat-branding-footer">Built with Aerini</div>
  </div>
`;

type PanelInternals = {
  onSchedulerStatus(evt: SchedulerStatusEvent): void;
  handleSend(overrideText?: string): Promise<void>;
  replyPending: boolean;
  awaitingReply: boolean;
};

function makePanel(): PanelInternals {
  document.body.innerHTML = CHAT_PANEL_HTML;
  const canvas = {
    nodes: new Map([["n1", { data: { node_type_id: "webhook", config: { port: 3456, path: "/webhook" } } }]]),
  } as unknown as Canvas;
  const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
  const runManager = { addRunStateListener: vi.fn() } as unknown as RunManager;
  return new ChatPanel(canvas, wfManager, vi.fn(), runManager) as unknown as PanelInternals;
}

function waitingEvent(): SchedulerStatusEvent {
  return {
    workflow_id: "wf1", workflow_name: "wf", status: "waiting", run_count: 1,
    last_run_at: null, next_run_at: null, last_error: null, last_result: null,
  };
}

function stoppedEvent(): SchedulerStatusEvent {
  return {
    workflow_id: "wf1", workflow_name: "wf", status: "stopped", run_count: 1,
    last_run_at: null, next_run_at: null, last_error: null, last_result: null,
  };
}

describe("ChatPanel — a user-initiated stop resolves a pending message", () => {
  beforeEach(() => { vi.useFakeTimers(); vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true })); });
  afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

  it("normal case: 'stopped' while a reply is pending clears it immediately, with no error bubble", async () => {
    const p = makePanel();
    p.onSchedulerStatus(waitingEvent());
    await p.handleSend("hello");
    expect(document.getElementById("chat-typing-row")).not.toBeNull();

    p.onSchedulerStatus(stoppedEvent());

    expect(document.getElementById("chat-typing-row")).toBeNull();
    expect(document.querySelector(".chat-bubble--error")).toBeNull();
    expect(p.replyPending).toBe(false);
    expect(p.awaitingReply).toBe(false);
  });

  it("edge case: the original 30s timeout never fires once 'stopped' has already cleared the pending message", async () => {
    const p = makePanel();
    p.onSchedulerStatus(waitingEvent());
    await p.handleSend("hello");

    p.onSchedulerStatus(stoppedEvent());
    vi.advanceTimersByTime(30_000);

    expect(document.querySelector(".chat-bubble--error")).toBeNull();
  });
});
