/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
  convertFileSrc: vi.fn((p: string) => p),
}));

import { ChatPanel } from "../panels/ChatPanel";
import { saveChatSession } from "../ipc/chat";
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

const MESSAGES = 20;
const BLOB_BYTES = 200 * 1024;

type Msg = { id: string; role: string; images: { filename: string; data: string; mime_type: string }[]; timestamp: number };
type PanelInternals = {
  store: { sessions: { id: string; name: string; createdAt: number; messages: Msg[] }[]; activeId: string };
  persist(): Promise<void>;
};

function imageMessage(i: number): Msg {
  return { id: `m${i}`, role: "ai", images: [{ filename: `${i}.png`, data: "A".repeat(BLOB_BYTES), mime_type: "image/png" }], timestamp: i + 1 };
}

/** Bytes of the JSON-serialized arguments of every invoke() call made while `work` runs. */
async function ipcBytesDuring(work: () => Promise<void>): Promise<number> {
  invokeMock.mockClear();
  await work();
  return invokeMock.mock.calls.reduce((sum, call) => sum + JSON.stringify(call[1] ?? {}).length, 0);
}

beforeEach(() => {
  invokeMock.mockReset().mockResolvedValue(undefined);
});

describe("ChatPanel persist() IPC payload", () => {
  it("sends about one message, not the whole session, when a 20-message session gains its 20th image message", async () => {
    document.body.innerHTML = CHAT_PANEL_HTML;
    const wfManager = { currentId: "wf1", chatSettings: {} } as unknown as WorkflowManager;
    const p = new ChatPanel({ nodes: new Map() } as unknown as Canvas, wfManager, vi.fn(), runManagerStub) as unknown as PanelInternals;
    const all = Array.from({ length: MESSAGES }, (_, i) => imageMessage(i));
    p.store = { sessions: [{ id: "s1", name: "A", createdAt: 1_000, messages: all.slice(0, MESSAGES - 1) }], activeId: "s1" };
    await p.persist();

    p.store.sessions[0].messages.push(all[MESSAGES - 1]);
    const after = await ipcBytesDuring(() => p.persist());
    const unchanged = await ipcBytesDuring(() => p.persist());

    const legacyWholeSession = {
      id: "s1", workflow_id: "wf1", name: "A", created_at: 1_000,
      messages: all.map((m) => ({ id: m.id, role: m.role, text: undefined, images: m.images, attachments: undefined, timestamp: m.timestamp })),
    };
    const before = await ipcBytesDuring(() => saveChatSession(legacyWholeSession));

    console.info(`persist() IPC bytes, ${MESSAGES} messages x ~${BLOB_BYTES / 1024} KB: before(whole session)=${before} after(one new message)=${after} after(nothing new)=${unchanged}`);

    const oneMessage = JSON.stringify(all[0]).length;
    expect(before).toBeGreaterThan(MESSAGES * BLOB_BYTES);
    expect(after).toBeLessThan(oneMessage + 2_000);
    expect(unchanged).toBeLessThan(500);
    expect(before / after).toBeGreaterThan(MESSAGES * 0.9);
  });
});
