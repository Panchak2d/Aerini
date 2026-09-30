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
  handleSend(overrideText?: string, overrideAttachments?: Attachment[]): Promise<void>;
  renderMessages(): void;
  renderPendingAttachments(): void;
  autosizeInput(): void;
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


const $ = (sel: string) => document.querySelector(sel) as HTMLElement;
const IMG = { filename: "pic.png", data: "iVBORw0KGgo=", mime_type: "image/png" };
const PDF = { filename: "notes.pdf", data: "Yg==", mime_type: "application/pdf" };

beforeEach(() => {
  chatMocks.listChatSessionMeta.mockReset();
  chatMocks.loadChatMessages.mockReset().mockResolvedValue([]);
  chatMocks.appendChatMessage.mockReset().mockResolvedValue(undefined);
  chatMocks.saveChatSessionMeta.mockReset().mockResolvedValue(undefined);
  chatMocks.saveChatSession.mockReset().mockResolvedValue(undefined);
  chatMocks.deleteChatSession.mockReset().mockResolvedValue(undefined);
  localStorage.clear();
});

describe("ChatPanel — single status notice (banner / centered layouts)", () => {
  it("normal case: empty session while not running shows the centered notice with Start; the empty message list is hidden", () => {
    const p = makePanel();
    p.renderMessages();

    expect($("#chat-status").classList.contains("hidden")).toBe(false);
    expect($("#chat-status").dataset.layout).toBe("centered");
    expect($("#btn-chat-start").classList.contains("hidden")).toBe(false);
    expect($("#chat-messages").classList.contains("hidden")).toBe(true);
  });

  it("edge case: going live drops Start, and the first message swaps the notice out for the conversation", async () => {
    vi.useFakeTimers(); vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true }));
    try {
      const p = makePanel(webhookOnlyCanvas());
      p.onSchedulerStatus(waitingEvent());
      p.renderMessages();
      expect($("#chat-status").dataset.layout).toBe("centered");
      expect($("#btn-chat-start").classList.contains("hidden")).toBe(true);
      expect($("#chat-status-text").textContent).toContain("Send a message");

      p.inputEl.value = "hello";
      await p.handleSend();

      expect($("#chat-status").classList.contains("hidden")).toBe(true);
      expect($("#chat-messages").classList.contains("hidden")).toBe(false);
    } finally { vi.useRealTimers(); vi.unstubAllGlobals(); }
  });

  it("edge case: not live but history exists uses the banner layout above the messages", () => {
    chatMocks.listChatSessionMeta.mockResolvedValue([]);
    const p = makePanel(webhookOnlyCanvas());
    p.renderMessages();
    // seed one message through the public render path
    (p as unknown as { store: { sessions: unknown[]; activeId: string } }).store = {
      activeId: "s1",
      sessions: [{ id: "s1", name: "A", createdAt: 1, messages: [{ id: "m1", role: "ai", text: "hi", timestamp: 1 }] }],
    };
    p.renderMessages();

    expect($("#chat-status").dataset.layout).toBe("banner");
    expect($("#chat-status").classList.contains("hidden")).toBe(false);
    expect($("#chat-messages").classList.contains("hidden")).toBe(false);
  });
});

describe("ChatPanel — attachment-only sends", () => {
  beforeEach(() => { vi.useFakeTimers(); vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true })); });
  afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

  it("normal case: an attachment alone enables Send, posts an empty message with the file, and renders a chip-only bubble", async () => {
    const p = makePanel(webhookOnlyCanvas());
    p.onSchedulerStatus(waitingEvent());
    $("#chat-input").removeAttribute("disabled");
    p.pendingAttachments = [PDF];
    p.renderPendingAttachments();
    expect(($("#chat-send-btn") as HTMLButtonElement).disabled).toBe(false);

    await p.handleSend();

    const body = JSON.parse((fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0][1].body);
    expect(body.message).toBe("");
    expect(body.attachments).toEqual([PDF]);
    const bubble = $("#chat-messages .chat-bubble--user");
    expect(bubble.querySelectorAll(".chat-attachment-chip")).toHaveLength(1);
    expect(bubble.querySelector(".chat-bubble-copy")).toBeNull();
  });

  it("edge case: nothing typed and nothing attached still sends nothing; a retry of an attachment-only message does not add a second bubble", async () => {
    const p = makePanel(webhookOnlyCanvas());
    p.onSchedulerStatus(waitingEvent());

    await p.handleSend();
    expect(fetch).not.toHaveBeenCalled();

    await p.handleSend("", [PDF]);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(document.querySelectorAll("#chat-messages .chat-bubble--user")).toHaveLength(0);
  });
});

describe("ChatPanel — sent chip actions", () => {
  afterEach(() => { vi.restoreAllMocks(); document.querySelectorAll(".chat-lightbox").forEach(n => n.remove()); });

  it("normal + edge case: an image chip opens the lightbox; a non-image chip downloads under its own filename", () => {
    (URL as unknown as { createObjectURL: unknown }).createObjectURL = vi.fn(() => "blob:x");
    (URL as unknown as { revokeObjectURL: unknown }).revokeObjectURL = vi.fn();
    const downloads: string[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) { downloads.push(this.download); });

    chatMocks.listChatSessionMeta.mockResolvedValue([]);
    const p = makePanel();
    (p as unknown as { store: unknown }).store = {
      activeId: "s1",
      sessions: [{ id: "s1", name: "A", createdAt: 1, messages: [{ id: "m1", role: "user", text: "x", attachments: [IMG, PDF], timestamp: 1 }] }],
    };
    p.renderMessages();
    const [imgChip, pdfChip] = Array.from(document.querySelectorAll<HTMLButtonElement>("#chat-messages button.chat-attachment-chip"));

    imgChip.click();
    expect(document.querySelector(".chat-lightbox img")?.getAttribute("src")).toBe(`data:image/png;base64,${IMG.data}`);
    expect(downloads).toEqual([]);

    pdfChip.click();
    expect(downloads).toEqual(["notes.pdf"]);
  });
});

describe("ChatPanel — input autosize", () => {
  it("normal case: the textarea height includes its border so multi-line text does not overflow by the border width", () => {
    const p = makePanel();
    const input = p.inputEl;
    Object.defineProperty(input, "scrollHeight", { configurable: true, value: 60 });
    Object.defineProperty(input, "offsetHeight",  { configurable: true, value: 46 });
    Object.defineProperty(input, "clientHeight",  { configurable: true, value: 44 });

    p.autosizeInput();

    expect(input.style.height).toBe("62px");
  });
});

describe("ChatPanel — unread attachments warning", () => {
  const send = async (nodes: Map<string, unknown>) => {
    vi.useFakeTimers(); vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true }));
    const toast = vi.fn();
    try {
      document.body.innerHTML = CHAT_PANEL_HTML;
      const canvas = { nodes } as unknown as Canvas;
      const wf = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
      const rm = { addRunStateListener: vi.fn() } as unknown as RunManager;
      const p = new ChatPanel(canvas, wf, toast, rm) as unknown as PanelInternals;
      p.onSchedulerStatus(waitingEvent());
      await p.handleSend("", [IMG]);
    } finally { vi.useRealTimers(); vi.unstubAllGlobals(); }
    return toast;
  };
  const hook = { data: { node_type_id: "webhook", name: "Webhook", config: { port: 3456, path: "/webhook" } } };

  it("warns when no node references the Webhook's files output", async () => {
    const toast = await send(new Map<string, unknown>([["n1", hook], ["n2", { data: { node_type_id: "ai_prompt", name: "AI", config: { prompt: "hi" } } }]]));
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("no node reads them"), "info");
  });

  it("stays silent when a node's Files port is wired to the Webhook", async () => {
    const toast = await send(new Map<string, unknown>([["n1", hook], ["n2", { data: { node_type_id: "ai_prompt", name: "AI", config: { attachments_expr: "{{Webhook.output.files}}" } } }]]));
    expect(toast).not.toHaveBeenCalledWith(expect.stringContaining("no node reads them"), "info");
  });
});
