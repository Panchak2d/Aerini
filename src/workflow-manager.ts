import {
  listWorkflows, saveWorkflow, loadWorkflow, deleteWorkflow,
  saveVersion, listVersions, getVersion, deleteVersion as ipcDeleteVersion,
  type WorkflowSummary, type VersionRow,
} from "./ipc/workflow";
import { invoke } from "@tauri-apps/api/core";
import { serialize, deserialize, type ChatSettings, DEFAULT_CHAT_SETTINGS } from "./canvas/CanvasSerializer";
import type { Canvas } from "./canvas/Canvas";
import type { CanvasNode } from "./canvas/Node";
import type { Connector } from "./canvas/Connector";
import { isTauri } from "./utils";

const LS_KEY = "aerini_workflows_v1";

function lsSave(id: string, name: string, json: string, tags: string[]): void {
  try {
    const all = JSON.parse(localStorage.getItem(LS_KEY) ?? "{}");
    all[id] = { id, name, json, updated_at: new Date().toISOString(), tags };
    localStorage.setItem(LS_KEY, JSON.stringify(all));
  } catch (e) {
    console.error("Aerini: localStorage save failed", e);
  }
}
function lsList(): WorkflowSummary[] {
  try { return Object.values(JSON.parse(localStorage.getItem(LS_KEY) ?? "{}")) as WorkflowSummary[]; }
  catch { return []; }
}
function lsLoad(id: string): string | null {
  try { return (JSON.parse(localStorage.getItem(LS_KEY) ?? "{}")[id] as { json: string })?.json ?? null; }
  catch { return null; }
}
function lsDelete(id: string): void {
  try { const all = JSON.parse(localStorage.getItem(LS_KEY) ?? "{}"); delete all[id]; localStorage.setItem(LS_KEY, JSON.stringify(all)); }
  catch {}
}

// Tracks which workflows are currently running so the sidebar shows a live dot.

const _runningWorkflows = new Map<string, boolean>();

export function setWorkflowRunning(id: string, running: boolean): void {
  if (running) _runningWorkflows.set(id, true);
  else _runningWorkflows.delete(id);
  // Update the dot on the sidebar item without a full list refresh
  const item = document.querySelector<HTMLElement>(`[data-wf-id="${id}"]`);
  if (item) {
    const existing = item.querySelector(".workflow-item-run-state");
    if (existing) existing.remove();
    const wrap = document.createElement("span");
    wrap.className = "workflow-item-run-state";
    if (_runningWorkflows.has(id)) {
      item.classList.add("wf-is-running");
      const dot = document.createElement("span");
      dot.className = "wf-run-dot"; dot.title = "Running";
      wrap.appendChild(dot);
    } else {
      item.classList.remove("wf-is-running");
    }
    item.insertBefore(wrap, item.firstChild);
  }
}

export function isWorkflowRunning(id: string): boolean {
  return _runningWorkflows.has(id);
}

export class WorkflowManager {
  canvas: Canvas;
  currentId   = `wf_${crypto.randomUUID()}`;
  currentName = "Untitled";
  currentTags: string[] = [];
  /** Per-workflow parallel execution setting. Serialised into workflow JSON. */
  parallelExecution   = false;
  maxConcurrentNodes  = 8;
  /** Per-workflow Chat Panel feature toggles. Serialised into workflow JSON
   *  under "settings.chat" — read by ChatPanel.applyToggles() on panel open. */
  chatSettings: ChatSettings = { ...DEFAULT_CHAT_SETTINGS };
  hasUnsaved  = false;
  private sortMode = "updated_desc";

  private autoSaveTimer: ReturnType<typeof setTimeout> | null = null;
  private onUnsavedChange: (u: boolean) => void;
  private onTitleChange:   (n: string)  => void;
  private onStatusChange:  (m: string)  => void;
  private onToast:         (m: string, t: "success" | "error" | "info") => void;
  // Async confirm function — replaces window.confirm which is suppressed in Tauri
  private confirmFn:    (msg: string, isDanger?: boolean) => Promise<boolean>;
  private onPanelClose: (() => void) | null = null;
  onNavigate: (() => void) | null = null;

  constructor(
    canvas: Canvas,
    callbacks: {
      onUnsaved: (u: boolean) => void;
      onTitle:   (n: string)  => void;
      onStatus:  (m: string)  => void;
      onToast:   (m: string, t: "success" | "error" | "info") => void;
      confirm:   (msg: string, isDanger?: boolean) => Promise<boolean>;
      onPanelClose?: () => void;
    }
  ) {
    this.canvas          = canvas;
    this.onUnsavedChange = callbacks.onUnsaved;
    this.onTitleChange   = callbacks.onTitle;
    this.onStatusChange  = callbacks.onStatus;
    this.onToast         = callbacks.onToast;
    this.confirmFn       = callbacks.confirm;
    this.onPanelClose    = callbacks.onPanelClose ?? null;
  }

  markUnsaved(on: boolean): void {
    this.hasUnsaved = on;
    this.onUnsavedChange(on);
  }

  scheduleAutoSave(): void {
    if (this.autoSaveTimer) clearTimeout(this.autoSaveTimer);
    this.autoSaveTimer = setTimeout(async () => {
      if (!this.hasUnsaved) return;
      const json = serialize(this.currentId, this.currentName, this.canvas.nodes, this.canvas.connectors, this.parallelExecution, this.maxConcurrentNodes, this.chatSettings, this.currentTags);
      if (isTauri()) {
        try { await saveWorkflow(json); } catch (e) { console.error("Aerini: autosave failed", e); }
      } else {
        lsSave(`autosave_${this.currentId}`, `[autosave] ${this.currentName}`, json, this.currentTags);
      }
    }, 30_000);
  }

  setSortMode(mode: string): void {
    this.sortMode = mode;
    this.refreshWorkflowList();
  }

  async refreshWorkflowList(): Promise<void> {
    const list = document.getElementById("workflow-list")!;
    list.innerHTML = "";
    let wfs = isTauri() ? await listWorkflows().catch(() => []) : lsList();

    // Running workflows appear only in Background Runs — exclude from this list
    wfs = wfs.filter(wf => !isWorkflowRunning(wf.id));

    wfs.sort((a, b) => {
      switch (this.sortMode) {
        case "name_asc":      return a.name.localeCompare(b.name);
        case "name_desc":     return b.name.localeCompare(a.name);
        case "updated_asc":   return new Date(a.updated_at).getTime() - new Date(b.updated_at).getTime();
        default:              return new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime();
      }
    });

    // Update activity bar badge
    const wfBadge = document.querySelector<HTMLElement>('.activity-btn[data-zone="workflows"] .activity-badge');
    if (wfBadge) {
      wfBadge.textContent = wfs.length > 0 ? String(wfs.length) : "";
      wfBadge.classList.toggle("activity-badge--hidden", wfs.length === 0);
    }

    if (!wfs.length) {
      const empty = document.createElement("div");
      empty.className = "workflow-list-empty";
      empty.textContent = "No saved workflows";
      list.appendChild(empty);
      return;
    }

    for (const wf of wfs) {
      const item = document.createElement("div");
      item.className = "workflow-item";
      item.dataset.wfId = wf.id;
      if (wf.id === this.currentId) item.classList.add("active");

      // Run state indicator dot
      const runWrap = document.createElement("span");
      runWrap.className = "workflow-item-run-state";
      if (_runningWorkflows.has(wf.id)) {
        item.classList.add("wf-is-running");
        const dot = document.createElement("span");
        dot.className = "wf-run-dot"; dot.title = "Running";
        runWrap.appendChild(dot);
      }
      item.appendChild(runWrap);

      const nameEl = document.createElement("span");
      nameEl.className = "workflow-item-name";
      nameEl.textContent = wf.name;

      const tags = wf.tags ?? [];
      item.dataset.wfTags = tags.join(" ").toLowerCase();

      const updatedAt = new Date(wf.updated_at);
      const savedLabel = !isNaN(updatedAt.getTime())
        ? `Last saved: ${updatedAt.toLocaleDateString([], { month:"short", day:"numeric", hour:"2-digit", minute:"2-digit" })}`
        : "";
      item.title = tags.length ? `${savedLabel}${savedLabel ? " • " : ""}Tags: ${tags.join(", ")}` : savedLabel;

      let tagsEl: HTMLElement | null = null;
      if (tags.length) {
        tagsEl = document.createElement("span");
        tagsEl.className = "workflow-item-tags";
        // Cap visible chips so a long tag list can't push the name out or
        // overflow the row — the full list is still in item.title above.
        const shown = tags.slice(0, 2);
        for (const t of shown) {
          const chip = document.createElement("span");
          chip.className = "workflow-item-tag";
          chip.textContent = t;
          tagsEl.appendChild(chip);
        }
        if (tags.length > shown.length) {
          const more = document.createElement("span");
          more.className = "workflow-item-tag workflow-item-tag-more";
          more.textContent = `+${tags.length - shown.length}`;
          tagsEl.appendChild(more);
        }
      }

      const delBtn = document.createElement("button");
      delBtn.className = "workflow-item-del";
      delBtn.textContent = "✕";
      delBtn.title = "Delete workflow";
      delBtn.addEventListener("click", async e => {
        e.stopPropagation();
        // Block deletion while the workflow is actively running in the background
        if (isWorkflowRunning(wf.id)) {
          this.onToast(
            `"${wf.name}" is currently running in the background. Stop it before deleting.`,
            "error"
          );
          return;
        }
        const ok = await this.confirmFn(`Delete "${wf.name}"? This cannot be undone.`, true);
        if (!ok) return;
        if (isTauri()) {
          try {
            await deleteWorkflow(wf.id);
          } catch (err) {
            this.onToast(`Delete failed: ${err}`, "error");
            return;
          }
        } else {
          lsDelete(wf.id);
        }
        if (wf.id === this.currentId) this.handleNew();
        else await this.refreshWorkflowList();
      });

      item.appendChild(nameEl);
      if (tagsEl) item.appendChild(tagsEl);

      const dupBtn = document.createElement("button");
      dupBtn.className = "workflow-item-dup";
      dupBtn.title = "Duplicate workflow";
      dupBtn.innerHTML = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>`;
      dupBtn.addEventListener("click", async e => {
        e.stopPropagation();
        await this.duplicateWorkflow(wf.id, wf.name);
      });

      item.appendChild(dupBtn);
      item.appendChild(delBtn);
      item.addEventListener("click", () => this.handleLoad(wf.id));
      list.appendChild(item);
    }
  }

  async handleLoad(id: string): Promise<void> {
    if (this.hasUnsaved) {
      const ok = await this.confirmFn(`You have unsaved changes to "${this.currentName}". Load a different workflow and lose them?`);
      if (!ok) return;
    }
    this.onStatusChange("Loading…");
    try {
      const json = isTauri() ? await loadWorkflow(id) : lsLoad(id);
      if (!json) { this.onStatusChange("Workflow not found"); return; }
      const { id: wfId, name, nodes, connectors, parallelExecution, maxConcurrentNodes, chatSettings, tags } = deserialize(json);
      this.parallelExecution = parallelExecution;
      this.maxConcurrentNodes = maxConcurrentNodes;
      this.chatSettings = chatSettings;
      this.currentTags = tags;
      this.canvas.nodes = nodes; this.canvas.connectors = connectors;
      this.canvas.clearSelection();
      if (localStorage.getItem("aerini_autofit") !== "false") {
        this.canvas.fitToScreen();
      } else {
        // Restore saved viewport for this workflow if available
        const saved = localStorage.getItem(`aerini_viewport_${wfId}`);
        if (saved) {
          try {
            const { panX, panY, zoom } = JSON.parse(saved) as { panX: number; panY: number; zoom: number };
            this.canvas.panX = panX; this.canvas.panY = panY; this.canvas.zoom = zoom;
          } catch { /* ignore corrupt entry */ }
        }
      }
      this.currentId = wfId; this.currentName = name;
      this.markUnsaved(false); this.onTitleChange(name);
      this.onPanelClose?.();
      document.getElementById("output-drawer")!.classList.add("hidden");
      await this.refreshWorkflowList();
      this.onStatusChange(`Loaded "${name}"`);
      this.onNavigate?.();
    } catch (e) {
      this.onStatusChange(`Load failed: ${e}`);
      this.onToast(`Could not load workflow: ${e}`, "error");
    }
  }

  async duplicateWorkflow(id: string, name: string): Promise<void> {
    try {
      const json = isTauri() ? await loadWorkflow(id) : lsLoad(id);
      if (!json) { this.onToast("Could not find workflow to duplicate", "error"); return; }

      // Routes through deserialize/serialize — the same pattern every other
      // load/save path in this file uses — instead of hand-editing the raw
      // parsed JSON, so any validation deserialize() performs (e.g. dropping
      // edges whose from_node/to_node isn't present in the node list) is
      // applied here too.
      const { nodes, connectors, parallelExecution, maxConcurrentNodes, chatSettings, tags } = deserialize(json);

      const newId   = `wf_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
      const newName = `${name} (copy)`;

      // Re-assign all node IDs to avoid collisions, re-keying the Map so
      // each entry's key still matches its own data.id (CanvasNode/Connector
      // objects returned by deserialize() are safe to mutate in place — this
      // duplicate call is the only reference to them).
      const idMap: Record<string, string> = {};
      const newNodes = new Map<string, CanvasNode>();
      for (const node of nodes.values()) {
        const newNodeId = `node_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`;
        idMap[node.data.id] = newNodeId;
        node.data.id = newNodeId;
        newNodes.set(newNodeId, node);
      }

      // deserialize() guarantees every connector's from_node/to_node is a
      // key in `nodes`, so both idMap lookups below are always present.
      const newConnectors = new Map<string, Connector>();
      for (const conn of connectors.values()) {
        const newEdgeId = `edge_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`;
        conn.data.id = newEdgeId;
        conn.data.from_node = idMap[conn.data.from_node];
        conn.data.to_node = idMap[conn.data.to_node];
        newConnectors.set(newEdgeId, conn);
      }

      const dupJson = serialize(newId, newName, newNodes, newConnectors, parallelExecution, maxConcurrentNodes, chatSettings, tags);
      if (isTauri()) await saveWorkflow(dupJson);
      else lsSave(newId, newName, dupJson, tags);
      await this.refreshWorkflowList();
      this.onToast(`Duplicated as "${newName}"`, "success");
    } catch (e) {
      this.onToast(`Duplicate failed: ${e}`, "error");
    }
  }

  // Load an in-memory workflow object directly (used by onboarding example)
  async loadFromObject(obj: { id: string; name: string; nodes: unknown[]; edges: unknown[] }): Promise<void> {
    const json = JSON.stringify({ id: obj.id, name: obj.name, nodes: obj.nodes, edges: obj.edges });
    try {
      const { id: wfId, name, nodes, connectors, parallelExecution, maxConcurrentNodes, chatSettings, tags } = deserialize(json);
      this.parallelExecution = parallelExecution;
      this.maxConcurrentNodes = maxConcurrentNodes;
      this.chatSettings = chatSettings;
      this.currentTags = tags;
      this.canvas.nodes = nodes; this.canvas.connectors = connectors;
      this.canvas.clearSelection();
      this.canvas.fitToScreen();
      this.currentId = wfId; this.currentName = name;
      this.markUnsaved(true); this.onTitleChange(name);
      this.onPanelClose?.();
      document.getElementById("output-drawer")!.classList.add("hidden");
      this.onStatusChange(`Loaded "${name}"`);
      this.onNavigate?.();
    } catch (e) {
      this.onToast(`Could not load example: ${e}`, "error");
    }
  }

  async handleSave(): Promise<void> {
    if (this.currentName === "Untitled") {
      const titleEl = document.getElementById("workflow-name-label");
      if (!titleEl) { this.onStatusChange("Save cancelled — title element not found"); return; }
      const name = await this.startRename(titleEl);
      if (!name?.trim()) { this.onStatusChange("Save cancelled"); return; }
      this.onTitleChange(this.currentName);
    }
    const json = serialize(this.currentId, this.currentName, this.canvas.nodes, this.canvas.connectors, this.parallelExecution, this.maxConcurrentNodes, this.chatSettings, this.currentTags);
    this.onStatusChange("Saving…");
    try {
      if (isTauri()) {
        await saveWorkflow(json);
        // Save a version snapshot for history — fire-and-forget, don't block save
        saveVersion(this.currentId, json).catch(e =>
          console.warn("Aerini: saveVersion failed:", e)
        );
      } else {
        lsSave(this.currentId, this.currentName, json, this.currentTags);
      }
      this.markUnsaved(false);
      this.onToast(`✓ Saved "${this.currentName}"`, "success");
      this.onStatusChange(`✓ Saved "${this.currentName}"`);
      await this.refreshWorkflowList();
    } catch (e) {
      this.onStatusChange(`Save failed: ${e}`);
      this.onToast(`Save failed: ${e}`, "error");
    }
  }

  async handleNew(): Promise<void> {
    if (this.hasUnsaved) {
      const ok = await this.confirmFn(`Start a new workflow? Unsaved changes to "${this.currentName}" will be lost.`);
      if (!ok) return;
    }
    this.currentId          = `wf_${crypto.randomUUID()}`;
    this.currentName        = "Untitled";
    this.currentTags        = [];
    this.parallelExecution  = false;
    this.maxConcurrentNodes = 8;
    this.chatSettings       = { ...DEFAULT_CHAT_SETTINGS };
    this.markUnsaved(false);
    this.canvas.nodes.clear();
    this.canvas.connectors.clear();
    this.canvas.clearSelection();
    this.onPanelClose?.();
    this.onTitleChange("Untitled");
    document.getElementById("output-drawer")!.classList.add("hidden");
    this.onStatusChange("New workflow");
    this.refreshWorkflowList();
    this.onNavigate?.();
  }

  /** Returns version history for the current workflow (SQLite in Tauri, empty in browser). */
  async getVersions(): Promise<VersionRow[]> {
    if (!isTauri()) return [];
    try {
      return await listVersions(this.currentId);
    } catch (e) {
      console.error("Aerini: listVersions failed:", e);
      return [];
    }
  }

  /** Returns the raw snapshot JSON for a single version, or null if not found / not in Tauri. */
  async getVersionJson(versionId: string): Promise<string | null> {
    if (!isTauri()) return null;
    try {
      return await getVersion(versionId);
    } catch (e) {
      console.error("Aerini: getVersion failed:", e);
      return null;
    }
  }

  /** Serializes the live in-memory canvas exactly as a save would, without persisting it. */
  getCurrentJson(): string {
    return serialize(this.currentId, this.currentName, this.canvas.nodes, this.canvas.connectors, this.parallelExecution, this.maxConcurrentNodes, this.chatSettings, this.currentTags);
  }

  /** Restore a version by ID — loads its snapshot onto the canvas. */
  async restoreVersion(versionId: string): Promise<{ id: string; name: string } | null> {
    if (!isTauri()) return null;
    // Awaited and deliberately left to throw out of this function (unlike
    // the fire-and-forget saveVersion calls elsewhere in this file): the
    // caller's own try/catch (VersionPanel's restore handler) reports the
    // real error instead of it collapsing into the generic "version not
    // found" result below.
    const currentJson = await loadWorkflow(this.currentId);
    if (currentJson) {
      await saveVersion(this.currentId, currentJson, "Before restore");
    }
    try {
      const snapshot = await getVersion(versionId);
      if (!snapshot) return null;
      const { id: wfId, name, nodes, connectors, parallelExecution, maxConcurrentNodes, chatSettings, tags } = deserialize(snapshot);
      this.parallelExecution = parallelExecution;
      this.maxConcurrentNodes = maxConcurrentNodes;
      this.chatSettings = chatSettings;
      this.currentTags = tags;
      this.canvas.nodes = nodes;
      this.canvas.connectors = connectors;
      this.canvas.clearSelection();
      this.canvas.fitToScreen();
      this.currentId   = wfId;
      this.currentName = name;
      this.markUnsaved(true);
      this.onTitleChange(name);
      return { id: wfId, name };
    } catch (e) {
      console.error("Aerini: restoreVersion failed:", e);
      return null;
    }
  }

  /** Delete a single version entry by ID. */
  async deleteVersion(versionId: string): Promise<void> {
    if (!isTauri()) return;
    await ipcDeleteVersion(versionId);
  }

  /**
   * Save the live in-memory canvas as a manually named version snapshot.
   * Returns false without writing anything if this workflow has never been
   * saved yet — workflow_versions.workflow_id has an ON DELETE CASCADE FK
   * into workflows(id), so a version row can't exist for a workflow that
   * isn't persisted.
   */
  async saveNamedVersion(message: string): Promise<boolean> {
    if (!isTauri()) return false;
    const persisted = await loadWorkflow(this.currentId);
    if (!persisted) return false;
    const json = serialize(this.currentId, this.currentName, this.canvas.nodes, this.canvas.connectors, this.parallelExecution, this.maxConcurrentNodes, this.chatSettings, this.currentTags);
    await saveVersion(this.currentId, json, message.trim() || undefined);
    return true;
  }

  /**
   * Called before a background run.
   * Saves the current workflow, then returns a frozen snapshot {id, name, json}
   * so the run can proceed independently of whatever canvas is shown next.
   */
  async prepareForBgRun(): Promise<{ id: string; name: string; json: string } | null> {
    if (this.canvas.nodes.size === 0) return null;
    // Ensure the workflow has a name before saving
    if (this.currentName === "Untitled") {
      const titleEl = document.getElementById("workflow-name-label");
      if (!titleEl) return null;
      const name = await this.startRename(titleEl);
      if (!name?.trim()) return null;
      this.onTitleChange(this.currentName);
    }
    // Capture snapshot first — before any save I/O
    const id   = this.currentId;
    const name = this.currentName;
    const json = serialize(id, name, this.canvas.nodes, this.canvas.connectors,
      this.parallelExecution, this.maxConcurrentNodes, this.chatSettings, this.currentTags);
    // Persist to storage — this is required, not optional.
    // The scheduler daemon looks up the workflow from the DB by ID;
    // if the save fails the scheduler will immediately error on first run.
    try {
      if (isTauri()) {
        await saveWorkflow(json);
        saveVersion(id, json).catch(e => console.warn("Aerini: saveVersion failed:", e));
      } else {
        lsSave(id, name, json, this.currentTags);
      }
      this.markUnsaved(false);
      await this.refreshWorkflowList();
    } catch (e) {
      this.onToast(`Could not save workflow before background run: ${e}`, "error");
      return null;
    }
    return { id, name, json };
  }

  handleExport(): void {
    if (this.canvas.nodes.size === 0) {
      this.onToast("Nothing to export — add some nodes first", "error");
      return;
    }

    const json = serialize(this.currentId, this.currentName, this.canvas.nodes, this.canvas.connectors, this.parallelExecution, this.maxConcurrentNodes, this.chatSettings, this.currentTags);
    const obj  = JSON.parse(json) as { id: string; name: string; nodes: unknown[]; edges: unknown[]; metadata?: { author?: string; tags?: string[] } };
    const file = { aerini_version:"1", schema_version:"1.0", id:obj.id, name:obj.name, description:"", author:obj.metadata?.author ?? "", tags:obj.metadata?.tags ?? [], nodes:obj.nodes, edges:obj.edges };
    const content  = JSON.stringify(file, null, 2);
    const filename = `${(this.currentName || "workflow").replace(/\s+/g, "-").toLowerCase()}.aerini`;

    invoke<string>("save_file_dialog", { content, filename })
      .then((savedPath) => {
        // Extract just the filename from the full path for the toast
        const parts = savedPath.replace(/\\/g, "/").split("/");
        const name  = parts[parts.length - 1];
        this.onToast(`✓ Saved "${name}"`, "success");
      })
      .catch((err) => {
        if (err !== "cancelled") {
          this.onToast(`Export failed: ${err}`, "error");
        }
      });
  }

  startRename(el: HTMLElement): Promise<string | null> {
    return new Promise(resolve => {
      const inp = document.createElement("input") as HTMLInputElement;
      inp.type = "text"; inp.value = this.currentName;
      inp.className = "workflow-rename-input";
      inp.autocomplete = "off";
      el.replaceWith(inp); inp.focus(); inp.select();

      let cancelled = false;
      let settled   = false;

      const commit = () => {
        if (settled) return;
        settled = true;
        const v = cancelled ? null : (inp.value.trim() || null);
        this.currentName = v ?? this.currentName;
        if (v) this.markUnsaved(true);
        const lbl = document.createElement("span");
        lbl.id = "workflow-name-label"; lbl.className = "workflow-title";
        lbl.title = "Double-click to rename"; lbl.textContent = this.currentName;
        lbl.addEventListener("dblclick", () => this.startRename(lbl));
        inp.replaceWith(lbl);
        resolve(v);
      };

      inp.addEventListener("blur", commit);
      inp.addEventListener("keydown", e => {
        if (e.key === "Enter")  { inp.blur(); }
        if (e.key === "Escape") { cancelled = true; inp.blur(); }
      });
    }); // end new Promise
  } // end startRename
}
