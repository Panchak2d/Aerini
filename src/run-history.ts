import { invoke } from "@tauri-apps/api/core";
import type { WorkflowResult } from "./ipc/workflow";
import { escapeHtml } from "./utils";
import { showConfirm } from "./confirm";

export interface HistoryPanel extends HTMLElement {
  _refresh(): void;
}

/** Returns a human-readable relative time string for recent dates.
 *  Falls back to absolute date for anything older than 24 hours.  */
function relativeTime(date: Date): string {
  const diffMs   = Date.now() - date.getTime();
  const diffSecs = Math.floor(diffMs / 1000);
  const diffMins = Math.floor(diffSecs / 60);
  const diffHrs  = Math.floor(diffMins / 60);

  if (diffSecs < 60)   return "just now";
  if (diffMins < 60)   return `${diffMins}m ago`;
  if (diffHrs  < 24)   return `${diffHrs}h ago`;
  if (diffHrs  < 48)   return "yesterday";
  return date.toLocaleDateString([], { month: "short", day: "numeric" });
}

const PAGE_SIZE = 20;

export interface RunRecord {
  id:            string;
  workflow_id:   string;
  workflow_name: string;
  ran_at:        string;
  success:       boolean;
  duration_ms:   number;
  result_json:   string;
  status:        string;
}

export async function saveRunStarted(
  id:           string,
  workflowId:   string,
  workflowName: string,
): Promise<void> {
  try {
    await invoke("save_run_started", { id, workflowId, workflowName, ranAt: new Date().toISOString() });
  } catch (e) { console.error("Failed to save run-started record:", e); }
}

export async function saveRunToHistory(
  id:           string,
  workflowId:   string,
  workflowName: string,
  result:       WorkflowResult,
): Promise<void> {
  await migrateFromLocalStorage(workflowId, workflowName);
  const durationMs = computeDurationMs(result.logs);
  const record: RunRecord = {
    id,
    workflow_id:   workflowId,
    workflow_name: workflowName,
    ran_at:        new Date().toISOString(),
    success:       result.success,
    duration_ms:   durationMs,
    result_json:   JSON.stringify(result),
    status:        result.success ? "success" : "failed",
  };
  try { await invoke("save_run_record", { record }); }
  catch (e) { console.error("Failed to save run record:", e); }
}

export async function deleteRunRecord(id: string): Promise<void> {
  await invoke("delete_run_record", { id });
}

export async function clearRunRecords(workflowId: string): Promise<void> {
  await invoke("clear_run_records", { workflowId: workflowId });
}

async function loadPage(workflowId: string, offset: number, filter: string): Promise<RunRecord[]> {
  return invoke<RunRecord[]>("list_run_records", { workflowId, offset, limit: PAGE_SIZE, filter });
}

let _migrationDone = false;

async function migrateFromLocalStorage(workflowId: string, _workflowName: string): Promise<void> {
  if (_migrationDone) return;
  _migrationDone = true;
  const LS_KEY = "aerini_run_history_v1";
  const raw = localStorage.getItem(LS_KEY);
  if (!raw) return;
  try {
    const old = JSON.parse(raw) as Array<{
      id: string; workflowName: string; ranAt: string;
      success: boolean; durationMs: number; result: WorkflowResult;
    }>;
    for (const o of old) {
      const record: RunRecord = {
        id: o.id, workflow_id: workflowId, workflow_name: o.workflowName,
        ran_at: o.ranAt, success: o.success, duration_ms: o.durationMs,
        result_json: JSON.stringify(o.result),
        status: o.success ? "success" : "failed",
      };
      try {
        await invoke("save_run_record", { record });
      } catch (e) {
        console.warn("Failed to save run record:", e);
      }
    }
  } catch { /* ignore */ }
  localStorage.removeItem(LS_KEY);
}

export function renderHistoryPanel(
  workflowId:   string,
  onRestoreRun: (result: WorkflowResult) => void,
): HistoryPanel {
  const wrap = document.createElement("div") as unknown as HistoryPanel;
  wrap.className = "history-panel";

  let currentOffset = 0;
  let currentFilter = "all";
  let isLoading     = false;

  // Toolbar
  const toolbar = document.createElement("div");
  toolbar.className = "history-toolbar";

  const filterWrap = document.createElement("div");
  filterWrap.className = "history-filter-wrap";

  [{ value: "all", label: "All" }, { value: "success", label: "Success" }, { value: "failed", label: "Failed" }]
    .forEach(f => {
      const btn = document.createElement("button");
      btn.className = "history-filter-btn" + (f.value === "all" ? " active" : "");
      btn.textContent = f.label;
      btn.dataset.filter = f.value;
      btn.addEventListener("click", () => {
        if (currentFilter === f.value) return;
        currentFilter = f.value;
        filterWrap.querySelectorAll<HTMLElement>(".history-filter-btn")
          .forEach(b => b.classList.toggle("active", b.dataset.filter === f.value));
        currentOffset = 0;
        loadAndRender();
      });
      filterWrap.appendChild(btn);
    });

  const clearBtn = document.createElement("button");
  clearBtn.className = "history-clear-btn";
  clearBtn.textContent = "Clear all";
  clearBtn.addEventListener("click", async () => {
    const ok = await showConfirm("Clear all run history for this workflow? This cannot be undone.", true, "Clear");
    if (!ok) return;
    clearBtn.textContent = "Clearing…";
    clearBtn.disabled = true;
    await clearRunRecords(workflowId).catch(() => {});
    currentOffset = 0;
    await loadAndRender();
    clearBtn.textContent = "Clear all";
    clearBtn.disabled = false;
  });

  toolbar.appendChild(filterWrap);
  toolbar.appendChild(clearBtn);
  wrap.appendChild(toolbar);

  const content = document.createElement("div");
  content.className = "history-content";
  wrap.appendChild(content);

  // Pagination
  const pagination = document.createElement("div");
  pagination.className = "history-pagination";
  const prevBtn  = document.createElement("button");
  prevBtn.className = "history-page-btn"; prevBtn.textContent = "← Prev";
  const pageLabel = document.createElement("span");
  pageLabel.className = "history-page-label";
  const nextBtn  = document.createElement("button");
  nextBtn.className = "history-page-btn"; nextBtn.textContent = "Next →";
  prevBtn.addEventListener("click", () => { if (currentOffset < PAGE_SIZE) return; currentOffset -= PAGE_SIZE; loadAndRender(); });
  nextBtn.addEventListener("click", () => { currentOffset += PAGE_SIZE; loadAndRender(); });
  pagination.appendChild(prevBtn); pagination.appendChild(pageLabel); pagination.appendChild(nextBtn);
  wrap.appendChild(pagination);

  async function loadAndRender() {
    if (isLoading) return;
    isLoading = true;
    content.innerHTML = `<div class="history-loading">Loading…</div>`;
    try {
      const records = await loadPage(workflowId, currentOffset, currentFilter);
      content.innerHTML = "";
      if (!records.length && currentOffset === 0) {
        content.innerHTML = `<div class="history-empty">No runs yet${currentFilter !== "all" ? ` matching "${escapeHtml(currentFilter)}"` : ""}.</div>`;
        pagination.style.display = "none";
        return;
      }
      if (!records.length) {
        currentOffset = Math.max(0, currentOffset - PAGE_SIZE);
        isLoading = false; loadAndRender(); return;
      }
      for (const record of records) content.appendChild(buildItem(record, onRestoreRun));
      pageLabel.textContent = `Page ${Math.floor(currentOffset / PAGE_SIZE) + 1}`;
      prevBtn.disabled = currentOffset === 0;
      nextBtn.disabled = records.length < PAGE_SIZE;
      pagination.style.display = "flex";
    } catch (e) {
      content.innerHTML = `<div class="history-empty">Failed to load: ${escapeHtml(String(e))}</div>`;
    } finally { isLoading = false; }
  }

  loadAndRender();
  wrap._refresh = () => { currentOffset = 0; loadAndRender(); };
  return wrap;
}

function buildItem(record: RunRecord, onRestoreRun: (r: WorkflowResult) => void): HTMLElement {
  const item = document.createElement("div");
  const isInterrupted = record.status === "running" || record.status === "interrupted";
  item.className = `history-item ${isInterrupted ? "history-fail" : record.success ? "history-ok" : "history-fail"}`;
  item.tabIndex = 0;
  item.setAttribute("role", "button");
  const ranAt   = new Date(record.ran_at);
  const timeStr = ranAt.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const dateStr = ranAt.toLocaleDateString([], { month: "short", day: "numeric" });
  const dur     = record.duration_ms < 1000 ? `${record.duration_ms}ms` : `${(record.duration_ms / 1000).toFixed(1)}s`;
  const badgeText = isInterrupted ? "INTERRUPTED" : record.success ? "OK" : "FAIL";
  item.innerHTML = `
    <div class="history-item-row">
      <span class="history-badge ${record.success && !isInterrupted ? "history-badge-ok" : "history-badge-fail"}">${badgeText}</span>
      <span class="history-name">${escapeHtml(record.workflow_name)}</span>
      <span class="history-dur">${isInterrupted ? "" : dur}</span>
      <button class="history-del-btn" title="Delete this run">✕</button>
    </div>
    <div class="history-meta"><span title="${dateStr} · ${timeStr}" aria-label="${dateStr} at ${timeStr}">${relativeTime(ranAt)}</span></div>`;
  item.addEventListener("click", (e) => {
    if ((e.target as HTMLElement).classList.contains("history-del-btn")) return;
    if (isInterrupted) return;
    try { onRestoreRun(JSON.parse(record.result_json) as WorkflowResult); } catch { /* */ }
  });
  item.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    if ((e.target as HTMLElement).classList.contains("history-del-btn")) return;
    e.preventDefault();
    item.click();
  });
  item.querySelector<HTMLButtonElement>(".history-del-btn")!.addEventListener("click", async (e) => {
    e.stopPropagation();
    await deleteRunRecord(record.id).catch(() => {});
    item.remove();
  });
  return item;
}

function computeDurationMs(logs: WorkflowResult["logs"]): number {
  if (logs.length < 2) return 0;
  return new Date(logs[logs.length - 1].timestamp).getTime() - new Date(logs[0].timestamp).getTime();
}

