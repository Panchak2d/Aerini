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
  clearChatSession: vi.fn(),
}));

vi.mock("../ipc/chat", () => ({
  listChatSessions: vi.fn().mockResolvedValue([]),
  saveChatSession: vi.fn(),
  deleteChatSession: vi.fn(),
}));

import { ChatPanel } from "../panels/ChatPanel";
import { DEFAULT_CHAT_SETTINGS } from "../canvas/CanvasSerializer";
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
    <div class="chat-banner hidden" id="chat-not-running-banner">
      <button class="btn-primary chat-banner-btn" id="btn-chat-start">Start</button>
    </div>
    <div class="chat-messages" id="chat-messages"></div>
    <div class="chat-input-row">
      <button class="chat-attach-btn" id="chat-attach-btn" data-tooltip="Attach file"></button>
      <textarea id="chat-input" class="chat-input" rows="1" disabled></textarea>
      <button class="chat-send-btn" id="chat-send-btn" disabled></button>
    </div>
    <div class="chat-branding-footer" id="chat-branding-footer">Built with Aerini</div>
  </div>
`;

function makePanel(): ChatPanel {
  document.body.innerHTML = CHAT_PANEL_HTML;
  const canvas = { nodes: new Map() } as unknown as Canvas;
  const wfManager = { currentId: "wf1", chatSettings: DEFAULT_CHAT_SETTINGS } as unknown as WorkflowManager;
  return new ChatPanel(canvas, wfManager, vi.fn(), runManagerStub);
}

function attachBtn(): HTMLButtonElement {
  return document.getElementById("chat-attach-btn") as HTMLButtonElement;
}

beforeEach(() => {
  vi.restoreAllMocks();
});

describe("ChatPanel — attach button discoverability", () => {
  it("normal case: gated off is never .hidden, and toggling the setting flips aria-disabled/tooltip", () => {
    const panel = makePanel();
    const btn = attachBtn();

    panel.applyToggles({ ...DEFAULT_CHAT_SETTINGS, allow_attachments: false });
    expect(btn.classList.contains("hidden")).toBe(false);
    expect(btn.getAttribute("aria-disabled")).toBe("true");
    expect(btn.getAttribute("data-tooltip")).toMatch(/off/i);

    panel.applyToggles({ ...DEFAULT_CHAT_SETTINGS, allow_attachments: true });
    expect(btn.classList.contains("hidden")).toBe(false);
    expect(btn.hasAttribute("aria-disabled")).toBe(false);
    expect(btn.getAttribute("data-tooltip")).toBe("Attach file");
  });

  it("edge case: clicking while gated off never opens the file picker", () => {
    const panel = makePanel();
    const clickSpy = vi.spyOn(HTMLInputElement.prototype, "click").mockImplementation(() => {});

    panel.applyToggles({ ...DEFAULT_CHAT_SETTINGS, allow_attachments: false });
    attachBtn().click();

    expect(clickSpy).not.toHaveBeenCalled();
  });
});
