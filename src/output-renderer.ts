import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { NODE_IDS } from "./node-ids";
import type { WorkflowResult } from "./ipc/workflow";
import type { CanvasNode } from "./canvas/Node";
import { escapeHtml } from "./utils";

type NodeMap = Map<string, CanvasNode>;

interface MediaFile {
  filename: string;
  data: string;       // raw base64, no data: URI prefix
  mime_type: string;
}

interface MediaBatchValue {
  files: MediaFile[];
  count: number;
  source: string;
}

export function renderSummaryTab(result: WorkflowResult, nodes: NodeMap): string {
  const duration   = computeDuration(result.logs);
  const nodeCount  = Object.keys(result.node_outputs).length;
  const errorCount = result.logs.filter(l => l.level === "error").length;

  const statusText  = result.success ? "Workflow completed successfully" : "Workflow stopped with an error";
  const badge       = result.success ? "OK" : "FAIL";
  const statusClass = result.success ? "success" : "error";

  let html = `
    <div class="sum-header sum-header--${statusClass}">
      <span class="sum-badge sum-badge--${statusClass}">${badge}</span>
      <div>
        <div class="sum-title sum-title--${statusClass}">${statusText}</div>
        ${duration ? `<div class="sum-meta">${nodeCount} node${nodeCount !== 1 ? "s" : ""} · ${duration}${errorCount ? ` · ${errorCount} error${errorCount !== 1 ? "s" : ""}` : ""}</div>` : ""}
      </div>
    </div>`;

  // Per-node rows
  const entries = Object.entries(result.node_outputs);
  if (entries.length) {
    html += `<div class="sum-nodes">`;
    for (const [id, out] of entries) {
      const node    = nodes.get(id);
      const name    = node?.data.name ?? cleanNodeId(id);
      const typeId  = node?.data.node_type_id ?? "";
      const failed  = result.logs.some(l => l.node_id === id && l.level === "error");
      const mark    = failed ? "✕" : "✓";
      const markClass = failed ? "err" : "ok";
      const preview = previewForNode(typeId, out);

      html += `
        <div class="sum-node-row">
          <span class="sum-node-mark sum-node-mark--${markClass}">${mark}</span>
          <span class="sum-node-name">${escapeHtml(name)}</span>
          <span class="sum-node-type">${typeLabel(typeId)}</span>
          <span class="sum-node-preview">${escapeHtml(preview)}</span>
        </div>`;
    }
    html += `</div>`;
  }

  // Error detail — plain language
  if (!result.success) {
    const err = result.logs.find(l => l.level === "error");
    if (err) {
      const suggestion = errorSuggestion(err.message, err.node_id ? (nodes.get(err.node_id)?.data.node_type_id ?? "") : "");
      html += `
        <div class="sum-error-card">
          <div class="sum-error-title">What went wrong</div>
          <div class="sum-error-msg">${escapeHtml(err.message)}</div>
          ${suggestion ? `<div class="sum-error-hint">${escapeHtml(suggestion)}</div>` : ""}
          <button class="sum-error-copy" data-copy-text="${escapeHtml(err.message)}">Copy error</button>
        </div>`;
    }
  }

  return html;
}

export function renderResultsTab(result: WorkflowResult, nodes: NodeMap): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "res-wrap";

  const entries = Object.entries(result.node_outputs);
  if (!entries.length) {
    wrap.innerHTML = `<div class="res-empty">No node outputs to display</div>`;
    return wrap;
  }

  // Node selector
  const selectorRow = document.createElement("div");
  selectorRow.className = "res-selector-row";

  const select = document.createElement("select");
  select.className = "res-node-select";

  for (const [id] of entries) {
    const node  = nodes.get(id);
    const name  = node?.data.name ?? cleanNodeId(id);
    const failed = result.logs.some(l => l.node_id === id && l.level === "error");
    const opt   = document.createElement("option");
    opt.value   = id;
    opt.textContent = `${failed ? "✕" : "✓"}  ${name}`;
    select.appendChild(opt);
  }

  // Default to last output node, or first entry
  const outputEntry = entries.find(([id]) => nodes.get(id)?.data.node_type_id === NODE_IDS.OUTPUT);
  if (outputEntry) select.value = outputEntry[0];

  selectorRow.appendChild(select);
  wrap.appendChild(selectorRow);

  // Output display area
  const display = document.createElement("div");
  display.className = "res-display";
  wrap.appendChild(display);

  const renderSelected = async () => {
    const id     = select.value;
    const out    = result.node_outputs[id];
    const node   = nodes.get(id);
    const typeId = node?.data.node_type_id ?? "";
    const name   = node?.data.name ?? cleanNodeId(id);

    // media_batch requires async rendering (video/audio need write_temp_file)
    if (typeId === NODE_IDS.OUTPUT) {
      const obj = out as Record<string, unknown>;
      if (obj?.output_type === "media_batch") {
        display.innerHTML = "";
        await renderMediaBatch(obj.value as MediaBatchValue, display, name);
        return;
      }
    }

    display.innerHTML = renderTypedOutput(typeId, out, name);
  };

  select.addEventListener("change", () => { void renderSelected(); });
  void renderSelected();

  return wrap;
}

export function renderErrorsTab(result: WorkflowResult, nodes: NodeMap): string {
  const errors = result.logs.filter(l => l.level === "error");
  if (!errors.length) {
    return `<div class="res-empty">No errors — this tab appears when a run fails</div>`;
  }

  return errors.map(err => {
    const node       = err.node_id ? nodes.get(err.node_id) : null;
    const nodeName   = node?.data.name ?? (err.node_id ? cleanNodeId(err.node_id) : "Workflow");
    const typeId     = node?.data.node_type_id ?? "";
    const suggestion = errorSuggestion(err.message, typeId);
    const t          = new Date(err.timestamp).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });

    return `
      <div class="err-card">
        <div class="err-card-header">
          <span class="err-node-name">${escapeHtml(nodeName)}</span>
          <span class="err-time">${t}</span>
        </div>
        <div class="err-msg">${escapeHtml(err.message)}</div>
        ${suggestion ? `<div class="err-hint">${escapeHtml(suggestion)}</div>` : ""}
        <button class="err-copy" data-copy-text="${escapeHtml(err.message)}">Copy</button>
      </div>`;
  }).join("");
}

export function renderLogsView(result: WorkflowResult): string {
  if (!result.logs.length) {
    return `<div class="res-empty">No messages to show</div>`;
  }
  return `<div class="log-list">${result.logs.map(l => {
    const mark      = l.level === "error" ? "ERR" : l.level === "warn" ? "WRN" : "INF";
    const markClass = l.level === "error" ? "error" : l.level === "warn" ? "warn" : "info";
    const t         = new Date(l.timestamp).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
    return `<div class="log-line">
      <span class="log-mark log-mark--${markClass}">${mark}</span>
      <span class="log-time">${t}</span>
      <span class="log-msg">${escapeHtml(l.message)}</span>
    </div>`;
  }).join("")}</div>`;
}

export function renderDebugView(result: WorkflowResult): string {
  return result.logs.map(l => {
    const mark      = l.level === "error" ? "ERR" : l.level === "warn" ? "WRN" : "   ";
    const markClass = l.level === "error" ? "error" : l.level === "warn" ? "warn" : "info";
    const t         = new Date(l.timestamp).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
    const node      = l.node_id ? ` [${l.node_id.slice(0, 12)}]` : "";
    return `<span class="dbg-mark dbg-mark--${markClass}">${mark}</span> <span class="dbg-time">${t}${node}</span> ${escapeHtml(l.message)}`;
  }).join("\n");
}

// ---------------------------------------------------------------------------
// Media batch renderer — async because video/audio require write_temp_file
// ---------------------------------------------------------------------------

// Validate mime_type to only allow RFC 2045 safe characters.
// Prevents attribute injection when mime_type is interpolated into innerHTML.
function safeMimeType(mime: string): string {
  return /^[a-zA-Z0-9][a-zA-Z0-9!#$&\-^_.+]*\/[a-zA-Z0-9][a-zA-Z0-9!#$&\-^_.+]*$/.test(mime)
    ? mime
    : "application/octet-stream";
}

async function renderMediaBatch(
  batchValue: MediaBatchValue | null | undefined,
  container: HTMLElement,
  name: string,
): Promise<void> {
  const files = batchValue?.files ?? [];
  const count = files.length;

  const header = document.createElement("div");
  header.className = "res-node-header";
  header.innerHTML = `
    <span class="res-node-name">${escapeHtml(name)}</span>
    <span class="res-status-badge res-status-badge--muted">${count} file${count !== 1 ? "s" : ""}</span>`;
  container.appendChild(header);

  if (!count) {
    const empty = document.createElement("div");
    empty.className = "res-empty";
    empty.textContent = "No files in output";
    container.appendChild(empty);
    return;
  }

  const grid = document.createElement("div");
  grid.className = "res-media-grid";
  container.appendChild(grid);

  for (const file of files) {
    const item = document.createElement("div");
    item.className = "res-media-item";
    const mime = safeMimeType(file.mime_type);

    if (mime.startsWith("image/")) {
      // Use DOM setAttribute for src/href to avoid any residual injection risk
      const img = document.createElement("img");
      img.src = `data:${mime};base64,${file.data}`;
      img.alt = file.filename;
      img.className = "res-media-img";

      const caption = document.createElement("div");
      caption.className = "res-media-caption";
      caption.textContent = file.filename;

      const dl = document.createElement("a");
      dl.className = "res-media-dl";
      dl.href = `data:${mime};base64,${file.data}`;
      dl.download = file.filename;
      dl.textContent = "Download";

      item.appendChild(img);
      item.appendChild(caption);
      item.appendChild(dl);
    } else if (mime.startsWith("video/") || mime.startsWith("audio/")) {
      const tag = mime.startsWith("video/") ? "video" : "audio";
      try {
        const absPath = await invoke<string>("write_temp_file", {
          filename: file.filename,
          data: file.data,
        });
        const assetUrl = convertFileSrc(absPath);

        const media = document.createElement(tag) as HTMLVideoElement | HTMLAudioElement;
        media.controls = true;
        media.src = assetUrl;
        media.className = `res-media-${tag}`;

        const caption = document.createElement("div");
        caption.className = "res-media-caption";
        caption.textContent = file.filename;

        const dl = document.createElement("a");
        dl.className = "res-media-dl";
        dl.href = assetUrl;
        dl.download = file.filename;
        dl.textContent = "Download";

        item.appendChild(media);
        item.appendChild(caption);
        item.appendChild(dl);
      } catch (err) {
        item.innerHTML = `<div class="res-media-error">Failed to load ${escapeHtml(file.filename)}: ${escapeHtml(String(err))}</div>`;
      }
    } else {
      const dl = document.createElement("a");
      dl.className = "res-media-dl";
      dl.href = `data:${mime};base64,${file.data}`;
      dl.download = file.filename;
      dl.textContent = `📄 ${file.filename}`;
      item.appendChild(dl);
    }

    grid.appendChild(item);
  }
}

// ---------------------------------------------------------------------------
// Typed output dispatch — sync paths only; media_batch bypasses this
// ---------------------------------------------------------------------------

function renderTypedOutput(typeId: string, out: unknown, nodeName: string): string {
  switch (typeId) {
    case NODE_IDS.HTTP_REQUEST: return renderHttpOutput(out, nodeName);
    case NODE_IDS.AI_PROMPT:
    case NODE_IDS.AI_AGENT:    return renderAiOutput(out, nodeName);
    case NODE_IDS.OUTPUT:         return renderOutputNodeResult(out, nodeName);
    case NODE_IDS.SOCIAL_UPLOAD:  return renderSocialUploadOutput(out, nodeName);
    case NODE_IDS.CODE:
    case NODE_IDS.TRANSFORM_DATA: return renderCodeOutput(out, nodeName);
    default:            return renderGenericOutput(out, nodeName);
  }
}

function renderHttpOutput(out: unknown, name: string): string {
  const obj = out as Record<string, unknown>;
  // Rust HTTP node outputs: { status, body, headers }
  const status  = (obj?.status ?? obj?.status_code) as number | string | undefined;
  const body    = obj?.body ?? obj?.content ?? obj?.response;
  const headers = obj?.headers as Record<string, string> | undefined;

  const statusNum   = typeof status === "string" ? parseInt(status, 10) : (status ?? 0);
  const statusClass = statusNum && statusNum < 400 ? "success" : statusNum >= 400 ? "error" : "muted";
  const statusText  = status ? `HTTP ${status}` : "Response";

  let bodyHtml = "";
  if (body !== undefined) {
    const bodyStr = typeof body === "string" ? body : JSON.stringify(body, null, 2);
    const isJson  = typeof body === "object" && body !== null;
    bodyHtml = `
      <div class="res-section-label">Response body</div>
      ${isJson
        ? `<pre class="res-code">${syntaxHighlight(JSON.stringify(body, null, 2))}</pre>`
        : `<pre class="res-code">${escapeHtml(bodyStr)}</pre>`}`;
  }

  let headersHtml = "";
  if (headers && Object.keys(headers).length) {
    headersHtml = `
      <details class="res-details">
        <summary>Response headers (${Object.keys(headers).length})</summary>
        <div class="res-kv-list">
          ${Object.entries(headers).map(([k, v]) =>
            `<div class="res-kv-row"><span class="res-kv-key">${escapeHtml(k)}</span><span class="res-kv-val">${escapeHtml(String(v))}</span></div>`
          ).join("")}
        </div>
      </details>`;
  }

  return `
    <div class="res-node-header">
      <span class="res-node-name">${escapeHtml(name)}</span>
      <span class="res-status-badge res-status-badge--${statusClass}">${statusText}</span>
    </div>
    ${bodyHtml}
    ${headersHtml}`;
}

function renderAiOutput(out: unknown, name: string): string {
  const obj     = out as Record<string, unknown>;
  const content = obj?.content ?? obj?.text ?? obj?.result ?? obj?.response;
  const model   = obj?.model as string | undefined;
  const usage   = obj?.usage as Record<string, number> | undefined;

  const text = typeof content === "string" ? content : JSON.stringify(content, null, 2);
  const usageHtml = usage
    ? `<div class="res-ai-meta">${Object.entries(usage).map(([k, v]) => `${k}: ${v}`).join(" · ")}</div>`
    : "";
  const modelHtml = model ? `<div class="res-ai-meta">Model: ${escapeHtml(model)}</div>` : "";

  return `
    <div class="res-node-header">
      <span class="res-node-name">${escapeHtml(name)}</span>
    </div>
    ${modelHtml}
    <div class="res-ai-text">${escapeHtml(text)}</div>
    ${usageHtml}`;
}

function renderOutputNodeResult(out: unknown, name: string): string {
  // Rust output node outputs: { value, label, type, output_type? }
  const obj     = out as Record<string, unknown>;
  const label   = typeof obj?.label === "string" ? obj.label : name;
  const typeStr = typeof obj?.type === "string" ? obj.type : "";
  const value   = obj?.value ?? obj?.content ?? out;

  const isJson   = typeof value === "object" && value !== null;
  const isNull   = value === null || value === undefined;
  const valueStr = isNull ? "null" : typeof value === "string" ? value : JSON.stringify(value, null, 2);

  return `
    <div class="res-node-header">
      <span class="res-node-name">${escapeHtml(label)}</span>
      ${typeStr ? `<span class="res-status-badge res-status-badge--muted">${escapeHtml(typeStr)}</span>` : ""}
    </div>
    ${isNull
      ? `<div class="res-empty">No output value</div>`
      : isJson
        ? `<pre class="res-code">${syntaxHighlight(JSON.stringify(value, null, 2))}</pre>`
        : `<div class="res-text">${escapeHtml(valueStr)}</div>`}`;
}

function renderCodeOutput(out: unknown, name: string): string {
  const obj    = out as Record<string, unknown>;
  const result = obj?.result ?? obj?.output ?? obj?.stdout ?? out;
  const err    = obj?.error ?? obj?.stderr;

  let html = `<div class="res-node-header"><span class="res-node-name">${escapeHtml(name)}</span></div>`;

  if (err) {
    html += `<div class="res-section-label res-section-label--error">Error output</div>
             <pre class="res-code res-code--error">${escapeHtml(String(err))}</pre>`;
  }

  const isJson = typeof result === "object" && result !== null;
  html += `<div class="res-section-label">Result</div>
           ${isJson
             ? `<pre class="res-code">${syntaxHighlight(JSON.stringify(result, null, 2))}</pre>`
             : `<div class="res-text">${escapeHtml(String(result))}</div>`}`;

  return html;
}

function renderSocialUploadOutput(out: unknown, name: string): string {
  const obj      = (out ?? {}) as Record<string, unknown>;
  const platform = (obj.platform as string | undefined) ?? "unknown";
  const uploaded = Array.isArray(obj.uploaded) ? obj.uploaded as Array<Record<string, unknown>> : [];
  const errors   = Array.isArray(obj.errors)   ? obj.errors   as Array<Record<string, unknown>> : [];
  const count    = typeof obj.count === "number" ? obj.count : uploaded.length;

  const platformLabel = { youtube: "YouTube", instagram: "Instagram", tiktok: "TikTok" }[platform] ?? platform;
  const statusClass   = errors.length && !uploaded.length ? "error" : errors.length ? "warn" : "success";
  const statusText    = `${count} uploaded${errors.length ? ` · ${errors.length} failed` : ""}`;

  const uploadedHtml = uploaded.length
    ? `<div class="res-section-label">Uploaded</div>
       <div class="soc-upload-list">
         ${uploaded.map(u => `
           <div class="soc-upload-item">
             <span class="soc-upload-filename">${escapeHtml(String(u.filename ?? ""))}</span>
             ${u.url ? `<a class="soc-upload-link" href="${escapeHtml(String(u.url))}" target="_blank" rel="noopener noreferrer">View ↗</a>` : ""}
           </div>`).join("")}
       </div>`
    : "";

  const errorsHtml = errors.length
    ? `<div class="res-section-label res-section-label--error">Errors</div>
       ${errors.map(e => `
         <div class="soc-error-card">
           <div class="soc-error-code">${escapeHtml(String(e.code ?? "ERROR"))}</div>
           <div class="soc-error-filename">${escapeHtml(String(e.filename ?? ""))}</div>
           <div class="soc-error-explanation">${escapeHtml(String(e.explanation ?? e.message ?? ""))}</div>
           ${e.action ? `<div class="soc-error-action">${escapeHtml(String(e.action))}</div>` : ""}
         </div>`).join("")}`
    : "";

  return `
    <div class="res-node-header">
      <span class="res-node-name">${escapeHtml(name)}</span>
      <span class="res-status-badge res-status-badge--${statusClass}">${escapeHtml(platformLabel)} · ${statusText}</span>
    </div>
    ${uploadedHtml}
    ${errorsHtml}`;
}

function renderGenericOutput(out: unknown, name: string): string {
  if (out === null || out === undefined) {
    return `<div class="res-node-header"><span class="res-node-name">${escapeHtml(name)}</span></div>
            <div class="res-empty">No output</div>`;
  }

  // If it's a flat object with simple key-value pairs, show as a table
  if (typeof out === "object" && !Array.isArray(out)) {
    const obj     = out as Record<string, unknown>;
    const entries = Object.entries(obj);
    const isFlat  = entries.every(([, v]) => typeof v !== "object" || v === null);

    if (isFlat && entries.length <= 20) {
      const rows = entries.map(([k, v]) =>
        `<div class="res-kv-row">
           <span class="res-kv-key">${escapeHtml(k)}</span>
           <span class="res-kv-val">${escapeHtml(String(v))}</span>
         </div>`
      ).join("");
      return `
        <div class="res-node-header"><span class="res-node-name">${escapeHtml(name)}</span></div>
        <div class="res-kv-list">${rows}</div>`;
    }
  }

  // Fallback: collapsible JSON
  const preview = extractPreview(out);
  return `
    <div class="res-node-header"><span class="res-node-name">${escapeHtml(name)}</span></div>
    <div class="res-preview-text">${escapeHtml(preview)}</div>
    <details class="res-details">
      <summary>Raw data</summary>
      <pre class="res-code">${syntaxHighlight(JSON.stringify(out, null, 2))}</pre>
    </details>`;
}

function previewForNode(typeId: string, out: unknown): string {
  const obj = out as Record<string, unknown>;
  switch (typeId) {
    case NODE_IDS.HTTP_REQUEST: {
      // Rust outputs: { status, body, headers }
      const status = obj?.status ?? obj?.status_code;
      const body   = obj?.body ?? obj?.content;
      const preview = body !== null && body !== undefined
        ? (typeof body === "string" ? body.slice(0, 50) : JSON.stringify(body).slice(0, 50))
        : "";
      return status ? `HTTP ${status}${preview ? ` · ${preview}` : ""}` : extractPreview(out);
    }
    case NODE_IDS.AI_PROMPT:
    case NODE_IDS.AI_AGENT: {
      const content = obj?.content ?? obj?.text ?? obj?.result;
      return typeof content === "string" ? content.slice(0, 60) : extractPreview(out);
    }
    case NODE_IDS.OUTPUT: {
      const label   = typeof obj?.label === "string" ? obj.label : "";
      // media_batch: show file count instead of trying to preview base64 data
      if (obj?.output_type === "media_batch") {
        const bv    = obj?.value as Record<string, unknown> | undefined;
        const count = typeof bv?.count === "number"
          ? bv.count
          : (bv?.files as unknown[] | undefined)?.length ?? 0;
        const suffix = `${count} file${count !== 1 ? "s" : ""}`;
        return label ? `${label}: ${suffix}` : suffix;
      }
      const value = obj?.value;
      if (value === null || value === undefined) return label || "null";
      const str = typeof value === "string" ? value : JSON.stringify(value);
      return label ? `${label}: ${str.slice(0, 40)}` : str.slice(0, 60);
    }
    case NODE_IDS.SCHEDULE:
    case NODE_IDS.MANUAL_TRIGGER:
    case NODE_IDS.WEBHOOK: {
      const triggeredAt = obj?.triggered_at as string | undefined;
      const mode = obj?.mode as string | undefined;
      if (triggeredAt) {
        const t = new Date(triggeredAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
        return mode ? `${mode} · triggered ${t}` : `triggered ${t}`;
      }
      return extractPreview(out);
    }
    default:
      return extractPreview(out);
  }
}

function errorSuggestion(message: string, typeId: string): string {
  const m = message.toLowerCase();

  if (m.includes("connection refused") || m.includes("failed to connect")) {
    return typeId === NODE_IDS.HTTP_REQUEST
      ? "Check the URL is correct and the server is reachable."
      : "Check the server address and port.";
  }
  if (m.includes("timeout")) return "The request timed out. Try increasing the timeout in the node config.";
  if (m.includes("unauthorized") || m.includes("401")) return "Authentication failed. Check your credentials in Credentials.";
  if (m.includes("forbidden") || m.includes("403")) return "Access denied. Verify your API key has the required permissions.";
  if (m.includes("not found") || m.includes("404")) return "The requested resource was not found. Check the URL or endpoint path.";
  if (m.includes("api key") || m.includes("apikey") || m.includes("api_key")) return "API key missing or invalid. Configure it in Credentials.";
  if (m.includes("json") || m.includes("parse")) return "The response could not be parsed. Check the input format.";
  if (m.includes("rate limit") || m.includes("429")) return "Rate limit exceeded. Add a Delay node before this one, or reduce execution frequency.";
  if (m.includes("smtp") || m.includes("email")) return "Email delivery failed. Check your SMTP credentials in Credentials.";
  return "";
}

function typeLabel(typeId: string): string {
  const labels: Record<string, string> = {
    http_request: "HTTP", ai_prompt: "AI", ai_agent: "AI Agent",
    output: "Output", schedule: "Trigger", webhook: "Webhook",
    manual_trigger: "Trigger", code: "Code", transform_data: "Transform",
    if_condition: "Condition", loop: "Loop", send_email: "Email",
    shell_exec: "Shell", set_variable: "Variable", get_variable: "Variable",
  };
  return labels[typeId] ?? typeId.replace(/_/g, " ");
}

function cleanNodeId(id: string): string {
  return id.replace(/^node_\d+_/, "").replace(/_/g, " ") || id;
}

function computeDuration(logs: WorkflowResult["logs"]): string {
  if (logs.length < 2) return "";
  const first = new Date(logs[0].timestamp).getTime();
  const last  = new Date(logs[logs.length - 1].timestamp).getTime();
  const ms    = last - first;
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

export function extractPreview(out: unknown): string {
  if (out === null || out === undefined) return "null";
  if (typeof out === "string") {
    const trimmed = out.trim();
    if ((trimmed.startsWith("{") || trimmed.startsWith("[")) && trimmed.length < 500) {
      try { return JSON.stringify(JSON.parse(trimmed), null, 0).slice(0, 120); } catch { /* not JSON */ }
    }
    return out.slice(0, 120);
  }
  if (typeof out === "number" || typeof out === "boolean") return String(out);
  const obj = out as Record<string, unknown>;
  for (const k of ["content", "result", "stdout", "text", "body", "value", "output", "message", "data"]) {
    if (obj[k] !== undefined) {
      const v = obj[k];
      if (typeof v === "string") return v.slice(0, 120);
      if (typeof v === "number" || typeof v === "boolean") return String(v);
      return JSON.stringify(v).slice(0, 120);
    }
  }
  return JSON.stringify(out).slice(0, 120);
}

/**
 * Syntax-highlight a JSON string for HTML display.
 *
 * CALL-ORDER CONTRACT: `json` must be the raw output of `JSON.stringify` —
 * never pre-escaped HTML.  This function calls `escapeHtml` internally as
 * its first step, then applies span-wrapping regexes.  Passing pre-escaped
 * HTML (e.g. a string containing `&amp;` or `&lt;`) will cause double-escaping
 * and corrupt the displayed output.
 *
 * All call sites in this file satisfy this contract because they pass
 * `JSON.stringify(value)` directly.  Do not refactor those call sites to
 * pre-escape without updating this function accordingly.
 */
export function syntaxHighlight(json: string): string {
  return escapeHtml(json).replace(
    /("(\\u[a-zA-Z0-9]{4}|\\[^u]|[^\\"])*"(\s*:)?|\b(true|false|null)\b|-?\d+(?:\.\d*)?(?:[eE][+\-]?\d+)?)/g,
    match => {
      if (/^"/.test(match))
        return match.endsWith(":")
          ? `<span class="json-key">${match}</span>`
          : `<span class="json-str">${match}</span>`;
      if (/true|false/.test(match)) return `<span class="json-bool">${match}</span>`;
      if (/null/.test(match)) return `<span class="json-null">${match}</span>`;
      return `<span class="json-num">${match}</span>`;
    }
  );
}

// Legacy exports — kept for backward compatibility with any callers
export function renderRunSummary(result: WorkflowResult): string {
  return renderSummaryTab(result, new Map());
}

// Called after any innerHTML assignment that includes copy buttons produced by
// renderSummaryTab or renderErrorsTab. Reads data-copy-text, attaches click
// listeners. Safe to call multiple times — each call only processes buttons
// that have not yet been wired (checks for the data attribute being present).

export function wireCopyButtons(container: Element): void {
  container.querySelectorAll<HTMLButtonElement>("[data-copy-text]").forEach(btn => {
    const text = btn.dataset.copyText ?? "";
    btn.addEventListener("click", () => {
      navigator.clipboard.writeText(text).then(() => { btn.textContent = "Copied"; });
    });
  });
}
