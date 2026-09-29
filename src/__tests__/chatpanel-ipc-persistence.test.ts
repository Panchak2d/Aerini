/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

const wfMocks = vi.hoisted(() => ({
  clearChatSession: vi.fn(),
  getSetting: vi.fn(),
  setSetting: vi.fn(),
}));
vi.mock("../ipc/workflow", () => ({
  startScheduledWorkflow: vi.fn(),
  getScheduledJobs: vi.fn().mockResolvedValue([]),
  parseSchedulerError: (raw: string) => {
    try { return JSON.parse(raw); } catch { return { error_kind: "other", message: raw }; }
  },
  clearChatSession: wfMocks.clearChatSession,
  getSetting: wfMocks.getSetting,
  setSetting: wfMocks.setSetting,
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

type ChatStoreShape = { sessions: { id: string; name: string; messages: unknown[]; createdAt: number }[]; activeId: string };
type PanelInternals = {
  store: ChatStoreShape;
  loadStoreForCurrentWorkflow(): Promise<void>;
  persist(): Promise<void>;
  deleteSession(id: string): Promise<void>;
  handleClear(): Promise<void>;
};

function makeSession(id: string, name: string) {
  return { id, name, messages: [], createdAt: 1_000 };
}

function makePanel(): PanelInternals {
  document.body.innerHTML = CHAT_PANEL_HTML;
  const canvas = { nodes: new Map() } as unknown as Canvas;
  const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
  return new ChatPanel(canvas, wfManager, vi.fn(), runManagerStub) as unknown as PanelInternals;
}

beforeEach(() => {
  wfMocks.clearChatSession.mockReset().mockResolvedValue(undefined);
  wfMocks.getSetting.mockReset().mockResolvedValue(null);
  wfMocks.setSetting.mockReset().mockResolvedValue(undefined);
  chatMocks.listChatSessionMeta.mockReset();
  chatMocks.loadChatMessages.mockReset().mockResolvedValue([]);
  chatMocks.appendChatMessage.mockReset().mockResolvedValue(undefined);
  chatMocks.saveChatSessionMeta.mockReset().mockResolvedValue(undefined);
  chatMocks.saveChatSession.mockReset().mockResolvedValue(undefined);
  chatMocks.deleteChatSession.mockReset().mockResolvedValue(undefined);
  localStorage.clear();
});

describe("ChatPanel load/persist through the backend chat tables", () => {
  it("loadStoreForCurrentWorkflow lists session metadata and fetches only the active session's messages (normal case)", async () => {
    chatMocks.listChatSessionMeta.mockResolvedValue([
      { id: "s2", workflow_id: "wf1", name: "B", created_at: 2_000 },
      { id: "s1", workflow_id: "wf1", name: "A", created_at: 1_000 },
    ]);
    chatMocks.loadChatMessages.mockResolvedValue([{ id: "m1", role: "user", text: "hi", timestamp: 5 }]);
    wfMocks.getSetting.mockResolvedValue("s1");
    const p = makePanel();

    await p.loadStoreForCurrentWorkflow();

    expect(chatMocks.listChatSessionMeta).toHaveBeenCalledWith("wf1");
    expect(chatMocks.loadChatMessages).toHaveBeenCalledTimes(1);
    expect(chatMocks.loadChatMessages).toHaveBeenCalledWith("s1");
    expect(p.store.sessions.map((s) => s.id)).toEqual(["s2", "s1"]);
    expect(p.store.activeId).toBe("s1");
    expect(p.store.sessions[1].messages).toHaveLength(1);
    expect(p.store.sessions[0].messages).toEqual([]);
  });

  it("does not write back what it just loaded (edge case)", async () => {
    chatMocks.listChatSessionMeta.mockResolvedValue([{ id: "s1", workflow_id: "wf1", name: "A", created_at: 1_000 }]);
    chatMocks.loadChatMessages.mockResolvedValue([{ id: "m1", role: "user", text: "hi", timestamp: 5 }]);
    wfMocks.getSetting.mockResolvedValue("s1");
    const p = makePanel();

    await p.loadStoreForCurrentWorkflow();
    await p.persist();

    expect(chatMocks.appendChatMessage).not.toHaveBeenCalled();
    expect(chatMocks.saveChatSessionMeta).not.toHaveBeenCalled();
  });

  it("migrates a legacy localStorage blob when the backend has no sessions yet, then clears the old key (edge case)", async () => {
    chatMocks.listChatSessionMeta.mockResolvedValue([]);
    const legacy = { sessions: [makeSession("s1", "Old Session")], activeId: "s1" };
    localStorage.setItem("aerini_chat_wf1", JSON.stringify(legacy));
    const p = makePanel();

    await p.loadStoreForCurrentWorkflow();

    expect(chatMocks.saveChatSession).toHaveBeenCalledWith(
      expect.objectContaining({ id: "s1", workflow_id: "wf1", name: "Old Session" }),
    );
    expect(wfMocks.setSetting).toHaveBeenCalledWith("chat_active_session:wf1", "s1");
    expect(localStorage.getItem("aerini_chat_wf1")).toBeNull();
    expect(p.store.sessions.map((s) => s.id)).toEqual(["s1"]);
  });

  it("starts with an empty store when the backend has no sessions and there is no legacy data", async () => {
    chatMocks.listChatSessionMeta.mockResolvedValue([]);
    const p = makePanel();

    await p.loadStoreForCurrentWorkflow();

    expect(chatMocks.saveChatSession).not.toHaveBeenCalled();
    expect(p.store.sessions).toEqual([]);
  });

  it("persist() appends each unsynced message once, then only records activeId (normal + edge case)", async () => {
    const p = makePanel();
    p.store = { sessions: [{ ...makeSession("s1", "A"), messages: [{ id: "m1", role: "user", text: "hi", timestamp: 5 }] }], activeId: "s1" };

    await p.persist();

    expect(chatMocks.appendChatMessage).toHaveBeenCalledTimes(1);
    expect(chatMocks.appendChatMessage).toHaveBeenCalledWith(
      { id: "s1", workflow_id: "wf1", name: "A", created_at: 1_000 },
      { id: "m1", role: "user", text: "hi", images: undefined, attachments: undefined, timestamp: 5 },
    );
    expect(chatMocks.saveChatSession).not.toHaveBeenCalled();
    expect(wfMocks.setSetting).toHaveBeenCalledWith("chat_active_session:wf1", "s1");

    chatMocks.appendChatMessage.mockClear();
    p.store.sessions[0].messages.push({ id: "m2", role: "ai", text: "yo", timestamp: 6 });
    await p.persist();
    expect(chatMocks.appendChatMessage.mock.calls.map((c) => c[1].id)).toEqual(["m2"]);
  });

  it("persist() saves a brand-new empty session's metadata once, without any message call (edge case)", async () => {
    const p = makePanel();
    (p as unknown as { createNewSession(): void }).createNewSession();

    await p.persist();
    await p.persist();

    expect(chatMocks.saveChatSessionMeta).toHaveBeenCalledTimes(1);
    expect(chatMocks.saveChatSessionMeta).toHaveBeenCalledWith(
      expect.objectContaining({ workflow_id: "wf1", id: p.store.activeId }),
    );
    expect(chatMocks.appendChatMessage).not.toHaveBeenCalled();
  });

  it("persist() keeps writing under the workflow the store was loaded for after the workflow changes (edge case)", async () => {
    chatMocks.listChatSessionMeta.mockResolvedValue([{ id: "s1", workflow_id: "wf1", name: "A", created_at: 1_000 }]);
    wfMocks.getSetting.mockResolvedValue("s1");
    const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
    document.body.innerHTML = CHAT_PANEL_HTML;
    const p = new ChatPanel({ nodes: new Map() } as unknown as Canvas, wfManager, vi.fn(), runManagerStub) as unknown as PanelInternals;
    await p.loadStoreForCurrentWorkflow();
    p.store.sessions[0].messages.push({ id: "late", role: "ai", text: "x", timestamp: 9 });

    (wfManager as { currentId: string }).currentId = "wf2";
    await p.persist();

    expect(chatMocks.appendChatMessage).toHaveBeenCalledWith(expect.objectContaining({ workflow_id: "wf1" }), expect.anything());
    expect(wfMocks.setSetting).toHaveBeenCalledWith("chat_active_session:wf1", "s1");
    expect(wfMocks.setSetting).not.toHaveBeenCalledWith("chat_active_session:wf2", expect.anything());
  });

  it("deleteSession also deletes the removed session's backend row, not just the AI-memory clear", async () => {
    const p = makePanel();
    p.store = { sessions: [makeSession("s1", "A"), makeSession("s2", "B")], activeId: "s1" };

    await p.deleteSession("s2");

    expect(wfMocks.clearChatSession).toHaveBeenCalledWith("s2");
    expect(chatMocks.deleteChatSession).toHaveBeenCalledWith("s2");
  });

  it("handleClear deletes the old session's backend row since the replacement gets a new id", async () => {
    const p = makePanel();
    p.store = { sessions: [makeSession("s1", "A")], activeId: "s1" };

    await p.handleClear();

    expect(wfMocks.clearChatSession).toHaveBeenCalledWith("s1");
    expect(chatMocks.deleteChatSession).toHaveBeenCalledWith("s1");
    expect(p.store.sessions[0].id).not.toBe("s1");
  });
});
