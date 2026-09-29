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

type PanelInternals = {
  store: { sessions: { id: string; messages: { id: string; text?: string }[] }[]; activeId: string };
  schedulerRunning: boolean;
  loadStoreForCurrentWorkflow(): Promise<void>;
  refreshRunningState(): void;
  switchSession(id: string): Promise<void>;
  deleteSession(id: string): Promise<void>;
  appendMessage(m: { id: string; role: string; text: string; timestamp: number }): void;
  persist(): Promise<void>;
};

type Deferred<T> = { promise: Promise<T>; resolve(v: T): void; reject(e: unknown): void };
function deferred<T>(): Deferred<T> {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

const wire = (id: string, text: string) => [{ id, role: "ai", text, timestamp: 1 }];
const shownText = () => document.getElementById("chat-messages")!.textContent ?? "";
const loadingShown = () => document.querySelector("#chat-messages .chat-loading") !== null;
const inputLocked = () => (document.getElementById("chat-input") as HTMLTextAreaElement).disabled;
const flush = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };

let toastFn: ReturnType<typeof vi.fn>;

/** s1 (active, loaded, message "one"), s2 and s3 unloaded. The panel is live, so the input is enabled unless a switch locks it. */
async function makeLoadedPanel(): Promise<PanelInternals> {
  document.body.innerHTML = CHAT_PANEL_HTML;
  toastFn = vi.fn();
  const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
  const p = new ChatPanel({ nodes: new Map() } as unknown as Canvas, wfManager, toastFn as unknown as (m: string, t?: "success" | "error" | "info") => void, runManagerStub) as unknown as PanelInternals;
  chatMocks.listChatSessionMeta.mockResolvedValue([
    { id: "s3", workflow_id: "wf1", name: "C", created_at: 3_000 },
    { id: "s2", workflow_id: "wf1", name: "B", created_at: 2_000 },
    { id: "s1", workflow_id: "wf1", name: "A", created_at: 1_000 },
  ]);
  wfMocks.getSetting.mockResolvedValue("s1");
  chatMocks.loadChatMessages.mockImplementation(async (id: string) => (id === "s1" ? wire("m-s1", "one") : []));
  await p.loadStoreForCurrentWorkflow();
  p.schedulerRunning = true;
  p.refreshRunningState();
  (p as unknown as { renderMessages(): void }).renderMessages();
  chatMocks.loadChatMessages.mockClear();
  return p;
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

describe("ChatPanel — loading a session's messages on switch", () => {
  it("shows a loading state and locks the input until the messages arrive, then makes the session active (normal case)", async () => {
    const p = await makeLoadedPanel();
    const d = deferred<ReturnType<typeof wire>>();
    chatMocks.loadChatMessages.mockReturnValueOnce(d.promise);
    expect(inputLocked()).toBe(false);

    const switching = p.switchSession("s2");

    expect(loadingShown()).toBe(true);
    expect(document.getElementById("chat-messages")!.getAttribute("aria-busy")).toBe("true");
    expect(inputLocked()).toBe(true);
    expect(p.store.activeId).toBe("s1");

    d.resolve(wire("m-s2", "two"));
    await switching;

    expect(chatMocks.loadChatMessages).toHaveBeenCalledWith("s2");
    expect(p.store.activeId).toBe("s2");
    expect(loadingShown()).toBe(false);
    expect(document.getElementById("chat-messages")!.hasAttribute("aria-busy")).toBe(false);
    expect(shownText()).toContain("two");
    expect(inputLocked()).toBe(false);
  });

  it("does not fetch a session again once it has been loaded (edge case)", async () => {
    const p = await makeLoadedPanel();
    chatMocks.loadChatMessages.mockResolvedValueOnce(wire("m-s2", "two"));

    await p.switchSession("s2");
    await p.switchSession("s1");
    await p.switchSession("s2");

    expect(chatMocks.loadChatMessages).toHaveBeenCalledTimes(1);
    expect(shownText()).toContain("two");
  });

  it("last click wins: an earlier switch whose load resolves late renders nothing (edge case)", async () => {
    const p = await makeLoadedPanel();
    const d2 = deferred<ReturnType<typeof wire>>();
    const d3 = deferred<ReturnType<typeof wire>>();
    chatMocks.loadChatMessages.mockReturnValueOnce(d2.promise).mockReturnValueOnce(d3.promise);

    const first = p.switchSession("s2");
    const second = p.switchSession("s3");
    d3.resolve(wire("m-s3", "three"));
    await second;
    d2.resolve(wire("m-s2", "two"));
    await first;

    expect(p.store.activeId).toBe("s3");
    expect(shownText()).toContain("three");
    expect(shownText()).not.toContain("two");
    expect(inputLocked()).toBe(false);
  });

  it("last click wins: an earlier switch whose load resolves first does not apply (edge case)", async () => {
    const p = await makeLoadedPanel();
    const d2 = deferred<ReturnType<typeof wire>>();
    const d3 = deferred<ReturnType<typeof wire>>();
    chatMocks.loadChatMessages.mockReturnValueOnce(d2.promise).mockReturnValueOnce(d3.promise);

    const first = p.switchSession("s2");
    const second = p.switchSession("s3");
    d2.resolve(wire("m-s2", "two"));
    await first;

    expect(p.store.activeId).toBe("s1");
    expect(loadingShown()).toBe(true);

    d3.resolve(wire("m-s3", "three"));
    await second;
    expect(p.store.activeId).toBe("s3");
    expect(shownText()).toContain("three");
  });

  it("deleting the session that is loading cancels the switch and restores the current session (edge case)", async () => {
    const p = await makeLoadedPanel();
    const d2 = deferred<ReturnType<typeof wire>>();
    chatMocks.loadChatMessages.mockReturnValueOnce(d2.promise);

    const switching = p.switchSession("s2");
    await p.deleteSession("s2");

    expect(loadingShown()).toBe(false);
    expect(shownText()).toContain("one");
    expect(inputLocked()).toBe(false);
    expect(chatMocks.deleteChatSession).toHaveBeenCalledWith("s2");

    d2.resolve(wire("m-s2", "two"));
    await switching;

    expect(p.store.activeId).toBe("s1");
    expect(p.store.sessions.map((s) => s.id)).toEqual(["s3", "s1"]);
    expect(shownText()).not.toContain("two");
  });

  it("a failed load keeps the previous session showing, unlocks the input, toasts, and can be retried (edge case)", async () => {
    const p = await makeLoadedPanel();
    chatMocks.loadChatMessages.mockRejectedValueOnce(new Error("db locked"));

    await p.switchSession("s2");

    expect(p.store.activeId).toBe("s1");
    expect(loadingShown()).toBe(false);
    expect(shownText()).toContain("one");
    expect(inputLocked()).toBe(false);
    expect(toastFn).toHaveBeenCalledWith(expect.stringContaining("load that session"), "error");

    chatMocks.loadChatMessages.mockResolvedValueOnce(wire("m-s2", "two"));
    await p.switchSession("s2");
    expect(p.store.activeId).toBe("s2");
    expect(shownText()).toContain("two");
  });

  it("deleting the active session loads the session it falls back to before showing it, and that session then saves normally (edge case)", async () => {
    const p = await makeLoadedPanel();
    chatMocks.loadChatMessages.mockResolvedValueOnce(wire("m-s3", "three"));

    await p.deleteSession("s1");

    expect(chatMocks.loadChatMessages).toHaveBeenCalledWith("s3");
    expect(p.store.activeId).toBe("s3");
    expect(shownText()).toContain("three");

    p.appendMessage({ id: "new", role: "user", text: "hello", timestamp: 9 });
    await p.persist();
    expect(chatMocks.appendChatMessage.mock.calls.map((c) => c[1].id)).toEqual(["new"]);
  });

  it("a session delete waits for a message write that is still in flight (edge case)", async () => {
    const p = await makeLoadedPanel();
    const d = deferred<void>();
    chatMocks.appendChatMessage.mockReturnValueOnce(d.promise);
    p.appendMessage({ id: "late", role: "user", text: "hi", timestamp: 9 });
    void p.persist();
    await flush();
    expect(chatMocks.appendChatMessage).toHaveBeenCalledTimes(1);

    const deleting = p.deleteSession("s2");
    await flush();
    expect(chatMocks.deleteChatSession).not.toHaveBeenCalled();

    d.resolve();
    await deleting;
    expect(chatMocks.deleteChatSession).toHaveBeenCalledWith("s2");
  });
});
