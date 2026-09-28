import { marked } from "marked";
import DOMPurify from "dompurify";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowManager } from "../workflow-manager";
import type { RunManager } from "../run-manager";
import { NODE_IDS } from "../node-ids";
import { startScheduledWorkflow, stopScheduledWorkflow, getScheduledJobs, getScheduledJob, parseSchedulerError, clearChatSession, getSetting, setSetting } from "../ipc/workflow";
import type { WorkflowResult, ScheduledJobRow } from "../ipc/workflow";
import { listChatSessions, saveChatSession, deleteChatSession } from "../ipc/chat";
import type { ChatSessionWire } from "../ipc/chat";
import type { SchedulerStatusEvent } from "../ipc/events";
import { addSchedulerStatusListener } from "../scheduler-events";
import { escapeHtml } from "../utils";
import { type ChatSettings, DEFAULT_CHAT_SETTINGS } from "../canvas/CanvasSerializer";

type Toast = (msg: string, type?: "success" | "error" | "info") => void;

const RESPONSE_TIMEOUT_MS = 30_000;

interface ChatImageFile { filename: string; data: string; mime_type: string; }

/** Outgoing user attachment — same wire shape as ChatImageFile but not image-only. */
interface ChatAttachment { filename: string; data: string; mime_type: string; }

// Accepted types match the AI Prompt node's static attachments UI (popover/extensions/ai-prompt.ts) exactly.
const CHAT_ATTACHMENT_ACCEPT = ".png,.jpg,.jpeg,.webp,.gif,.pdf,.txt,.md";

// file.type is unreliable for non-standard extensions (e.g. .md on Windows) — falls back to
// CHAT_ATTACHMENT_MIME_MAP by extension, same approach as the AI Prompt attachments UI.
const CHAT_ATTACHMENT_MIME_MAP: Record<string, string> = {
  ".png":  "image/png",
  ".jpg":  "image/jpeg",
  ".jpeg": "image/jpeg",
  ".webp": "image/webp",
  ".gif":  "image/gif",
  ".pdf":  "application/pdf",
  ".txt":  "text/plain",
  ".md":   "text/markdown",
};

// Same icon set as the AI Prompt attachments UI's chips (popover/extensions/ai-prompt.ts) — no emoji, themeable via currentColor.
const CHAT_ATTACHMENT_FILE_ICON = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>`;
const CHAT_ATTACHMENT_X_ICON    = `<svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;

/**
 * Hard wire-transmissibility ceiling, not a soft "file size" warning. Mirrors
 * aerini-engine/src/nodes/webhook.rs `MAX_BODY_BYTES` — the local webhook
 * server rejects any request body over this many bytes. handleSend() never
 * inspects the HTTP response (see the class doc above — resolution is
 * entirely event-driven), so a request rejected for size still waits out
 * the full 30s timeout instead of failing immediately. Enforced here as a
 * hard client-side block rather than a dismissible warning, since an
 * oversized request never succeeds at all — unlike a saved workflow file,
 * which just grows.
 */
const CHAT_WEBHOOK_BODY_CAP_BYTES = 1_000_000;
/** Reserve for JSON structure, session_id, and per-file filename/mime_type strings. */
const CHAT_BODY_JSON_OVERHEAD_BYTES = 2_000;

interface ChatMessage {
  id: string;
  role: "user" | "ai" | "error";
  text?: string;
  images?: ChatImageFile[];
  attachments?: ChatAttachment[];
  timestamp: number;
}

interface ChatSession {
  id: string;
  name: string;
  messages: ChatMessage[];
  createdAt: number;
}

interface ChatStore { sessions: ChatSession[]; activeId: string; }

/**
 * Chat Panel.
 *
 * IMPORTANT — message flow does NOT use the webhook's HTTP response.
 * The Webhook node (aerini-engine/src/nodes/webhook.rs, scheduler/runner.rs)
 * always replies "OK" over HTTP regardless of what the workflow produces —
 * there is no code path that holds the connection open for a result. The
 * actual reply arrives asynchronously as a `scheduler-status` Tauri event
 * with `last_result.node_outputs[<Output node id>]` once the background run
 * completes. `fetch()` below is fire-and-forget; resolution happens in
 * onSchedulerStatus().
 */
export class ChatPanel {
  private el:           HTMLElement;
  private messagesEl:   HTMLElement;
  private inputEl:      HTMLTextAreaElement;
  private sendBtn:      HTMLButtonElement;
  private bannerEl:     HTMLElement;
  private bannerTextEl: HTMLElement | null;
  private startBtn:     HTMLButtonElement;
  /** Set only while the active session has zero messages (see renderMessages/
   *  buildEmptyState) — null the rest of the time, once messagesEl has been
   *  rebuilt with real chat bubbles. */
  private emptyStateTextEl:   HTMLElement | null = null;
  private emptyStateStartBtn: HTMLButtonElement | null = null;
  private sessionLabel: HTMLElement;
  private sessionMenu:  HTMLElement;
  private chatBtn:      HTMLButtonElement | null;
  private attachBtn:    HTMLButtonElement;
  private brandingEl:   HTMLElement | null;
  /** Container for pending-attachment chips, inserted above .chat-input-row. No matching static markup in index.html — created here, mirroring the existing pattern of programmatic DOM construction elsewhere in this file (session menu, lightbox). */
  private pendingAttachmentsEl: HTMLElement;

  private canvas:    Canvas;
  private wfManager: WorkflowManager;
  private toast:     Toast;

  private store: ChatStore = { sessions: [], activeId: "" };
  /** Set on every panel open via applyToggles(wfManager.chatSettings). */
  private chatSettings: ChatSettings = { ...DEFAULT_CHAT_SETTINGS };

  // `awaitingReply` only gates the input lock / "one send at a time" rule —
  // it's cleared on timeout so the user regains control (per spec). `replyPending`
  // tracks whether we still owe a render for an outstanding request; it stays
  // true across a timeout so a reply that arrives late still gets shown instead
  // of being silently dropped.
  private awaitingReply  = false;
  private replyPending   = false;
  private pendingTimer:  ReturnType<typeof setTimeout> | null = null;
  private lastSentText   = "";
  /** Files attached to the next outgoing message, not yet sent. */
  private pendingAttachments:  ChatAttachment[] = [];
  /** Snapshot of what was actually sent, for the error-bubble Retry button. */
  private lastSentAttachments: ChatAttachment[] = [];
  private persistFailureToasted = false;
  /** Port the workflow's Webhook trigger is actually bound to right now —
   *  distinct from the node's static config port, since startOnFreePort()
   *  may have fallen back to a different one. Null until start/sync learns
   *  it; cleared on workflow switch. */
  private activeWebhookPort: number | null = null;
  /** True only while the scheduler/Start-button path (onSchedulerStatus's
   *  "running"/"waiting", or a synced DB row's "active") reports this
   *  workflow live. Deliberately not sourced from isWorkflowRunning() /
   *  _runningWorkflows: that shared flag is also set true by RunManager's
   *  ad-hoc "Run" button (see mainRunActive below), which never emits
   *  scheduler-status events — a reply can only ever resolve through one of
   *  those events, so gating send/banner on the shared flag would let the
   *  UI promise a reply that never arrives. */
  private schedulerRunning = false;
  /** True while the main Run button's ad-hoc, one-shot execution is in
   *  flight for the current workflow (see onMainRunStateChange). Distinct
   *  from schedulerRunning above — the ad-hoc path never emits
   *  scheduler-status events, so without this, refreshRunningState() had no
   *  way to know a Run was active and would show a stale/misleading "Start
   *  this workflow" prompt. */
  private mainRunActive = false;

  constructor(canvas: Canvas, wfManager: WorkflowManager, toast: Toast, runManager: RunManager) {
    this.canvas    = canvas;
    this.wfManager = wfManager;
    this.toast     = toast;

    this.el           = document.getElementById("chat-panel")!;
    this.messagesEl   = document.getElementById("chat-messages")!;
    this.inputEl      = document.getElementById("chat-input") as HTMLTextAreaElement;
    this.sendBtn      = document.getElementById("chat-send-btn") as HTMLButtonElement;
    this.bannerEl     = document.getElementById("chat-not-running-banner")!;
    this.bannerTextEl = this.bannerEl.querySelector("span");
    this.startBtn     = document.getElementById("btn-chat-start") as HTMLButtonElement;
    this.sessionLabel = document.getElementById("chat-session-label")!;
    this.sessionMenu  = document.getElementById("chat-session-menu")!;
    this.chatBtn      = document.getElementById("btn-chat") as HTMLButtonElement | null;
    this.attachBtn    = document.getElementById("chat-attach-btn") as HTMLButtonElement;
    this.brandingEl   = document.getElementById("chat-branding-footer");

    // No static markup for this in index.html — built and inserted here,
    // same approach as the existing session-menu/lightbox elements in this file.
    this.pendingAttachmentsEl = document.createElement("div");
    this.pendingAttachmentsEl.className = "chat-pending-attachments";
    this.pendingAttachmentsEl.style.display = "none";
    const inputRow = this.el.querySelector(".chat-input-row");
    if (inputRow && inputRow.parentElement) {
      inputRow.parentElement.insertBefore(this.pendingAttachmentsEl, inputRow);
    }

    this.bindStaticEvents();

    // listenSchedulerStatus (ipc/events.ts) only ever delivers to its first
    // caller (scheduler-events.ts, registered at app init) — this fan-out
    // subscription is the supported way for anything else to observe events.
    addSchedulerStatusListener((evt) => this.onSchedulerStatus(evt));

    // RunManager only ever runs one workflow at a time and keeps its
    // currentWorkflowId synced on every navigation (see app.ts's onNavigate),
    // so any event here always pertains to whatever is currently loaded —
    // same assumption toolbar.ts's own listener already relies on.
    runManager.addRunStateListener((running) => this.onMainRunStateChange(running));
  }

  // ── Public API ───────────────────────────────────────────────────────────

  toggle(): void {
    if (this.el.classList.contains("chat-open")) this.hide();
    else this.show();
  }

  isOpen(): boolean {
    return this.el.classList.contains("chat-open");
  }

  /** Same effect as clicking the in-panel Start button — for external
   *  callers (e.g. the toolbar's Run-button redirect) that want to trigger
   *  it without going through the DOM. */
  startForChat(): void {
    void this.handleStart();
  }

  async show(): Promise<void> {
    if (!this.hasWebhookAndOutput()) {
      this.toast("This workflow needs a Webhook trigger and an Output node to use Chat.", "info");
      return;
    }
    this.applyToggles(this.wfManager.chatSettings);
    await this.loadStoreForCurrentWorkflow();
    await this.syncActiveWebhookPort();
    this.renderSessionLabel();
    this.renderMessages();
    this.el.classList.add("chat-open");
    document.body.classList.add("chat-panel-open");
    this.refreshRunningState();
    this.inputEl.focus();
  }

  hide(): void {
    this.el.classList.remove("chat-open");
    document.body.classList.remove("chat-panel-open");
    this.closeSessionMenu();
  }

  /** Call after any canvas change or workflow switch — Webhook/Output presence can change. */
  refreshButtonVisibility(): void {
    const show = this.hasWebhookAndOutput();
    this.chatBtn?.classList.toggle("hidden", !show);
    if (!show && this.el.classList.contains("chat-open")) this.hide();
  }

  /** Call on workflow navigation — the open session and pending request belong to the old workflow. */
  onWorkflowSwitched(): void {
    this.cancelPending();
    this.activeWebhookPort = null;
    this.schedulerRunning = false;
    this.mainRunActive = false;
    if (this.el.classList.contains("chat-open")) this.hide();
    this.refreshButtonVisibility();
  }

  /**
   * Reads workflow-scoped Chat settings (called on every panel open — see show()).
   *
   * `session_persistence` is read here but not enforced: persist() and
   * loadStoreForCurrentWorkflow() always persist to the backend chat
   * tables regardless of this toggle.
   */
  applyToggles(settings: ChatSettings): void {
    this.chatSettings = settings;

    if (settings.allow_attachments) {
      this.attachBtn.removeAttribute("aria-disabled");
      this.attachBtn.setAttribute("data-tooltip", "Attach file");
    } else {
      this.attachBtn.setAttribute("aria-disabled", "true");
      this.attachBtn.setAttribute("data-tooltip", "Attachments are off — enable in Workflow Settings → Chat");
    }
    // Settings can be toggled off while attachments are already queued (e.g. user
    // opens Workflow Settings without closing Chat). Drop them rather than leaving
    // an invisible queue that still gets sent on the next message.
    if (!settings.allow_attachments && this.pendingAttachments.length > 0) {
      this.pendingAttachments = [];
      this.renderPendingAttachments();
    }

    this.brandingEl?.classList.toggle("hidden", !settings.show_branding);

    this.inputEl.maxLength = settings.max_message_length;
    if (this.inputEl.value.length > settings.max_message_length) {
      this.inputEl.value = this.inputEl.value.slice(0, settings.max_message_length);
      this.autosizeInput();
    }
  }

  // ── Attachments  ────────────────────────────────────────────────

  /**
   * Remaining bytes available for attachment payload on the *next* send, after
   * reserving room for the current message-length limit and JSON overhead.
   * See CHAT_WEBHOOK_BODY_CAP_BYTES for why this is a hard cap, not a warning.
   * `* 3` is a worst-case bytes-per-char estimate (covers non-ASCII text);
   * actual usage is normally far lower for ASCII-heavy messages.
   */
  private attachmentBudgetBytes(): number {
    const textReserve = this.chatSettings.max_message_length * 3;
    return Math.max(0, CHAT_WEBHOOK_BODY_CAP_BYTES - textReserve - CHAT_BODY_JSON_OVERHEAD_BYTES);
  }

  private pendingAttachmentBytes(): number {
    // Base64 chars map 1:1 to wire bytes (the base64 alphabet needs no JSON escaping).
    return this.pendingAttachments.reduce((sum, a) => sum + a.data.length, 0);
  }

  private handleAttachClick(): void {
    if (!this.chatSettings.allow_attachments) return; // gated off — aria-disabled, not native disabled
    if (this.attachBtn.disabled) return;               // transient: still reading a previous file

    const fileInput = document.createElement("input");
    fileInput.type   = "file";
    fileInput.accept = CHAT_ATTACHMENT_ACCEPT;
    fileInput.addEventListener("change", () => {
      const file = fileInput.files?.[0];
      if (!file) return;

      const ext = file.name.slice(file.name.lastIndexOf(".")).toLowerCase();
      const fallbackMime = CHAT_ATTACHMENT_MIME_MAP[ext];
      if (!fallbackMime) {
        this.toast(`"${file.name}" is not a supported attachment type.`, "error");
        return;
      }

      // Estimate post-base64 size from raw bytes (base64 ≈ raw × 4/3) before reading
      // the file at all — avoids a wasted read for a file that can't be sent anyway.
      const estBytes = Math.ceil(file.size / 3) * 4;
      const budget    = this.attachmentBudgetBytes();
      const remaining = budget - this.pendingAttachmentBytes();
      if (estBytes > remaining) {
        this.toast(
          `"${file.name}" is too large to attach (about ${Math.max(0, Math.floor(remaining / 1024))} KB left).`,
          "error",
        );
        return;
      }

      this.attachBtn.disabled = true;
      const reader = new FileReader();

      reader.onerror = () => {
        this.attachBtn.disabled = false;
        this.toast(`Could not read "${file.name}".`, "error");
      };

      reader.onload = () => {
        this.attachBtn.disabled = false;

        // Guard: FileReader.result is typed string | ArrayBuffer | null; must be string here.
        const raw = reader.result;
        if (typeof raw !== "string") return;

        // Slice after the first comma — correct way to strip "data:<mime>;base64,".
        const b64      = raw.slice(raw.indexOf(",") + 1);
        const mimeType = file.type || fallbackMime;

        this.pendingAttachments.push({ filename: file.name, data: b64, mime_type: mimeType });
        this.renderPendingAttachments();
      };

      reader.readAsDataURL(file);
    });
    fileInput.click();
  }

  private renderPendingAttachments(): void {
    this.pendingAttachmentsEl.innerHTML = "";

    if (this.pendingAttachments.length === 0) {
      this.pendingAttachmentsEl.style.display = "none";
      return;
    }
    this.pendingAttachmentsEl.style.display = "";

    for (let i = 0; i < this.pendingAttachments.length; i++) {
      const att = this.pendingAttachments[i];

      const chip = document.createElement("div");
      chip.className = "chat-attachment-chip";

      const lbl = document.createElement("span");
      lbl.className = "chat-attachment-chip-label";
      lbl.innerHTML = CHAT_ATTACHMENT_FILE_ICON + " " + escapeHtml(att.filename);
      lbl.title = att.filename;
      chip.appendChild(lbl);

      const removeBtn = document.createElement("button");
      removeBtn.type      = "button";
      removeBtn.className = "chat-attachment-chip-remove";
      removeBtn.setAttribute("aria-label", "Remove " + att.filename);
      removeBtn.innerHTML  = CHAT_ATTACHMENT_X_ICON;
      const idx = i;
      removeBtn.addEventListener("click", () => {
        this.pendingAttachments.splice(idx, 1);
        this.renderPendingAttachments();
      });
      chip.appendChild(removeBtn);
      this.pendingAttachmentsEl.appendChild(chip);
    }

    const remainingKb = Math.max(0, Math.floor((this.attachmentBudgetBytes() - this.pendingAttachmentBytes()) / 1024));
    const hint = document.createElement("span");
    hint.className = "chat-attachment-hint" + (remainingKb < 100 ? " chat-attachment-hint--warn" : "");
    hint.textContent = `${remainingKb} KB left`;
    this.pendingAttachmentsEl.appendChild(hint);
  }

  // ── Canvas inspection ────────────────────────────────────────────────────
  // Assumes one Webhook node and one Output node per workflow — the chat
  // pattern this panel implements. With multiple of either, the first one
  // found in canvas iteration order is used; not currently configurable.

  private hasWebhookAndOutput(): boolean {
    let hasWebhook = false, hasOutput = false;
    for (const n of this.canvas.nodes.values()) {
      if (n.data.node_type_id === NODE_IDS.WEBHOOK) hasWebhook = true;
      if (n.data.node_type_id === NODE_IDS.OUTPUT)   hasOutput  = true;
      if (hasWebhook && hasOutput) return true;
    }
    return false;
  }

  private findWebhookConfig(): { port: number; path: string } | null {
    for (const n of this.canvas.nodes.values()) {
      if (n.data.node_type_id === NODE_IDS.WEBHOOK) {
        const cfg  = n.data.config;
        const port = Number(cfg["port"]) || 3456;
        const path = typeof cfg["path"] === "string" && cfg["path"] ? (cfg["path"] as string) : "/webhook";
        return { port, path };
      }
    }
    return null;
  }

  private findOutputNodeId(): string | null {
    for (const n of this.canvas.nodes.values()) {
      if (n.data.node_type_id === NODE_IDS.OUTPUT) return n.data.id;
    }
    return null;
  }

  // ── Running state ────────────────────────────────────────────────────────

  /**
   * Reacts to the main Run button's ad-hoc, one-shot execution (RunManager) —
   * distinct from the scheduler/Start-button path this panel otherwise
   * tracks via onSchedulerStatus. A Run-button execution never emits
   * scheduler-status events, and per this class's doc comment above, a
   * reply can only ever resolve through that event — so this does not
   * enable the input (that would promise a reply that can never arrive).
   * It only keeps the banner from going stale/misleading while a Run is
   * in flight or has just ended.
   */
  private onMainRunStateChange(running: boolean): void {
    this.mainRunActive = running;
    if (this.el.classList.contains("chat-open")) this.refreshRunningState();
  }

  /** Text shared by the top banner (history exists) and the centered
   *  empty-state (no history yet) for the "not running" case — kept in one
   *  place so the two surfaces can't drift out of sync with each other. */
  private notRunningStatusText(): string {
    return this.mainRunActive
      ? "Running as a one-time test (Run button) \u2014 replies aren't available this way. Stop it, then use Start below for an interactive chat session."
      : "Start this workflow to begin chatting";
  }

  private refreshRunningState(): void {
    const running     = this.schedulerRunning;
    const hasMessages = this.activeSession().messages.length > 0;

    // Top banner only makes sense once there's message history to anchor it
    // to; a fresh/empty session uses the centered empty-state instead.
    this.bannerEl.classList.toggle("hidden", running || !hasMessages);
    if (this.bannerTextEl) this.bannerTextEl.textContent = this.notRunningStatusText();

    if (this.emptyStateTextEl) {
      this.emptyStateTextEl.textContent = running
        ? "Send a message to start chatting with this workflow."
        : this.notRunningStatusText();
    }
    this.emptyStateStartBtn?.classList.toggle("hidden", running);

    this.inputEl.disabled = !running || this.awaitingReply;
    this.sendBtn.disabled = this.inputEl.disabled || this.inputEl.value.trim().length === 0;
  }

  private onSchedulerStatus(evt: SchedulerStatusEvent): void {
    if (evt.workflow_id !== this.wfManager.currentId) return;
    this.schedulerRunning = evt.status === "running" || evt.status === "waiting";
    if (this.el.classList.contains("chat-open")) this.refreshRunningState();
    // Unlike a timeout (workflow may still reply late — replyPending stays true
    // on purpose, see its declaration above), a user-initiated stop guarantees
    // no reply is coming: the workflow process is gone. Clear immediately
    // instead of leaving the typing bubble to expire via the 30s timer.
    if (this.replyPending && evt.status === "stopped") {
      this.cancelPending();
    }
    if (this.replyPending && (evt.status === "waiting" || evt.status === "error")) {
      this.resolvePending(evt);
    }
  }

  private async handleStart(): Promise<void> {
    this.startBtn.disabled = true;
    const startLabel = this.startBtn.textContent;
    const emptyStateStartLabel = this.emptyStateStartBtn?.textContent;
    this.startBtn.textContent = "Starting…";
    if (this.emptyStateStartBtn) {
      this.emptyStateStartBtn.disabled = true;
      this.emptyStateStartBtn.textContent = "Starting…";
    }
    try {
      const snapshot = await this.wfManager.prepareForBgRun();
      if (!snapshot) return;
      const jobs = await getScheduledJobs();
      const existing = jobs.find(j => j.workflow_id === snapshot.id);
      if (existing?.status === "active") {
        this.activeWebhookPort = this.extractWebhookPort(existing);
        this.refreshRunningState();
        return;
      }
      const usedFallback = await this.startOnFreePort(snapshot.id);
      if (!usedFallback) this.toast("Workflow started — you can chat now", "success");
    } catch (rawError) {
      const err = parseSchedulerError(String(rawError));
      switch (err.error_kind) {
        case "already_running":
          this.refreshRunningState();
          break;
        case "not_schedulable":
          this.toast("This workflow needs a Webhook trigger to use Chat.", "error");
          break;
        case "workflow_not_found":
          this.toast("Workflow not found in the database. Save it first.", "error");
          break;
        default:
          this.toast(`Could not start workflow: ${(err as { message?: string }).message ?? rawError}`, "error");
      }
    } finally {
      this.startBtn.disabled = false;
      this.startBtn.textContent = startLabel;
      if (this.emptyStateStartBtn) {
        this.emptyStateStartBtn.disabled = false;
        this.emptyStateStartBtn.textContent = emptyStateStartLabel ?? "Start";
      }
    }
  }

  /**
   * Starts the workflow's webhook job on its configured port; on a port
   * conflict, falls back to the next few ports instead of failing outright.
   * Safe specifically for Chat — the panel always looks up whatever port
   * actually got bound (see activeWebhookPort/extractWebhookPort) rather
   * than assuming the static config, unlike an external integration
   * (Stripe, GitHub, ...) that's hard-coded to call one fixed URL and would
   * break if this silently moved its port instead.
   */
  private async startOnFreePort(id: string): Promise<boolean> {
    const basePort = this.findWebhookConfig()?.port ?? 3456;
    const MAX_ATTEMPTS = 10;
    for (let i = 0; i < MAX_ATTEMPTS; i++) {
      const candidate = basePort + i;
      try {
        await startScheduledWorkflow(id, i === 0 ? undefined : candidate);
        this.activeWebhookPort = candidate;
        if (i > 0) this.toast(`Port ${basePort} was busy — started on ${candidate} instead.`, "info");
        return i > 0;
      } catch (rawError) {
        if (parseSchedulerError(String(rawError)).error_kind !== "port_conflict") throw rawError;
      }
    }
    throw new Error(`Ports ${basePort}–${basePort + MAX_ATTEMPTS - 1} are all in use — free one up or change the Webhook node's port.`);
  }

  /** start_job() persists the *effective* port (after any fallback) into
   *  trigger_kind — the only place guaranteed to reflect what's actually
   *  bound right now, since the node's own config never changes. */
  private extractWebhookPort(row: ScheduledJobRow): number | null {
    try {
      const trigger = JSON.parse(row.trigger_kind) as { port?: unknown };
      return typeof trigger.port === "number" ? trigger.port : null;
    } catch {
      return null;
    }
  }

  /** Refreshes schedulerRunning from the DB on every call — a background
   *  job's status can change while this panel is closed, so a value cached
   *  from a prior open would go stale. Also learns the real bound port for
   *  a job that was already running before this panel session ever called
   *  startOnFreePort() itself — e.g. started earlier via the Always-On
   *  toggle. Only fills the cache when it's still empty; a live restart's
   *  new port is instead picked up by refreshActiveWebhookPort() at send
   *  time, below. Checks the DB row's own status rather than
   *  isWorkflowRunning() — that flag doesn't distinguish this background-job
   *  path from an ad-hoc Run-button execution (see schedulerRunning above),
   *  and only flips once the post-bind scheduler-status event arrives, so
   *  relying on it here could also skip this lookup entirely if Chat opens
   *  before that event lands. */
  private async syncActiveWebhookPort(): Promise<void> {
    try {
      const jobs = await getScheduledJobs();
      const row = jobs.find(j => j.workflow_id === this.wfManager.currentId);
      this.schedulerRunning = row?.status === "active";
      if (this.activeWebhookPort === null && row?.status === "active") {
        this.activeWebhookPort = this.extractWebhookPort(row);
      }
    } catch (e) {
      console.error("Aerini: failed to sync active webhook port", e);
    }
  }

  /** Unlike syncActiveWebhookPort() above, overwrites the cache unconditionally
   *  rather than only filling it when empty — so a workflow restarted on a
   *  different port (BgJobsPanel, Always-On, or a port-conflict fallback)
   *  while Chat stayed open still resolves to wherever it's actually bound
   *  now. Uses getScheduledJob() (one row) rather than getScheduledJobs()
   *  (the full list) since this runs on every send. Leaves schedulerRunning
   *  untouched: a row that isn't "active" just leaves the last-known port
   *  in place and lets the fetch below fail into the existing "can't reach
   *  the webhook" recovery path instead. */
  private async refreshActiveWebhookPort(): Promise<void> {
    try {
      const row = await getScheduledJob(this.wfManager.currentId);
      if (row?.status === "active") {
        const port = this.extractWebhookPort(row);
        if (port !== null) this.activeWebhookPort = port;
      }
    } catch (e) {
      console.error("Aerini: failed to refresh active webhook port before send", e);
    }
  }

  /**
   * Recovery action for the "Could not reach the workflow's webhook" error —
   * the DB/UI believe the job is active but the actual listener isn't
   * answering. Forces a real stop+start (not just a resend) so a dead
   * listener gets a fresh bind before retrying the message.
   */
  private async handleRestartAndRetry(): Promise<void> {
    const id = this.wfManager.currentId;
    try {
      await stopScheduledWorkflow(id);
    } catch {
      // Already stopped/unbound on the backend — fine, proceed to start.
    }
    try {
      await this.startOnFreePort(id);
    } catch (rawError) {
      const err = parseSchedulerError(String(rawError));
      this.toast(`Restart failed: ${(err as { message?: string }).message ?? rawError}`, "error");
      return;
    }
    this.refreshRunningState();
    this.handleSend(this.lastSentText, this.lastSentAttachments);
  }

  // ── Sending ──────────────────────────────────────────────────────────────

  private async handleSend(overrideText?: string, overrideAttachments?: ChatAttachment[]): Promise<void> {
    const text = (overrideText ?? this.inputEl.value).trim();
    if (!text) return;
    if (text.length > this.chatSettings.max_message_length) {
      this.toast(`Message exceeds the ${this.chatSettings.max_message_length}-character limit.`, "error");
      return;
    }
    if (this.awaitingReply) return;
    if (!this.schedulerRunning) { this.refreshRunningState(); return; }

    const webhook = this.findWebhookConfig();
    if (!webhook) { this.toast("No Webhook node found on this workflow.", "error"); return; }

    // Captured before any clear below — reassigning this.pendingAttachments to a new
    // array (not mutating it) means this reference stays valid either way.
    const attachments = overrideAttachments ?? this.pendingAttachments;

    this.lastSentText        = text;
    this.lastSentAttachments = attachments;
    if (!overrideText) {
      this.inputEl.value = "";
      this.autosizeInput();
      this.appendMessage({
        id: crypto.randomUUID(), role: "user", text,
        attachments: attachments.length > 0 ? attachments : undefined,
        timestamp: Date.now(),
      });
      this.persist();
      this.pendingAttachments = [];
      this.renderPendingAttachments();
    }

    this.awaitingReply      = true;
    this.replyPending       = true;
    this.inputEl.disabled   = true;
    this.sendBtn.disabled   = true;
    this.appendTypingBubble();

    const session = this.activeSession();
    // `attachments` key only added when non-empty — keeps the wire shape
    // unchanged for every workflow that doesn't use this feature.
    const payload: Record<string, unknown> = { message: text, session_id: session.id };
    if (attachments.length > 0) payload.attachments = attachments;

    await this.refreshActiveWebhookPort();

    try {
      const port = this.activeWebhookPort ?? webhook.port;
      await fetch(`http://127.0.0.1:${port}${webhook.path}`, {
        method:  "POST",
        headers: { "content-type": "application/json" },
        body:    JSON.stringify(payload),
      });
    } catch {
      this.replyPending = false; // request never sent — no event will ever resolve it
      this.failPending("Could not reach the workflow's webhook. Is it still running?", true);
      return;
    }

    this.pendingTimer = setTimeout(() => {
      this.failPending("No response after 30s. The workflow may be busy or the message was dropped.");
    }, RESPONSE_TIMEOUT_MS);
  }

  private resolvePending(evt: SchedulerStatusEvent): void {
    this.clearPendingTimer();
    this.awaitingReply = false;
    this.replyPending  = false;
    this.refreshRunningState();

    if (evt.status === "error") {
      this.replaceTypingBubbleWithError(evt.last_error ?? "Workflow run failed.", true);
      return;
    }
    const result = evt.last_result as WorkflowResult | null;
    if (!result) { this.removeTypingBubble(); return; }
    if (!result.success) {
      const errLog = result.logs.find(l => l.level === "error");
      this.replaceTypingBubbleWithError(result.error ?? errLog?.message ?? "Workflow run failed.", true);
      return;
    }

    const outputId = this.findOutputNodeId();
    const raw = outputId ? result.node_outputs[outputId] : undefined;
    this.renderAiReply(raw);
  }

  private failPending(message: string, restartRetry = false): void {
    this.clearPendingTimer();
    this.awaitingReply = false;
    this.refreshRunningState();
    this.replaceTypingBubbleWithError(message, true, restartRetry);
  }

  private cancelPending(): void {
    this.clearPendingTimer();
    this.awaitingReply = false;
    this.replyPending  = false;
    this.removeTypingBubble();
  }

  private clearPendingTimer(): void {
    if (this.pendingTimer) { clearTimeout(this.pendingTimer); this.pendingTimer = null; }
  }

  // ── Reply rendering ──────────────────────────────────────────────────────

  private renderAiReply(raw: unknown): void {
    this.removeTypingBubble();

    if (raw === undefined || raw === null) {
      this.appendMessage({
        id: crypto.randomUUID(), role: "ai",
        text: "_Workflow completed but the Output node produced nothing._",
        timestamp: Date.now(),
      });
      this.persist();
      return;
    }

    const obj          = raw as Record<string, unknown>;
    const isMediaBatch = obj["output_type"] === "media_batch";
    const value        = obj["value"];

    if (isMediaBatch && this.isFilesContainer(value)) {
      if (!this.chatSettings.allow_image_responses) {
        this.appendMessage({
          id: crypto.randomUUID(), role: "ai",
          text: "_Image responses are disabled in Workflow Settings._",
          timestamp: Date.now(),
        });
        this.persist();
        return;
      }
      const images = value.files.filter((f): f is ChatImageFile => this.isImageFile(f));
      if (images.length > 0) {
        this.appendMessage({ id: crypto.randomUUID(), role: "ai", images, timestamp: Date.now() });
        this.persist();
        return;
      }
    }

    let text: string;
    if (typeof value === "string")        text = value;
    else if (value === undefined || value === null) text = JSON.stringify(obj, null, 2);
    else                                   text = JSON.stringify(value, null, 2);

    this.appendMessage({ id: crypto.randomUUID(), role: "ai", text, timestamp: Date.now() });
    this.persist();
  }

  private isFilesContainer(value: unknown): value is { files: unknown[] } {
    return !!value && typeof value === "object" && Array.isArray((value as Record<string, unknown>)["files"]);
  }

  private isImageFile(f: unknown): f is ChatImageFile {
    if (!f || typeof f !== "object") return false;
    const o = f as Record<string, unknown>;
    return typeof o["filename"] === "string" && typeof o["data"] === "string"
        && typeof o["mime_type"] === "string" && (o["mime_type"] as string).startsWith("image/");
  }

  // ── Message list rendering ───────────────────────────────────────────────

  private appendMessage(msg: ChatMessage): void {
    this.activeSession().messages.push(msg);
    this.renderOneMessage(msg);
    this.scrollIfAtBottom();
  }

  private renderMessages(): void {
    this.messagesEl.innerHTML = "";
    this.emptyStateTextEl = null;
    this.emptyStateStartBtn = null;
    const session = this.activeSession();
    if (session.messages.length === 0) {
      this.messagesEl.appendChild(this.buildEmptyState());
      this.refreshRunningState(); // freshly built — sync text/button to current state now
      return;
    }
    for (const m of session.messages) this.renderOneMessage(m);
    this.messagesEl.scrollTop = this.messagesEl.scrollHeight;
  }

  /** Centered "no messages yet" state: explanatory text above a single Start
   *  action, replacing the old top banner for this case (see refreshRunningState). */
  private buildEmptyState(): HTMLElement {
    const wrap = document.createElement("div");
    wrap.className = "chat-empty-state";

    const text = document.createElement("p");
    text.className = "chat-empty-state-text";
    this.emptyStateTextEl = text;

    const btn = document.createElement("button");
    btn.className = "btn-primary chat-empty-state-start";
    btn.textContent = "Start";
    btn.addEventListener("click", () => this.handleStart());
    this.emptyStateStartBtn = btn;

    wrap.appendChild(text);
    wrap.appendChild(btn);
    return wrap;
  }

  /** Clears the centered empty-state block (if present) and its field refs —
   *  called wherever real content is about to appear in .chat-messages, so a
   *  stale Start button/text never lingers alongside an actual message. */
  private removeEmptyState(): void {
    this.messagesEl.querySelector(".chat-empty-state")?.remove();
    this.emptyStateTextEl = null;
    this.emptyStateStartBtn = null;
  }

  private renderOneMessage(msg: ChatMessage): void {
    this.removeEmptyState();
    const row = document.createElement("div");
    row.className = `chat-bubble-row chat-bubble-row--${msg.role === "user" ? "user" : "ai"}`;
    row.appendChild(
      msg.images && msg.images.length > 0
        ? this.buildImageBubble(msg.images)
        : this.buildTextBubble(msg.text ?? "", msg.role, msg.attachments)
    );
    this.messagesEl.appendChild(row);
  }

  private buildTextBubble(text: string, role: ChatMessage["role"], attachments?: ChatAttachment[]): HTMLElement {
    const bubble = document.createElement("div");
    bubble.className = role === "user"
      ? "chat-bubble chat-bubble--user"
      : role === "error" ? "chat-bubble chat-bubble--ai chat-bubble--error" : "chat-bubble chat-bubble--ai";

    const content = document.createElement("div");
    content.innerHTML = this.renderMarkdown(text);
    bubble.appendChild(content);

    for (const att of attachments ?? []) bubble.appendChild(this.buildSentAttachmentChip(att));

    const copyBtn = document.createElement("button");
    copyBtn.className = "chat-bubble-copy";
    copyBtn.textContent = "Copy";
    copyBtn.addEventListener("click", () => {
      navigator.clipboard.writeText(text).then(() => {
        copyBtn.textContent = "Copied";
        setTimeout(() => { copyBtn.textContent = "Copy"; }, 1200);
      }).catch(() => { /* clipboard unavailable — non-fatal */ });
    });
    bubble.appendChild(copyBtn);
    return bubble;
  }

  private buildSentAttachmentChip(att: ChatAttachment): HTMLElement {
    const chip = document.createElement("div");
    chip.className = "chat-attachment-chip";

    const lbl = document.createElement("span");
    lbl.className = "chat-attachment-chip-label";
    lbl.innerHTML = CHAT_ATTACHMENT_FILE_ICON;
    // Filename is user-supplied — inserted as a text node, never as markup.
    lbl.appendChild(document.createTextNode(" " + att.filename));
    lbl.title = att.filename;
    chip.appendChild(lbl);
    return chip;
  }

  private renderMarkdown(text: string): string {
    try {
      return DOMPurify.sanitize(marked.parse(text) as string);
    } catch {
      return DOMPurify.sanitize(`<p>${escapeHtml(text)}</p>`);
    }
  }

  private buildImageBubble(images: ChatImageFile[]): HTMLElement {
    const bubble = document.createElement("div");
    bubble.className = "chat-bubble chat-bubble--ai";
    const grid = document.createElement("div");
    grid.className = images.length > 1 ? "chat-image-grid chat-image-grid--multi" : "chat-image-grid";

    for (const img of images) {
      const src = `data:${img.mime_type};base64,${img.data}`;
      const wrap = document.createElement("div");
      wrap.className = "chat-image-wrap";

      const el = document.createElement("img");
      el.src = src;
      el.alt = img.filename;
      wrap.appendChild(el);
      wrap.addEventListener("click", () => this.openLightbox(src));

      const overlay = document.createElement("div");
      overlay.className = "chat-image-overlay";

      const copyBtn = document.createElement("button");
      copyBtn.title = "Copy image";
      copyBtn.setAttribute("data-tooltip", "Copy image");
      copyBtn.innerHTML = `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>`;
      copyBtn.addEventListener("click", (e) => { e.stopPropagation(); this.copyImage(img, copyBtn); });

      const dlBtn = document.createElement("button");
      dlBtn.title = "Download";
      dlBtn.setAttribute("data-tooltip", "Download");
      dlBtn.innerHTML = `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>`;
      dlBtn.addEventListener("click", (e) => { e.stopPropagation(); this.downloadImage(img); });

      overlay.appendChild(copyBtn);
      overlay.appendChild(dlBtn);
      wrap.appendChild(overlay);
      grid.appendChild(wrap);
    }
    bubble.appendChild(grid);
    return bubble;
  }

  private base64ToBlob(img: ChatImageFile): Blob {
    const bytes = atob(img.data);
    const arr = new Uint8Array(bytes.length);
    for (let i = 0; i < bytes.length; i++) arr[i] = bytes.charCodeAt(i);
    return new Blob([arr], { type: img.mime_type });
  }

  private downloadImage(img: ChatImageFile): void {
    try {
      const url = URL.createObjectURL(this.base64ToBlob(img));
      const a = document.createElement("a");
      a.href = url; a.download = img.filename || "image";
      document.body.appendChild(a); a.click(); a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch {
      this.toast("Could not download image.", "error");
    }
  }

  private async copyImage(img: ChatImageFile, copyBtn: HTMLButtonElement): Promise<void> {
    try {
      if (typeof ClipboardItem === "undefined") throw new Error("ClipboardItem unsupported");
      await navigator.clipboard.write([new ClipboardItem({ [img.mime_type]: this.base64ToBlob(img) })]);
      this.toast("Image copied to clipboard", "success");
    } catch {
      // ClipboardItem unsupported in this webview — fall back to download-only.
      copyBtn.style.display = "none";
      this.downloadImage(img);
    }
  }

  private openLightbox(src: string): void {
    const overlay = document.createElement("div");
    overlay.className = "chat-lightbox";
    const img = document.createElement("img");
    img.src = src;
    overlay.appendChild(img);

    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") close(); };
    const close = () => { overlay.remove(); document.removeEventListener("keydown", onKey); };
    overlay.addEventListener("click", (e) => { if (e.target === overlay) close(); });
    document.addEventListener("keydown", onKey);
    document.body.appendChild(overlay);
  }

  private appendTypingBubble(): void {
    this.removeEmptyState();
    const row = document.createElement("div");
    row.className = "chat-bubble-row chat-bubble-row--ai";
    row.id = "chat-typing-row";
    const bubble = document.createElement("div");
    bubble.className = "chat-bubble chat-bubble--ai";
    bubble.innerHTML = `<div class="chat-typing"><span></span><span></span><span></span></div>`;
    row.appendChild(bubble);
    this.messagesEl.appendChild(row);
    this.scrollIfAtBottom();
  }

  private removeTypingBubble(): void {
    document.getElementById("chat-typing-row")?.remove();
  }

  private replaceTypingBubbleWithError(message: string, withRetry = false, restartRetry = false): void {
    this.removeTypingBubble();
    const row = document.createElement("div");
    row.className = "chat-bubble-row chat-bubble-row--ai";
    const bubble = document.createElement("div");
    bubble.className = "chat-bubble chat-bubble--ai chat-bubble--error";
    bubble.textContent = message;
    if (withRetry) {
      bubble.appendChild(document.createElement("br"));
      const retry = document.createElement("button");
      retry.className = "chat-retry-btn";
      if (restartRetry) {
        retry.textContent = "Restart & Retry";
        retry.addEventListener("click", () => { row.remove(); this.handleRestartAndRetry(); });
      } else {
        retry.textContent = "Retry";
        retry.addEventListener("click", () => { row.remove(); this.handleSend(this.lastSentText, this.lastSentAttachments); });
      }
      bubble.appendChild(retry);
    }
    row.appendChild(bubble);
    this.messagesEl.appendChild(row);
    this.scrollIfAtBottom();
  }

  // ── Sessions ──────────────────────────────────────────────────────────────

  private activeSession(): ChatSession {
    let s = this.store.sessions.find(s => s.id === this.store.activeId);
    if (!s) {
      s = this.newSessionObject();
      this.store.sessions.unshift(s);
      this.store.activeId = s.id;
    }
    return s;
  }

  private newSessionObject(): ChatSession {
    return { id: crypto.randomUUID(), name: `Session ${new Date().toLocaleString()}`, messages: [], createdAt: Date.now() };
  }

  /** Legacy localStorage key — read (and cleared) only by the one-time migration below. */
  private storageKey(workflowId: string): string {
    return `aerini_chat_${workflowId}`;
  }

  /** Per-workflow "which session is active" pointer, stored via the existing generic settings table. */
  private activeSessionSettingKey(workflowId: string): string {
    return `chat_active_session:${workflowId}`;
  }

  private sessionFromWire(s: ChatSessionWire): ChatSession {
    return {
      id: s.id,
      name: s.name,
      createdAt: s.created_at,
      messages: s.messages.map((m) => ({
        id: m.id,
        role: m.role as ChatMessage["role"],
        text: m.text ?? undefined,
        images: m.images ?? undefined,
        attachments: m.attachments ?? undefined,
        timestamp: m.timestamp,
      })),
    };
  }

  private sessionToWire(s: ChatSession, workflowId: string): ChatSessionWire {
    return {
      id: s.id,
      workflow_id: workflowId,
      name: s.name,
      created_at: s.createdAt,
      messages: s.messages.map((m) => ({
        id: m.id, role: m.role, text: m.text, images: m.images, attachments: m.attachments, timestamp: m.timestamp,
      })),
    };
  }

  /**
   * One-time migration for a workflow that has legacy localStorage chat data
   * but no rows in the backend yet. Reads the old blob, saves each session
   * through the same backend calls persist() now uses, then removes the old
   * key. Returns the migrated store, or null if there was nothing to migrate.
   */
  private async migrateLegacyLocalStorage(workflowId: string): Promise<ChatStore | null> {
    let raw: string | null = null;
    try { raw = localStorage.getItem(this.storageKey(workflowId)); } catch { return null; }
    if (!raw) return null;

    let parsed: ChatStore | null = null;
    try {
      const p = JSON.parse(raw) as ChatStore;
      if (p && Array.isArray(p.sessions) && p.sessions.length > 0) parsed = p;
    } catch { /* corrupt legacy blob — nothing worth migrating */ }
    if (!parsed) return null;

    for (const session of parsed.sessions) {
      await saveChatSession(this.sessionToWire(session, workflowId));
    }
    if (parsed.activeId) {
      await setSetting(this.activeSessionSettingKey(workflowId), parsed.activeId);
    }
    try { localStorage.removeItem(this.storageKey(workflowId)); } catch { /* best-effort cleanup only */ }
    return parsed;
  }

  /**
   * `workflowId` is captured once at the top and threaded through every
   * awaited call below instead of re-reading `wfManager.currentId` after
   * each `await` — if the user switches workflows while this is in flight,
   * every `this.store = ...` assignment below is guarded so a late-resolving
   * load for the OLD workflow can't clobber a newer load that already won.
   */
  private async loadStoreForCurrentWorkflow(): Promise<void> {
    const workflowId = this.wfManager.currentId;
    try {
      const sessions = await listChatSessions(workflowId);
      if (sessions.length > 0) {
        const activeId = (await getSetting(this.activeSessionSettingKey(workflowId))) ?? sessions[0].id;
        if (this.wfManager.currentId === workflowId) {
          this.store = { sessions: sessions.map((s) => this.sessionFromWire(s)), activeId };
        }
        return;
      }
      const migrated = await this.migrateLegacyLocalStorage(workflowId);
      if (migrated) {
        if (this.wfManager.currentId === workflowId) this.store = migrated;
        return;
      }
    } catch (e) {
      console.error("Aerini: failed to load chat sessions from backend", e);
    }
    if (this.wfManager.currentId === workflowId) this.store = { sessions: [], activeId: "" };
  }

  private async persist(): Promise<void> {
    const workflowId = this.wfManager.currentId;
    try {
      await saveChatSession(this.sessionToWire(this.activeSession(), workflowId));
      await setSetting(this.activeSessionSettingKey(workflowId), this.store.activeId);
      this.persistFailureToasted = false;
    } catch {
      if (!this.persistFailureToasted) {
        this.persistFailureToasted = true;
        this.toast("Chat history isn't saving — check the app's connection to its backend.", "error");
      }
    }
  }

  private renderSessionLabel(): void {
    const name = this.activeSession().name;
    this.sessionLabel.textContent = name;
    this.sessionLabel.title = name;
  }

  private renderSessionMenu(): void {
    this.sessionMenu.innerHTML = "";
    for (const s of this.store.sessions) {
      const item = document.createElement("button");
      item.className = "toolbar-dropdown-item chat-session-item";
      item.innerHTML = `<span class="chat-session-item-name" title="${escapeHtml(s.name)}">${escapeHtml(s.name)}</span>`;
      const del = document.createElement("span");
      del.className = "chat-session-item-del";
      del.innerHTML = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
      del.addEventListener("click", (e) => { e.stopPropagation(); this.deleteSession(s.id); });
      item.appendChild(del);
      item.addEventListener("click", () => this.switchSession(s.id));
      this.sessionMenu.appendChild(item);
    }
    const newBtn = document.createElement("button");
    newBtn.className = "toolbar-dropdown-item chat-session-new";
    newBtn.textContent = "+ New Session";
    newBtn.addEventListener("click", () => this.createNewSession());
    this.sessionMenu.appendChild(newBtn);
  }

  private switchSession(id: string): void {
    if (this.awaitingReply) { this.toast("Wait for the current reply before switching sessions.", "info"); return; }
    this.store.activeId = id;
    this.persist();
    this.renderSessionLabel();
    this.renderMessages();
    this.closeSessionMenu();
  }

  private createNewSession(): void {
    if (this.awaitingReply) { this.toast("Wait for the current reply before starting a new session.", "info"); return; }
    const s = this.newSessionObject();
    this.store.sessions.unshift(s);
    this.store.activeId = s.id;
    this.persist();
    this.renderSessionLabel();
    this.renderMessages();
    this.closeSessionMenu();
  }

  private async deleteSession(id: string): Promise<void> {
    if (this.store.sessions.length <= 1) { this.toast("Can't delete the only session.", "info"); return; }
    try {
      await clearChatSession(id);
    } catch (e) {
      console.error("Aerini: clearChatSession failed", e);
      this.toast("Could not clear AI memory on the backend — session was deleted locally.", "error");
    }
    this.store.sessions = this.store.sessions.filter(s => s.id !== id);
    if (this.store.activeId === id) this.store.activeId = this.store.sessions[0].id;
    try {
      await deleteChatSession(id);
    } catch (e) {
      console.error("Aerini: deleteChatSession failed", e);
      this.toast("Could not remove the session from storage — it may reappear after restart.", "error");
    }
    this.persist();
    this.renderSessionLabel();
    this.renderMessages();
    this.renderSessionMenu();
  }

  private toggleSessionMenu(): void {
    this.renderSessionMenu();
    this.sessionMenu.classList.toggle("open");
  }

  private closeSessionMenu(): void {
    this.sessionMenu.classList.remove("open");
  }

  /**
   * Clears AI Memory for the active session server-side, then starts a fresh
   * local session in its place. (deleteSession(), above, does the equivalent
   * clearChatSession call for the session-menu delete path.)
   */
  private async handleClear(): Promise<void> {
    const clearBtn = document.getElementById("btn-chat-clear") as HTMLButtonElement | null;
    const clearLabel = clearBtn?.textContent;
    if (clearBtn) { clearBtn.disabled = true; clearBtn.textContent = "Clearing…"; }
    try {
      if (this.awaitingReply) this.cancelPending();
      const current = this.activeSession();
      try {
        await clearChatSession(current.id);
      } catch (e) {
        console.error("Aerini: clearChatSession failed", e);
        this.toast("Could not clear AI memory on the backend — chat history was cleared locally.", "error");
      }
      const idx = this.store.sessions.findIndex(s => s.id === current.id);
      const fresh = this.newSessionObject();
      fresh.name = current.name;
      this.store.sessions[idx] = fresh;
      this.store.activeId = fresh.id;
      try {
        await deleteChatSession(current.id);
      } catch (e) {
        console.error("Aerini: deleteChatSession failed", e);
      }
      this.persist();
      this.renderSessionLabel();
      this.renderMessages();
    } finally {
      if (clearBtn) { clearBtn.disabled = false; clearBtn.textContent = clearLabel ?? "Clear"; }
    }
  }

  // ── Static DOM bindings ──────────────────────────────────────────────────

  private bindStaticEvents(): void {
    document.getElementById("btn-chat-close")?.addEventListener("click", () => this.hide());
    this.startBtn.addEventListener("click", () => this.handleStart());
    document.getElementById("btn-chat-clear")?.addEventListener("click", () => this.handleClear());
    this.sendBtn.addEventListener("click", () => this.handleSend());
    this.attachBtn.addEventListener("click", () => this.handleAttachClick());

    document.getElementById("chat-session-btn")?.addEventListener("click", (e) => {
      e.stopPropagation();
      this.toggleSessionMenu();
    });
    document.addEventListener("click", () => this.closeSessionMenu());

    this.inputEl.addEventListener("input", () => {
      this.autosizeInput();
      this.sendBtn.disabled = this.inputEl.disabled || this.inputEl.value.trim().length === 0;
    });
    this.inputEl.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); this.handleSend(); }
    });
  }

  private autosizeInput(): void {
    this.inputEl.style.height = "auto";
    this.inputEl.style.height = `${Math.min(this.inputEl.scrollHeight, 120)}px`;
  }

  private scrollIfAtBottom(): void {
    const threshold = 60;
    const atBottom = this.messagesEl.scrollHeight - this.messagesEl.scrollTop - this.messagesEl.clientHeight < threshold;
    if (atBottom) {
      requestAnimationFrame(() => { this.messagesEl.scrollTop = this.messagesEl.scrollHeight; });
    }
  }
}
