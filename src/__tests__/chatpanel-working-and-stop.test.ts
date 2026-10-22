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
  cancelScheduledRun: vi.fn(),
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
import { cancelScheduledRun } from "../ipc/workflow";

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
  awaitingReply: boolean;
  stopRequested: boolean;
  chatSettings: Record<string, unknown>;
  fileInputEl: HTMLInputElement;
};

const toast = vi.fn();

function makePanel(): PanelInternals {
  document.body.innerHTML = CHAT_PANEL_HTML;
  const canvas = {
    nodes: new Map([["n1", { data: { node_type_id: "webhook", config: { port: 3456, path: "/webhook" } } }]]),
  } as unknown as Canvas;
  const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
  const runManager = { addRunStateListener: vi.fn() } as unknown as RunManager;
  return new ChatPanel(canvas, wfManager, toast, runManager) as unknown as PanelInternals;
}

function waitingEvent(): SchedulerStatusEvent {
  return {
    workflow_id: "wf1", workflow_name: "wf", status: "waiting", run_count: 1,
    last_run_at: null, next_run_at: null, last_error: null, last_result: null,
  };
}

const sendBtn = () => document.getElementById("chat-send-btn") as HTMLButtonElement;

async function pendingPanel(): Promise<PanelInternals> {
  const p = makePanel();
  p.onSchedulerStatus(waitingEvent());
  await p.handleSend("hello");
  return p;
}

describe("ChatPanel — slow reply is progress, not failure", () => {
  beforeEach(() => { vi.useFakeTimers(); vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true })); toast.mockClear(); });
  afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

  it("after 30s the typing bubble gains a neutral note, with no error bubble and the input still locked", async () => {
    const p = await pendingPanel();
    vi.advanceTimersByTime(30_000);

    expect(document.querySelector("#chat-typing-row .chat-working-note")?.textContent).toContain("working on your task");
    expect(document.querySelector(".chat-bubble--error")).toBeNull();
    expect(p.awaitingReply).toBe(true);
    expect((document.getElementById("chat-input") as HTMLTextAreaElement).disabled).toBe(true);
  });
});

describe("ChatPanel — Stop", () => {
  beforeEach(() => { vi.useFakeTimers(); vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true })); vi.mocked(cancelScheduledRun).mockReset(); toast.mockClear(); });
  afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

  it("while a reply is pending the send button is an enabled Stop; pressing it cancels that workflow's run and unlocks", async () => {
    vi.mocked(cancelScheduledRun).mockResolvedValue(undefined);
    const p = await pendingPanel();
    expect(sendBtn().disabled).toBe(false);
    expect(sendBtn().getAttribute("aria-label")).toBe("Stop");

    sendBtn().click();
    await vi.advanceTimersByTimeAsync(0);

    expect(cancelScheduledRun).toHaveBeenCalledWith("wf1");
    expect(document.getElementById("chat-typing-row")).toBeNull();
    expect(p.awaitingReply).toBe(false);
    expect(sendBtn().getAttribute("aria-label")).toBe("Send");
  });

  it("the run's closing status event after a Stop renders nothing", async () => {
    let resolveCancel!: () => void;
    vi.mocked(cancelScheduledRun).mockReturnValue(new Promise<void>(r => { resolveCancel = r; }));
    const p = await pendingPanel();
    const bubblesBefore = document.querySelectorAll(".chat-bubble").length;

    sendBtn().click();
    p.onSchedulerStatus({ ...waitingEvent(), status: "error", last_error: "Run cancelled by user" });
    resolveCancel();
    await vi.advanceTimersByTimeAsync(0);

    expect(document.querySelector(".chat-bubble--error")).toBeNull();
    expect(document.getElementById("chat-typing-row")).toBeNull();
    expect(document.querySelectorAll(".chat-bubble").length).toBe(bubblesBefore - 1);
  });

  it("when the cancel call fails the reply stays pending and the user is told", async () => {
    vi.mocked(cancelScheduledRun).mockRejectedValue("command not found");
    const p = await pendingPanel();

    sendBtn().click();
    await vi.advanceTimersByTimeAsync(0);

    expect(toast).toHaveBeenCalledWith(expect.stringContaining("Could not stop"), "error");
    expect(p.awaitingReply).toBe(true);
    expect(p.stopRequested).toBe(false);
    expect(sendBtn().disabled).toBe(false);
  });
});

describe("ChatPanel — attach button", () => {
  function pick(p: PanelInternals, file: File): void {
    Object.defineProperty(p.fileInputEl, "files", { value: [file], configurable: true });
    p.fileInputEl.dispatchEvent(new Event("change"));
  }

  it("uses one input that stays attached to the DOM, and a picked file becomes a chip", async () => {
    const p = makePanel();
    p.chatSettings = { allow_attachments: true, max_message_length: 4000 };
    expect(p.fileInputEl.isConnected).toBe(true);

    pick(p, new File(["hi"], "note.txt", { type: "text/plain" }));
    await vi.waitFor(() => expect(document.querySelector(".chat-attachment-chip")).not.toBeNull());

    expect(p.fileInputEl.isConnected).toBe(true);
    expect(p.fileInputEl.value).toBe("");
  });

  it("an unsupported type is reported instead of being silently ignored", () => {
    const p = makePanel();
    p.chatSettings = { allow_attachments: true, max_message_length: 4000 };
    toast.mockClear();

    pick(p, new File(["x"], "data.exe"));

    expect(toast).toHaveBeenCalledWith(expect.stringContaining("not a supported attachment type"), "error");
  });
});
