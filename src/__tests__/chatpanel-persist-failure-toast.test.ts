/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

vi.mock("../ipc/workflow", () => ({
  startScheduledWorkflow: vi.fn(),
  getScheduledJobs: vi.fn().mockResolvedValue([]),
  parseSchedulerError: (raw: string) => {
    try { return JSON.parse(raw); } catch { return { error_kind: "other", message: raw }; }
  },
  clearChatSession: vi.fn().mockResolvedValue(undefined),
  getSetting: vi.fn().mockResolvedValue(null),
  setSetting: vi.fn().mockResolvedValue(undefined),
}));

const chatMocks = vi.hoisted(() => ({ appendChatMessage: vi.fn() }));
vi.mock("../ipc/chat", () => ({
  listChatSessionMeta: vi.fn().mockResolvedValue([]),
  loadChatMessages: vi.fn().mockResolvedValue([]),
  appendChatMessage: chatMocks.appendChatMessage,
  saveChatSessionMeta: vi.fn(),
  saveChatSession: vi.fn(),
  deleteChatSession: vi.fn().mockResolvedValue(undefined),
}));

import { ChatPanel } from "../panels/ChatPanel";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowManager } from "../workflow-manager";
import type { RunManager } from "../run-manager";

const runManagerStub = { addRunStateListener: vi.fn() } as unknown as RunManager;

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

function makePanel(toastFn: Toast): ChatPanel {
  document.body.innerHTML = CHAT_PANEL_HTML;
  const canvas = { nodes: new Map() } as unknown as Canvas;
  const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
  return new ChatPanel(canvas, wfManager, toastFn, runManagerStub);
}

type Toast = (msg: string, type?: "success" | "error" | "info") => void;
type Msg = { id: string; role: string; text: string; timestamp: number };
type PanelWithPersist = {
  persist(): Promise<void>;
  store: { sessions: { id: string; name: string; createdAt: number; messages: Msg[] }[]; activeId: string };
};

function panelWithMessages(toastFn: Toast, ids: string[]): PanelWithPersist {
  const p = makePanel(toastFn) as unknown as PanelWithPersist;
  p.store = {
    sessions: [{ id: "s1", name: "A", createdAt: 1_000, messages: ids.map((id, i) => ({ id, role: "user", text: id, timestamp: i + 1 })) }],
    activeId: "s1",
  };
  return p;
}

beforeEach(() => {
  chatMocks.appendChatMessage.mockReset();
});

describe("ChatPanel persist() failure toast", () => {
  it("toasts once for a run of repeated failures, not per call (normal case)", async () => {
    chatMocks.appendChatMessage.mockRejectedValue(new Error("backend unreachable"));
    const toastFn = vi.fn();
    const p = panelWithMessages(toastFn, ["m1"]);

    await p.persist();
    await p.persist();
    await p.persist();

    expect(toastFn).toHaveBeenCalledTimes(1);
    expect(toastFn).toHaveBeenCalledWith(expect.stringContaining("isn't saving"), "error");
  });

  it("clears the one-shot flag on the next successful persist(), so a later failure toasts again (edge case)", async () => {
    chatMocks.appendChatMessage.mockRejectedValueOnce(new Error("backend unreachable"));
    const toastFn = vi.fn();
    const p = panelWithMessages(toastFn, ["m1"]);

    await p.persist();
    expect(toastFn).toHaveBeenCalledTimes(1);

    chatMocks.appendChatMessage.mockResolvedValueOnce(undefined);
    await p.persist();

    p.store.sessions[0].messages.push({ id: "m2", role: "user", text: "m2", timestamp: 9 });
    chatMocks.appendChatMessage.mockRejectedValueOnce(new Error("backend unreachable"));
    await p.persist();

    expect(toastFn).toHaveBeenCalledTimes(2);
  });

  it("retries only the message that failed on the next persist(), never re-sending saved ones (no loss, no duplicates)", async () => {
    const toastFn = vi.fn();
    const p = panelWithMessages(toastFn, ["m1", "m2", "m3"]);
    chatMocks.appendChatMessage
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error("backend unreachable"));

    await p.persist();
    const sentFirst = chatMocks.appendChatMessage.mock.calls.map((c) => c[1].id);
    expect(sentFirst).toEqual(["m1", "m2"]);

    chatMocks.appendChatMessage.mockReset().mockResolvedValue(undefined);
    await p.persist();
    expect(chatMocks.appendChatMessage.mock.calls.map((c) => c[1].id)).toEqual(["m2", "m3"]);

    chatMocks.appendChatMessage.mockClear();
    await p.persist();
    expect(chatMocks.appendChatMessage).not.toHaveBeenCalled();
  });
});
