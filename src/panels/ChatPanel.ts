import { marked } from "marked";
import DOMPurify from "dompurify";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowManager } from "../workflow-manager";
import { isWorkflowRunning } from "../workflow-manager";
import { NODE_IDS } from "../node-ids";
import { startScheduledWorkflow, getScheduledJobs, parseSchedulerError, clearChatSession } from "../ipc/workflow";
import type { WorkflowResult } from "../ipc/workflow";
import type { SchedulerStatusEvent } from "../ipc/events";
import { addSchedulerStatusListener } from "../scheduler-events";
import { escapeHtml } from "../utils";
import { type ChatSettings, DEFAULT_CHAT_SETTINGS } from "../canvas/CanvasSerializer";

type Toast = (msg: string, type?: "success" | "error" | "info") => void;

const RESPONSE_TIMEOUT_MS = 30_000;

interface ChatImageFile { filename: string; data: string; mime_type: string; }

/** Outgoing user attachment — same wire shape as ChatImageFile but not image-only (Patch 7). */
interface ChatAttachment { filename: string; data: string; mime_type: string; }

// Accepted types match Patch 6 (AI Prompt static attachments UI) exactly.
const CHAT_ATTACHMENT_ACCEPT = ".png,.jpg,.jpeg,.webp,.gif,.pdf,.txt,.md";

// file.type is unreliable for non-standard extensions (e.g. .md on Windows) — same fallback as Patch 6.
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

// Same icon set as Patch 6's attachment chips (popover/extensions/ai-prompt.ts) — no emoji, themeable via currentColor.
const CHAT_ATTACHMENT_FILE_ICON = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>`;
const CHAT_ATTACHMENT_X_ICON    = `<svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" aria-hidden="true"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;

/**
 * Hard wire-transmissibility ceiling, not a soft "file size" warning. Mirrors
 * aerini-engine/src/nodes/webhook.rs `MAX_BODY_BYTES` (VERIFIED in source,
 * 2026-06-25) — the local webhook server rejects any request body over this
 * many bytes. handleSend()'s fetch() does not check response.ok, so a body
 * that exceeds this would not error immediately; it would silently fail and
 * surface 30s later as a misleading "No response after 30s" timeout. This is
 * therefore enforced as a hard client-side block, not a dismissible warning
 * (deliberate deviation from Patch 6's pattern — there, oversized attachments
 * only bloat the saved workflow file and never fail outright).
 */
const CHAT_WEBHOOK_BODY_CAP_BYTES = 1_000_000;
/** Reserve for JSON structure, session_id, and per-file filename/mime_type strings. */
const CHAT_BODY_JSON_OVERHEAD_BYTES = 2_000;

interface ChatMessage {
  id: string;
  role: "user" | "ai" | "error";
  text?: string;
  images?: ChatImageFile[];
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
 * Chat Panel — Patch 5A (frontend only).
 *
 * IMPORTANT — message flow does NOT use the webhook's HTTP response.
 * The Webhook node (aerini-engine/src/nodes/webhook.rs, scheduler/runner.rs)
 * always replies "OK" over HTTP regardless of what the workflow produces —
 * there is no code path that holds the connection open for a result. The
 * actual reply arrives asynchronously as a `scheduler-status` Tauri event
 * with `last_result.node_outputs[<Output node id>]` once the background run
 * completes. `fetch()` below is fire-and-forget; resolution happens in
 * onSchedulerStatus(). See chat session notes for the full discrepancy
 * writeup against the original plan.
 */
export class ChatPanel {
  private el:           HTMLElement;
  private messagesEl:   HTMLElement;
  private inputEl:      HTMLTextAreaElement;
  private sendBtn:      HTMLButtonElement;
  private bannerEl:     HTMLElement;
  private startBtn:     HTMLButtonElement;
  private sessionLabel: HTMLElement;
  private sessionMenu:  HTMLElement;
  private chatBtn:      HTMLButtonElement | null;
  private attachBtn:    HTMLButtonElement;
  private brandingEl:   HTMLElement | null;
  /** Container for pending-attachment chips, inserted above .chat-input-row (Patch 7). No matching static markup in index.html — created here, mirroring the existing pattern of programmatic DOM construction elsewhere in this file (session menu, lightbox). */
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
  /** Files attached to the next outgoing message, not yet sent (Patch 7). */
  private pendingAttachments:  ChatAttachment[] = [];
  /** Snapshot of what was actually sent, for the error-bubble Retry button. */
  private lastSentAttachments: ChatAttachment[] = [];

  constructor(canvas: Canvas, wfManager: WorkflowManager, toast: Toast) {
    this.canvas    = canvas;
    this.wfManager = wfManager;
    this.toast     = toast;

    this.el           = document.getElementById("chat-panel")!;
    this.messagesEl   = document.getElementById("chat-messages")!;
    this.inputEl      = document.getElementById("chat-input") as HTMLTextAreaElement;
    this.sendBtn      = document.getElementById("chat-send-btn") as HTMLButtonElement;
    this.bannerEl     = document.getElementById("chat-not-running-banner")!;
    this.startBtn     = document.getElementById("btn-chat-start") as HTMLButtonElement;
    this.sessionLabel = document.getElementById("chat-session-label")!;
    this.sessionMenu  = document.getElementById("chat-session-menu")!;
    this.chatBtn      = document.getElementById("btn-chat") as HTMLButtonElement | null;
    this.attachBtn    = document.getElementById("chat-attach-btn") as HTMLButtonElement;
    this.brandingEl   = document.getElementById("chat-branding-footer");

    // No static markup for this in index.html (Patch 7) — built and inserted here,
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
  }

  // ── Public API ───────────────────────────────────────────────────────────

  toggle(): void {
    if (this.el.classList.contains("chat-open")) this.hide();
    else this.show();
  }

  show(): void {
    if (!this.hasWebhookAndOutput()) {
      this.toast("This workflow needs a Webhook trigger and an Output node to use Chat.", "info");
      return;
    }
    this.applyToggles(this.wfManager.chatSettings);
    this.loadStoreForCurrentWorkflow();
    this.renderSessionLabel();
    this.renderMessages();
    this.el.classList.add("chat-open");
    this.refreshRunningState();
    this.inputEl.focus();
  }

  hide(): void {
    this.el.classList.remove("chat-open");
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
    if (this.el.classList.contains("chat-open")) this.hide();
    this.refreshButtonVisibility();
  }

  /**
   * Reads workflow-scoped Chat settings (called on every panel open — see show()).
   *
   * Scope note: `session_persistence` is read and serialized correctly but not
   * yet enforced here — persist()/loadStoreForCurrentWorkflow() always use
   * localStorage regardless of this toggle. Enforcing it requires changing
   * show()'s per-open reload pattern, which is out of this patch's listed
   * file/behavior scope; flagging per rules.md Rule 6 rather than expanding
   * scope silently.
   */
  applyToggles(settings: ChatSettings): void {
    this.chatSettings = settings;

    this.attachBtn.style.display = settings.allow_attachments ? "" : "none";
    this.attachBtn.disabled      = !settings.allow_attachments;
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

  // ── Attachments (Patch 7) ────────────────────────────────────────────────

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
    if (this.attachBtn.disabled) return;

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
        this.attachBtn.disabled = !this.chatSettings.allow_attachments;
        this.toast(`Could not read "${file.name}".`, "error");
      };

      reader.onload = () => {
        this.attachBtn.disabled = !this.chatSettings.allow_attachments;

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
  // found in canvas iteration order is used; not configurable in this patch.

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

  private refreshRunningState(): void {
    const running = isWorkflowRunning(this.wfManager.currentId);
    this.bannerEl.classList.toggle("hidden", running);
    this.inputEl.disabled = !running || this.awaitingReply;
    this.sendBtn.disabled = this.inputEl.disabled || this.inputEl.value.trim().length === 0;
  }

  private onSchedulerStatus(evt: SchedulerStatusEvent): void {
    if (evt.workflow_id !== this.wfManager.currentId) return;
    if (this.el.classList.contains("chat-open")) this.refreshRunningState();
    if (this.replyPending && (evt.status === "waiting" || evt.status === "error")) {
      this.resolvePending(evt);
    }
  }

  private async handleStart(): Promise<void> {
    this.startBtn.disabled = true;
    try {
      const snapshot = await this.wfManager.prepareForBgRun();
      if (!snapshot) return;
      const jobs = await getScheduledJobs();
      const existing = jobs.find(j => j.workflow_id === snapshot.id);
      if (existing?.status === "active") { this.refreshRunningState(); return; }
      await startScheduledWorkflow(snapshot.id);
      this.toast("Workflow started — you can chat now", "success");
    } catch (rawError) {
      const err = parseSchedulerError(String(rawError));
      switch (err.error_kind) {
        case "already_running":
          this.refreshRunningState();
          break;
        case "port_conflict":
          this.toast(`Port ${err.port} is already in use by "${err.held_by_workflow_name}".`, "error");
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
    }
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
    if (!isWorkflowRunning(this.wfManager.currentId)) { this.refreshRunningState(); return; }

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
      this.appendMessage({ id: crypto.randomUUID(), role: "user", text, timestamp: Date.now() });
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
    // `attachments` key only added when non-empty — keeps the wire shape unchanged
    // for every workflow that doesn't use this feature (Rule 9 regression check).
    const payload: Record<string, unknown> = { message: text, session_id: session.id };
    if (attachments.length > 0) payload.attachments = attachments;

    try {
      await fetch(`http://127.0.0.1:${webhook.port}${webhook.path}`, {
        method:  "POST",
        headers: { "content-type": "application/json" },
        body:    JSON.stringify(payload),
      });
    } catch {
      this.replyPending = false; // request never sent — no event will ever resolve it
      this.failPending("Could not reach the workflow's webhook. Is it still running?");
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

  private failPending(message: string): void {
    this.clearPendingTimer();
    this.awaitingReply = false;
    this.refreshRunningState();
    this.replaceTypingBubbleWithError(message, true);
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
    const session = this.activeSession();
    if (session.messages.length === 0) {
      const hint = document.createElement("div");
      hint.className = "chat-empty-hint";
      hint.textContent = "Send a message to start chatting with this workflow.";
      this.messagesEl.appendChild(hint);
      return;
    }
    for (const m of session.messages) this.renderOneMessage(m);
    this.messagesEl.scrollTop = this.messagesEl.scrollHeight;
  }

  private renderOneMessage(msg: ChatMessage): void {
    this.messagesEl.querySelector(".chat-empty-hint")?.remove();
    const row = document.createElement("div");
    row.className = `chat-bubble-row chat-bubble-row--${msg.role === "user" ? "user" : "ai"}`;
    row.appendChild(
      msg.images && msg.images.length > 0
        ? this.buildImageBubble(msg.images)
        : this.buildTextBubble(msg.text ?? "", msg.role)
    );
    this.messagesEl.appendChild(row);
  }

  private buildTextBubble(text: string, role: ChatMessage["role"]): HTMLElement {
    const bubble = document.createElement("div");
    bubble.className = role === "user"
      ? "chat-bubble chat-bubble--user"
      : role === "error" ? "chat-bubble chat-bubble--ai chat-bubble--error" : "chat-bubble chat-bubble--ai";

    const content = document.createElement("div");
    content.innerHTML = this.renderMarkdown(text);
    bubble.appendChild(content);

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
      copyBtn.innerHTML = `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>`;
      copyBtn.addEventListener("click", (e) => { e.stopPropagation(); this.copyImage(img, copyBtn); });

      const dlBtn = document.createElement("button");
      dlBtn.title = "Download";
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
    this.messagesEl.querySelector(".chat-empty-hint")?.remove();
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

  private replaceTypingBubbleWithError(message: string, withRetry = false): void {
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
      retry.textContent = "Retry";
      retry.addEventListener("click", () => { row.remove(); this.handleSend(this.lastSentText, this.lastSentAttachments); });
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

  private storageKey(): string {
    return `aerini_chat_${this.wfManager.currentId}`;
  }

  private loadStoreForCurrentWorkflow(): void {
    try {
      const raw = localStorage.getItem(this.storageKey());
      if (raw) {
        const parsed = JSON.parse(raw) as ChatStore;
        if (parsed && Array.isArray(parsed.sessions)) { this.store = parsed; return; }
      }
    } catch { /* corrupt or unavailable storage — fall through to a fresh store */ }
    this.store = { sessions: [], activeId: "" };
  }

  private persist(): void {
    try { localStorage.setItem(this.storageKey(), JSON.stringify(this.store)); }
    catch { /* storage full or unavailable — chat still works for this session, just not saved */ }
  }

  private renderSessionLabel(): void {
    this.sessionLabel.textContent = this.activeSession().name;
  }

  private renderSessionMenu(): void {
    this.sessionMenu.innerHTML = "";
    for (const s of this.store.sessions) {
      const item = document.createElement("button");
      item.className = "toolbar-dropdown-item chat-session-item";
      item.innerHTML = `<span class="chat-session-item-name">${escapeHtml(s.name)}</span>`;
      const del = document.createElement("span");
      del.className = "chat-session-item-del";
      del.innerHTML = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
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

  private deleteSession(id: string): void {
    if (this.store.sessions.length <= 1) { this.toast("Can't delete the only session.", "info"); return; }
    this.store.sessions = this.store.sessions.filter(s => s.id !== id);
    if (this.store.activeId === id) this.store.activeId = this.store.sessions[0].id;
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
   * local session in its place. Note: this only covers the header "Clear"
   * button — deleting an individual session from the session menu
   * (deleteSession()) does not call clearChatSession, so its AI Memory rows
   * are left orphaned under that session_id. Out of this patch's listed
   * scope (the plan names "Clear" specifically); flagging per Rule 6.
   */
  private async handleClear(): Promise<void> {
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
    this.persist();
    this.renderSessionLabel();
    this.renderMessages();
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
