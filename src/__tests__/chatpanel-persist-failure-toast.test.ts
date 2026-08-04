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

const chatMocks = vi.hoisted(() => ({ saveChatSession: vi.fn() }));
vi.mock("../ipc/chat", () => ({
  listChatSessions: vi.fn().mockResolvedValue([]),
  saveChatSession: chatMocks.saveChatSession,
  deleteChatSession: vi.fn().mockResolvedValue(undefined),
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

function makePanel(toastFn: Toast): ChatPanel {
  document.body.innerHTML = CHAT_PANEL_HTML;
  const canvas = { nodes: new Map() } as unknown as Canvas;
  const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
  return new ChatPanel(canvas, wfManager, toastFn);
}

type Toast = (msg: string, type?: "success" | "error" | "info") => void;
type PanelWithPersist = { persist(): Promise<void> };

beforeEach(() => {
  chatMocks.saveChatSession.mockReset();
});

describe("ChatPanel persist() failure toast", () => {
  it("toasts once for a run of repeated failures, not per call (normal case)", async () => {
    chatMocks.saveChatSession.mockRejectedValue(new Error("backend unreachable"));
    const toastFn = vi.fn();
    const p = makePanel(toastFn) as unknown as PanelWithPersist;

    await p.persist();
    await p.persist();
    await p.persist();

    expect(toastFn).toHaveBeenCalledTimes(1);
    expect(toastFn).toHaveBeenCalledWith(expect.stringContaining("isn't saving"), "error");
  });

  it("clears the one-shot flag on the next successful persist(), so a later failure toasts again (edge case)", async () => {
    chatMocks.saveChatSession.mockRejectedValueOnce(new Error("backend unreachable"));
    const toastFn = vi.fn();
    const p = makePanel(toastFn) as unknown as PanelWithPersist;

    await p.persist();
    expect(toastFn).toHaveBeenCalledTimes(1);

    chatMocks.saveChatSession.mockResolvedValueOnce(undefined);
    await p.persist();

    chatMocks.saveChatSession.mockRejectedValueOnce(new Error("backend unreachable"));
    await p.persist();

    expect(toastFn).toHaveBeenCalledTimes(2);
  });
});
