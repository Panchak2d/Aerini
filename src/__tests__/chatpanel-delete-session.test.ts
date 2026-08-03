/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

const wfMocks = vi.hoisted(() => ({ clearChatSession: vi.fn() }));
vi.mock("../ipc/workflow", () => ({
  startScheduledWorkflow: vi.fn(),
  getScheduledJobs: vi.fn().mockResolvedValue([]),
  parseSchedulerError: (raw: string) => {
    try { return JSON.parse(raw); } catch { return { error_kind: "other", message: raw }; }
  },
  clearChatSession: wfMocks.clearChatSession,
}));

import { ChatPanel } from "../panels/ChatPanel";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowManager } from "../workflow-manager";

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
    <div class="chat-banner hidden" id="chat-not-running-banner">
      <button class="btn-primary chat-banner-btn" id="btn-chat-start">Start</button>
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

function makeSession(id: string, name: string) {
  return { id, name, messages: [], createdAt: Date.now() };
}

function makePanel(): ChatPanel {
  document.body.innerHTML = CHAT_PANEL_HTML;
  const canvas = { nodes: new Map() } as unknown as Canvas;
  const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
  return new ChatPanel(canvas, wfManager, vi.fn());
}

beforeEach(() => {
  wfMocks.clearChatSession.mockReset();
  wfMocks.clearChatSession.mockResolvedValue(undefined);
});

describe("ChatPanel.deleteSession, orphaned AI-memory rows", () => {
  it("calls clearChatSession with the deleted session's id before removing it locally (normal case)", async () => {
    const panel = makePanel();
    const p = panel as unknown as { store: { sessions: unknown[]; activeId: string }; deleteSession(id: string): Promise<void> };
    p.store = { sessions: [makeSession("s1", "A"), makeSession("s2", "B")], activeId: "s1" };

    await p.deleteSession("s2");

    expect(wfMocks.clearChatSession).toHaveBeenCalledWith("s2");
    expect(wfMocks.clearChatSession).toHaveBeenCalledTimes(1);
    expect(p.store.sessions.map((s) => (s as { id: string }).id)).toEqual(["s1"]);
  });

  it("still deletes locally, with an error toast, when the backend clearChatSession call fails (edge case)", async () => {
    wfMocks.clearChatSession.mockRejectedValueOnce(new Error("network down"));
    const toastFn = vi.fn();
    document.body.innerHTML = CHAT_PANEL_HTML;
    const canvas = { nodes: new Map() } as unknown as Canvas;
    const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
    const panel = new ChatPanel(canvas, wfManager, toastFn);
    const p = panel as unknown as { store: { sessions: unknown[]; activeId: string }; deleteSession(id: string): Promise<void> };
    p.store = { sessions: [makeSession("s1", "A"), makeSession("s2", "B")], activeId: "s1" };

    await p.deleteSession("s2");

    expect(wfMocks.clearChatSession).toHaveBeenCalledWith("s2");
    expect(p.store.sessions.map((s) => (s as { id: string }).id)).toEqual(["s1"]);
    expect(toastFn).toHaveBeenCalledWith(expect.stringContaining("Could not clear AI memory"), "error");
  });

  it("refuses to delete the only remaining session and never calls clearChatSession", async () => {
    const panel = makePanel();
    const p = panel as unknown as { store: { sessions: unknown[]; activeId: string }; deleteSession(id: string): Promise<void> };
    p.store = { sessions: [makeSession("s1", "A")], activeId: "s1" };

    await p.deleteSession("s1");

    expect(wfMocks.clearChatSession).not.toHaveBeenCalled();
    expect(p.store.sessions.length).toBe(1);
  });
});
