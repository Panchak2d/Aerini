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

const chatMocks = vi.hoisted(() => ({ deleteChatSession: vi.fn() }));
vi.mock("../ipc/chat", () => ({
  listChatSessionMeta: vi.fn().mockResolvedValue([]),
  loadChatMessages: vi.fn().mockResolvedValue([]),
  appendChatMessage: vi.fn(),
  saveChatSessionMeta: vi.fn(),
  saveChatSession: vi.fn(),
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

function makeSession(id: string, name: string) {
  return { id, name, messages: [], createdAt: Date.now() };
}

type PanelWithStore = { store: { sessions: unknown[]; activeId: string } };

function makePanel(): ChatPanel {
  document.body.innerHTML = CHAT_PANEL_HTML;
  const canvas = { nodes: new Map() } as unknown as Canvas;
  const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
  const panel = new ChatPanel(canvas, wfManager, vi.fn(), runManagerStub);
  (panel as unknown as PanelWithStore).store = { sessions: [makeSession("s1", "A")], activeId: "s1" };
  return panel;
}

function clearBtn(): HTMLButtonElement {
  return document.getElementById("btn-chat-clear") as HTMLButtonElement;
}

beforeEach(() => {
  wfMocks.clearChatSession.mockReset().mockResolvedValue(undefined);
  chatMocks.deleteChatSession.mockReset().mockResolvedValue(undefined);
});

describe("ChatPanel — Clear button loading state", () => {
  it("normal case: disables and relabels the button while clearing, restores both on completion", async () => {
    makePanel();
    const btn = clearBtn();

    btn.click();
    expect(btn.disabled).toBe(true); // mid-flight, before clearChatSession resolves
    expect(btn.textContent).toBe("Clearing…");
    await vi.waitFor(() => expect(btn.disabled).toBe(false));

    expect(wfMocks.clearChatSession).toHaveBeenCalledWith("s1");
    expect(chatMocks.deleteChatSession).toHaveBeenCalledWith("s1");
    expect(btn.textContent).toBe("Clear");
  });

  it("edge case: backend clearChatSession fails — button still restores instead of staying stuck disabled", async () => {
    wfMocks.clearChatSession.mockRejectedValueOnce(new Error("network down"));
    makePanel();
    const btn = clearBtn();

    btn.click();
    await vi.waitFor(() => expect(btn.disabled).toBe(false));

    expect(btn.textContent).toBe("Clear");
  });
});
