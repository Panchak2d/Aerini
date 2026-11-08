/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";

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

function render(text: string): string {
  document.body.innerHTML = CHAT_PANEL_HTML;
  const canvas = { nodes: new Map() } as unknown as Canvas;
  const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
  const panel = new ChatPanel(canvas, wfManager, vi.fn(), runManagerStub);
  return (panel as unknown as { renderMarkdown(t: string): string }).renderMarkdown(text);
}

describe("ChatPanel markdown sanitising", () => {
  it("keeps absolute https links and formatting", () => {
    const html = render("**hi** [docs](https://github.com/Panchak2d/aerini)");
    expect(html).toContain("<strong>hi</strong>");
    expect(html).toContain('href="https://github.com/Panchak2d/aerini"');
  });

  it("drops relative, javascript:, file: and mailto: hrefs and form controls", () => {
    const html = render(
      "[a](/steal) [b](javascript:alert(1)) [c](file:///etc/passwd) [d](mailto:x@y.z)\n\n" +
        '<form action="https://evil.example"><input name="pw"><button>go</button></form>',
    );
    expect(html).not.toMatch(/href="(?!https?:)/);
    expect(html).not.toMatch(/<(form|input|button)/i);
  });
});
