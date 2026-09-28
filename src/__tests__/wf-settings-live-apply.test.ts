/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { bindWfSettings } from "../wf-settings";
import { DEFAULT_CHAT_SETTINGS } from "../canvas/CanvasSerializer";
import type { IChatPanel } from "../toolbar";
import type { WorkflowManager } from "../workflow-manager";

const WF_SETTINGS_HTML = `
  <button id="btn-wf-settings"></button>
  <div id="wf-settings-modal" class="hidden">
    <button id="btn-close-wf-settings"></button>
    <input id="wf-setting-parallel" type="checkbox" />
    <div id="wf-setting-concurrency-row"></div>
    <input id="wf-setting-max-concurrent" type="number" />
    <input id="wf-setting-unlimited-duration" type="checkbox" />
    <input id="wf-setting-max-duration" type="number" />
    <div id="wf-setting-max-duration-row"></div>
    <input id="wf-setting-chat-attachments" type="checkbox" />
    <input id="wf-setting-chat-images" type="checkbox" />
    <input id="wf-setting-chat-max-length" type="number" />
    <input id="wf-setting-chat-persistence" type="checkbox" />
    <input id="wf-setting-chat-branding" type="checkbox" />
    <div id="wf-setting-tags-chips"></div>
    <input id="wf-setting-tags-input" />
  </div>
`;

function stubChatPanel(): IChatPanel {
  return {
    toggle: vi.fn(),
    refreshButtonVisibility: vi.fn(),
    onWorkflowSwitched: vi.fn(),
    isOpen: vi.fn().mockReturnValue(true),
    startForChat: vi.fn(),
    applyToggles: vi.fn(),
  };
}

function stubWfManager(): WorkflowManager {
  return {
    currentTags: [],
    chatSettings: { ...DEFAULT_CHAT_SETTINGS },
    markUnsaved: vi.fn(),
  } as unknown as WorkflowManager;
}

function change(id: string, checked: boolean): void {
  const el = document.getElementById(id) as HTMLInputElement;
  el.checked = checked;
  el.dispatchEvent(new Event("change"));
}

beforeEach(() => {
  document.body.innerHTML = WF_SETTINGS_HTML;
});

describe("bindWfSettings — live-applies chat settings to an open Chat panel", () => {
  it("normal case: toggling allow_attachments calls chatPanel.applyToggles with the updated settings", () => {
    const wfManager = stubWfManager();
    const chatPanel = stubChatPanel();
    bindWfSettings(wfManager, chatPanel);

    change("wf-setting-chat-attachments", true);

    expect(chatPanel.applyToggles).toHaveBeenCalledTimes(1);
    expect(chatPanel.applyToggles).toHaveBeenCalledWith(wfManager.chatSettings);
    expect(wfManager.chatSettings.allow_attachments).toBe(true);
  });

  it("edge case: each of the 5 chat-setting handlers applies live, non-chat settings do not", () => {
    const wfManager = stubWfManager();
    const chatPanel = stubChatPanel();
    bindWfSettings(wfManager, chatPanel);

    change("wf-setting-parallel", true);
    expect(chatPanel.applyToggles).not.toHaveBeenCalled();

    change("wf-setting-chat-images", false);
    change("wf-setting-chat-persistence", false);
    change("wf-setting-chat-branding", false);
    expect(chatPanel.applyToggles).toHaveBeenCalledTimes(3);
  });
});
