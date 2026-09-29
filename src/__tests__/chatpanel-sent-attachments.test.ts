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
  getSetting: vi.fn().mockResolvedValue(null),
  setSetting: vi.fn().mockResolvedValue(undefined),
}));

const chatMocks = vi.hoisted(() => ({
  listChatSessionMeta: vi.fn(),
  loadChatMessages: vi.fn(),
  appendChatMessage: vi.fn(),
  saveChatSessionMeta: vi.fn(),
  saveChatSession: vi.fn(),
  deleteChatSession: vi.fn(),
}));
vi.mock("../ipc/chat", () => ({
  listChatSessionMeta: chatMocks.listChatSessionMeta,
  loadChatMessages: chatMocks.loadChatMessages,
  appendChatMessage: chatMocks.appendChatMessage,
  saveChatSessionMeta: chatMocks.saveChatSessionMeta,
  saveChatSession: chatMocks.saveChatSession,
  deleteChatSession: chatMocks.deleteChatSession,
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

type Attachment = { filename: string; data: string; mime_type: string };
type PanelInternals = {
  onSchedulerStatus(evt: SchedulerStatusEvent): void;
  handleSend(overrideText?: string): Promise<void>;
  loadStoreForCurrentWorkflow(): Promise<void>;
  renderMessages(): void;
  inputEl: HTMLTextAreaElement;
  pendingAttachments: Attachment[];
};

function makePanel(nodes?: Map<string, unknown>): PanelInternals {
  document.body.innerHTML = CHAT_PANEL_HTML;
  const canvas = { nodes: nodes ?? new Map() } as unknown as Canvas;
  const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
  const runManager = { addRunStateListener: vi.fn() } as unknown as RunManager;
  return new ChatPanel(canvas, wfManager, vi.fn(), runManager) as unknown as PanelInternals;
}

function webhookOnlyCanvas(): Map<string, unknown> {
  return new Map([["n1", { data: { node_type_id: "webhook", config: { port: 3456, path: "/webhook" } } }]]);
}

function waitingEvent(): SchedulerStatusEvent {
  return {
    workflow_id: "wf1", workflow_name: "wf", status: "waiting", run_count: 1,
    last_run_at: null, next_run_at: null, last_error: null, last_result: null,
  };
}

const SENT_CHIPS = "#chat-messages .chat-bubble--user .chat-attachment-chip";

beforeEach(() => {
  chatMocks.listChatSessionMeta.mockReset();
  chatMocks.loadChatMessages.mockReset().mockResolvedValue([]);
  chatMocks.appendChatMessage.mockReset().mockResolvedValue(undefined);
  chatMocks.saveChatSessionMeta.mockReset().mockResolvedValue(undefined);
  chatMocks.saveChatSession.mockReset().mockResolvedValue(undefined);
  chatMocks.deleteChatSession.mockReset().mockResolvedValue(undefined);
  localStorage.clear();
});

describe("ChatPanel — sent attachments in the message bubble", () => {
  beforeEach(() => { vi.useFakeTimers(); vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true })); });
  afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

  it("normal case: a sent attachment shows as a chip in the user's own bubble, is saved on the wire, and clears the pre-send strip", async () => {
    const att = { filename: "notes.pdf", data: "Yg==", mime_type: "application/pdf" };
    const p = makePanel(webhookOnlyCanvas());
    p.onSchedulerStatus(waitingEvent());
    p.inputEl.value = "see attached";
    p.pendingAttachments = [att];

    await p.handleSend();

    const chips = document.querySelectorAll(SENT_CHIPS);
    expect(chips).toHaveLength(1);
    expect(chips[0].textContent).toContain("notes.pdf");
    expect(chatMocks.appendChatMessage).toHaveBeenCalledWith(
      expect.objectContaining({ workflow_id: "wf1" }),
      expect.objectContaining({ role: "user", text: "see attached", attachments: [att] }),
    );
    expect(p.pendingAttachments).toEqual([]);
    expect(document.querySelectorAll(".chat-pending-attachments .chat-attachment-chip")).toHaveLength(0);
  });

  it("edge case: a message sent with no attachments renders no chip and puts no attachments on the wire", async () => {
    const p = makePanel(webhookOnlyCanvas());
    p.onSchedulerStatus(waitingEvent());
    p.inputEl.value = "just text";

    await p.handleSend();

    expect(document.querySelectorAll("#chat-messages .chat-attachment-chip")).toHaveLength(0);
    const saved = chatMocks.appendChatMessage.mock.calls[0][1];
    expect(saved.attachments).toBeUndefined();
  });
});

describe("ChatPanel — sent attachments after reload", () => {
  it("normal + edge case: stored attachments come back as chips (filename rendered as text, not markup); a null attachments field renders none", async () => {
    chatMocks.listChatSessionMeta.mockResolvedValue([{ id: "s1", workflow_id: "wf1", name: "A", created_at: 1_000 }]);
    chatMocks.loadChatMessages.mockResolvedValue([
      {
        id: "m1", role: "user", text: "look", images: null, timestamp: 1,
        attachments: [{ filename: "<img src=x onerror=alert(1)>.txt", data: "YQ==", mime_type: "text/plain" }],
      },
      { id: "m2", role: "user", text: "plain", images: null, attachments: null, timestamp: 2 },
    ]);
    const p = makePanel();

    await p.loadStoreForCurrentWorkflow();
    p.renderMessages();

    const chips = document.querySelectorAll(SENT_CHIPS);
    expect(chips).toHaveLength(1);
    expect(chips[0].textContent).toContain("<img src=x onerror=alert(1)>.txt");
    expect(chips[0].querySelector("img")).toBeNull();
  });
});
