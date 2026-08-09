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
}));

vi.mock("../ipc/chat", () => ({
  listChatSessions: vi.fn().mockResolvedValue([]),
  saveChatSession: vi.fn(),
  deleteChatSession: vi.fn(),
}));

import { ChatPanel } from "../panels/ChatPanel";
import { getScheduledJob } from "../ipc/workflow";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowManager } from "../workflow-manager";
import type { RunManager } from "../run-manager";
import type { SchedulerStatusEvent } from "../ipc/events";

const mockGetScheduledJob = vi.mocked(getScheduledJob);

// Mirrors index.html's real banner markup (span + Start button) — the
// other ChatPanel test files' fixtures omit the span since they don't
// exercise banner text, but this file specifically does.
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
      <span>Start this workflow to begin chatting</span>
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

type PanelInternals = {
  onMainRunStateChange(running: boolean): void;
  refreshRunningState(): void;
  onSchedulerStatus(evt: SchedulerStatusEvent): void;
  handleSend(overrideText?: string): Promise<void>;
  activeWebhookPort: number | null;
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

function activeJobRow(port: number) {
  return {
    workflow_id: "wf1", workflow_name: "wf", trigger_kind: JSON.stringify({ port }),
    status: "active" as const, always_on: false, run_count: 0,
    last_run_at: null, next_run_at: null, last_error: null, created_at: "",
  };
}

function bannerText(): string {
  return document.querySelector("#chat-not-running-banner span")!.textContent ?? "";
}

describe("ChatPanel — main Run button state (onMainRunStateChange)", () => {
  it("normal case: an ad-hoc Run in flight gets an explanatory banner instead of the generic 'Start' prompt, and input stays disabled", () => {
    const p = makePanel();
    p.onMainRunStateChange(true);
    p.refreshRunningState();

    const input = document.getElementById("chat-input") as HTMLTextAreaElement;
    expect(bannerText()).toContain("one-time test");
    expect(input.disabled).toBe(true);
  });

  it("edge case: once the Run ends, the banner reverts to the default prompt rather than staying stuck on the Run-specific text", () => {
    const p = makePanel();
    p.onMainRunStateChange(true);
    p.refreshRunningState();

    p.onMainRunStateChange(false);
    p.refreshRunningState();

    expect(bannerText()).toBe("Start this workflow to begin chatting");
  });
});

describe("ChatPanel — send gating (schedulerRunning)", () => {
  beforeEach(() => { vi.useFakeTimers(); vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true })); });
  afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

  it("normal case: an ad-hoc Run in flight does not let a chat message reach the webhook", async () => {
    const p = makePanel(webhookOnlyCanvas());
    p.onMainRunStateChange(true);
    await p.handleSend("hello");
    expect(fetch).not.toHaveBeenCalled();
  });

  it("edge case: a real scheduler 'waiting' status lets the message reach the webhook", async () => {
    const p = makePanel(webhookOnlyCanvas());
    p.onSchedulerStatus(waitingEvent());
    await p.handleSend("hello");
    expect(fetch).toHaveBeenCalledWith("http://127.0.0.1:3456/webhook", expect.objectContaining({ method: "POST" }));
  });
});

describe("ChatPanel — activeWebhookPort resync (refreshActiveWebhookPort)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true }));
    mockGetScheduledJob.mockReset();
  });
  afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

  it("normal case: a stale cached port is overwritten with the DB's current bound port before send", async () => {
    mockGetScheduledJob.mockResolvedValue(activeJobRow(9001));
    const p = makePanel(webhookOnlyCanvas());
    p.onSchedulerStatus(waitingEvent());
    p.activeWebhookPort = 3456; // stale — learned before an external restart moved the bind

    await p.handleSend("hello");

    expect(fetch).toHaveBeenCalledWith("http://127.0.0.1:9001/webhook", expect.objectContaining({ method: "POST" }));
  });

  it("edge case: a row that isn't 'active' leaves the last-known port in place instead of clearing it", async () => {
    mockGetScheduledJob.mockResolvedValue(null);
    const p = makePanel(webhookOnlyCanvas());
    p.onSchedulerStatus(waitingEvent());
    p.activeWebhookPort = 9002; // learned earlier this session; DB lookup now finds nothing

    await p.handleSend("hello");

    expect(fetch).toHaveBeenCalledWith("http://127.0.0.1:9002/webhook", expect.objectContaining({ method: "POST" }));
  });
});
