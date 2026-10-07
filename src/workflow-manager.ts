import {
  listWorkflows, saveWorkflow, loadWorkflow, deleteWorkflow,
  saveVersion, listVersions, getVersion, deleteVersion as ipcDeleteVersion,
  getSetting, setSetting,
  type WorkflowSummary, type VersionRow,
} from "./ipc/workflow";
import { invoke } from "@tauri-apps/api/core";
import { serialize, deserialize, type ChatSettings, type WorkflowDocument, DEFAULT_CHAT_SETTINGS } from "./canvas/CanvasSerializer";
import type { Canvas } from "./canvas/Canvas";
import type { CanvasNode } from "./canvas/Node";
import type { Connector } from "./canvas/Connector";
import { isTauri } from "./utils";
import { activateZone } from "./sidebar-sections";

const LS_KEY = "aerini_workflows_v1";

function lsSave(id: string, name: string, json: string, tags: string[], collectionId: string | null = null): void {
  try {
    const all = JSON.parse(localStorage.getItem(LS_KEY) ?? "{}");
    all[id] = { id, name, json, updated_at: new Date().toISOString(), tags, collection_id: collectionId };
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
function lsDelete(id: string): boolean {
  try {
    const all = JSON.parse(localStorage.getItem(LS_KEY) ?? "{}");
    delete all[id];
    localStorage.setItem(LS_KEY, JSON.stringify(all));
    return true;
  } catch (e) {
    console.error("Aerini: localStorage delete failed", e);
    return false;
  }
}

// -- Workflow Collections (named folders in the Workflows sidebar) --
//
// Collections themselves (id/name/color/order/collapsed) are metadata about
// the sidebar, not about any one workflow, stored as a single JSON blob,
// not a per-workflow field. Persisted via the generic settings key/value
// store in both run modes: get/setSetting (Tauri, already-existing IPC) or
// localStorage (browser), mirroring the get/set-style helpers already used
// for the same purpose in sidebar-sections.ts.
//
// A workflow's *membership* (`collection_id`) is a per-workflow field,
// separate from this blob; see WorkflowSummary.collection_id and
// CanvasSerializer's metadata.collection_id.

export type CollectionColor = "blue" | "green" | "purple" | "amber" | "red" | "slate";
export const COLLECTION_COLORS: readonly CollectionColor[] = ["blue", "green", "purple", "amber", "red", "slate"];

/** Maps a stored color name to an existing theme CSS variable, never a
 *  literal hex, so a collection's color stays correct under both the dark
 *  and paper themes. "slate" reuses --cat-trigger rather than a new token:
 *  its dark-theme value (#8aa9c9) is a suitable neutral, and it already
 *  exists for exactly this "muted category" purpose. */
export function collectionColorVar(color: CollectionColor): string {
  switch (color) {
    case "blue":   return "var(--blue)";
    case "green":  return "var(--green)";
    case "purple": return "var(--purple)";
    case "amber":  return "var(--amber)";
    case "red":    return "var(--red)";
    case "slate":  return "var(--cat-trigger)";
  }
}

export interface CollectionDef {
  id: string;
  name: string;
  color: CollectionColor;
  order: number;
  collapsed: boolean;
}

interface CollectionsBlob {
  collections: CollectionDef[];
  /** Uncategorized is a fixed bucket, not a CollectionDef; its collapsed
   *  state is tracked separately rather than as a fake array entry, so it
   *  can't be accidentally renamed, recolored, or deleted like a real one. */
  uncategorizedCollapsed: boolean;
}

const LS_COLLECTIONS_KEY = "aerini_collections_v1";
const SETTINGS_COLLECTIONS_KEY = "workflow_collections_v1";
const DEFAULT_COLLECTIONS_BLOB: CollectionsBlob = { collections: [], uncategorizedCollapsed: false };

function isCollectionsBlob(v: unknown): v is CollectionsBlob {
  return !!v && typeof v === "object" && Array.isArray((v as CollectionsBlob).collections);
}

function parseCollectionsBlob(raw: string | null): CollectionsBlob {
  if (!raw) return { ...DEFAULT_COLLECTIONS_BLOB };
  try {
    const parsed: unknown = JSON.parse(raw);
    return isCollectionsBlob(parsed) ? parsed : { ...DEFAULT_COLLECTIONS_BLOB };
  } catch {
    return { ...DEFAULT_COLLECTIONS_BLOB };
  }
}

/** Groups a workflow list into per-collection buckets, ordered by
 *  CollectionDef.order, with the always-present Uncategorized bucket last.
 *  Pure: no DOM, no storage, so the grouping logic is directly testable. */
export function groupWorkflowsByCollection(
  wfs: WorkflowSummary[],
  collections: CollectionDef[],
): Array<{ collection: CollectionDef | null; items: WorkflowSummary[] }> {
  const ordered = [...collections].sort((a, b) => a.order - b.order);
  const knownIds = new Set(ordered.map(c => c.id));
  const groups: Array<{ collection: CollectionDef | null; items: WorkflowSummary[] }> = ordered.map(c => ({
    collection: c,
    items: wfs.filter(w => w.collection_id === c.id),
  }));
  // Uncategorized also catches an orphaned collection_id (points at a
  // collection that no longer exists, e.g. deleted from another tab since
  // this list was last loaded) so such a workflow is never silently dropped.
  groups.push({ collection: null, items: wfs.filter(w => !w.collection_id || !knownIds.has(w.collection_id)) });
  return groups;
}

/** Drops the Uncategorized bucket from a render pass when it has no
 *  workflows, so an empty "Uncategorized" folder doesn't sit in the sidebar
 *  as permanent clutter; it reappears the moment a workflow lands there.
 *  Named collections still render empty (they're user-created, intentional,
 *  and are a drop target), so only the null bucket is ever filtered here.
 *  Pure: the data layer (groupWorkflowsByCollection) still always includes
 *  Uncategorized; only this render-facing step hides the empty case. */
export function visibleCollectionGroups(
  groups: Array<{ collection: CollectionDef | null; items: WorkflowSummary[] }>,
): Array<{ collection: CollectionDef | null; items: WorkflowSummary[] }> {
  return groups.filter(g => g.collection !== null || g.items.length > 0);
}

/** Inclusive shift-click range over a flat, already-rendered id order.
 *  Returns [] if either endpoint isn't present (e.g. it was filtered out
 *  by search since the anchor was set) rather than guessing a range. */
export function computeSelectionRange(orderedIds: string[], fromId: string, toId: string): string[] {
  const a = orderedIds.indexOf(fromId);
  const b = orderedIds.indexOf(toId);
  if (a === -1 || b === -1) return [];
  const [lo, hi] = a < b ? [a, b] : [b, a];
  return orderedIds.slice(lo, hi + 1);
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
      dot.className = "wf-run-dot"; dot.title = "Running"; dot.setAttribute("data-tooltip", "Running");
      wrap.appendChild(dot);
    } else {
      item.classList.remove("wf-is-running");
    }
    item.insertBefore(wrap, item.firstChild);
  }
  updateRunningPill();
}

export function isWorkflowRunning(id: string): boolean {
  return _runningWorkflows.has(id);
}

/** Small cross-link pill shown under the Workflows toolbar when one or more
 *  workflows are running in the background. Running workflows are excluded
 *  from this list entirely (see refreshWorkflowList). Background Runs is
 *  their one home (docs/background-runs.md), so this is the only in-panel
 *  indicator that something is running, plus a one-click jump to it. */
function updateRunningPill(): void {
  const pill = document.getElementById("wf-bgruns-pill");
  const label = document.getElementById("wf-bgruns-pill-label");
  if (!pill || !label) return;
  const n = _runningWorkflows.size;
  pill.classList.toggle("hidden", n === 0);
  if (n > 0) {
    label.textContent = n === 1 ? "1 workflow running in background" : `${n} workflows running in background`;
  }
}

export class WorkflowManager {
  canvas: Canvas;
  currentId   = `wf_${crypto.randomUUID()}`;
  currentName = "Untitled";
  currentTags: string[] = [];
  /** Exclusive Workflows-sidebar collection membership. null = Uncategorized. */
  currentCollectionId: string | null = null;
  /** Collection folder definitions (id/name/color/order/collapsed), lazily
   *  hydrated once by ensureCollectionsLoaded(), then kept in memory and
   *  persisted on every mutation. */
  collections: CollectionDef[] = [];
  private collectionsLoaded    = false;
  private uncategorizedCollapsed = false;
  /** Multi-select state (Ctrl/Cmd+Click, Shift+Click, or explicit Select
   *  mode). Session-only, intentionally not persisted, matching the
   *  prototype and the app's other transient UI state (e.g. dropdowns). */
  selectedIds = new Set<string>();
  selectMode  = false;
  private selectAnchorId: string | null = null;
  /** Id of the collection currently showing its inline rename input: either
   *  a freshly created, not-yet-named collection, or an existing one mid
   *  rename via the "⋯" menu. */
  private editingCollectionId: string | null = null;
  /** Set by "+ New collection..." inside the bulk bar's Move-to menu; the
   *  freshly created collection is auto-assigned the pending selection the
   *  moment its name is committed. */
  private pendingAssignAfterCreate: string | null = null;
  /** Per-workflow parallel execution setting. Serialised into workflow JSON. */
  parallelExecution   = false;
  maxConcurrentNodes  = 8;
  /** Desktop-only opt-out of the 24h manual-run ceiling. Serialised into workflow JSON. */
  unlimitedDuration   = false;
  /** Wall-clock limit for the whole run, in seconds. undefined = no limit. Serialised into workflow JSON. */
  maxDurationSecs: number | undefined = undefined;
  /** Per-workflow Chat Panel feature toggles. Serialised into workflow JSON
   *  under "settings.chat"; read by ChatPanel.applyToggles() on panel open. */
  chatSettings: ChatSettings = { ...DEFAULT_CHAT_SETTINGS };
  hasUnsaved  = false;
  private sortMode = "updated_desc";
  /** Guards refreshWorkflowList against overlapping calls (see its doc comment). */
  private _refreshInFlight: Promise<void> | null = null;
  private _refreshQueued = false;

  private autoSaveTimer: ReturnType<typeof setTimeout> | null = null;
  private autoSaveInFlight: Promise<void> | null = null;
  private onUnsavedChange: (u: boolean) => void;
  private onTitleChange:   (n: string)  => void;
  private onStatusChange:  (m: string)  => void;
  private onToast:         (m: string, t: "success" | "error" | "info") => void;
  // Async confirm function that replaces window.confirm, which is suppressed in Tauri
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

    document.getElementById("wf-bgruns-pill")?.addEventListener("click", () => activateZone("bgruns"));
    this.bindBulkBar();

    document.addEventListener("keydown", (e) => {
      if (e.key !== "Escape") return;
      if (this.selectMode || this.selectedIds.size > 0) this.exitSelectMode();
    });
  }

  markUnsaved(on: boolean): void {
    if (!on) this.setAutoSaveFailed(false);
    this.hasUnsaved = on;
    this.onUnsavedChange(on);
  }

  /** Set while the last autosave failed; cleared by the next successful save. */
  autoSaveFailed = false;
  onAutoSaveStateChange: (() => void) | null = null;

  scheduleAutoSave(): void {
    if (this.autoSaveTimer) clearTimeout(this.autoSaveTimer);
    this.autoSaveTimer = setTimeout(() => { void this.runAutoSave(); }, isWorkflowRunning(this.currentId) ? 500 : 30_000);
  }

  /** Runs a pending autosave now instead of waiting out its timer. Resolves true when nothing is left unsaved. */
  async flushAutoSave(): Promise<boolean> {
    if (this.autoSaveTimer || this.autoSaveFailed) await this.runAutoSave();
    else if (this.autoSaveInFlight) await this.autoSaveInFlight;
    return !this.autoSaveFailed;
  }

  private runAutoSave(): Promise<void> {
    if (this.autoSaveTimer) { clearTimeout(this.autoSaveTimer); this.autoSaveTimer = null; }
    if (!this.hasUnsaved && !this.autoSaveFailed) return Promise.resolve();
    const p = this.persistAutoSave().finally(() => { if (this.autoSaveInFlight === p) this.autoSaveInFlight = null; });
    this.autoSaveInFlight = p;
    return p;
  }

  private async persistAutoSave(): Promise<void> {
    const json = serialize(this.currentId, this.currentName, this.canvas.nodes, this.canvas.connectors, this.parallelExecution, this.maxConcurrentNodes, this.chatSettings, this.currentTags, this.unlimitedDuration, this.currentCollectionId, this.maxDurationSecs);
    if (isTauri()) {
      try {
        await saveWorkflow(json);
        this.setAutoSaveFailed(false);
      } catch (e) {
        console.error("Aerini: autosave failed", e);
        this.setAutoSaveFailed(true);
        if (isWorkflowRunning(this.currentId)) {
          this.onToast(`Autosave failed: ${e}`, "error");
        }
      }
    } else {
      lsSave(`autosave_${this.currentId}`, `[autosave] ${this.currentName}`, json, this.currentTags, this.currentCollectionId);
    }
  }

  private setAutoSaveFailed(failed: boolean): void {
    if (this.autoSaveFailed === failed) return;
    this.autoSaveFailed = failed;
    this.onAutoSaveStateChange?.();
  }

  setSortMode(mode: string): void {
    this.sortMode = mode;
    this.refreshWorkflowList();
  }

  // -- Collections: load/persist ------------------------------------------

  private async ensureCollectionsLoaded(): Promise<void> {
    if (this.collectionsLoaded) return;
    this.collectionsLoaded = true; // set before the await, so a second concurrent
    // refreshWorkflowList() call must not also start loading.
    let blob = DEFAULT_COLLECTIONS_BLOB;
    try {
      if (isTauri()) {
        const raw = await getSetting(SETTINGS_COLLECTIONS_KEY);
        blob = parseCollectionsBlob(raw);
      } else {
        blob = parseCollectionsBlob(localStorage.getItem(LS_COLLECTIONS_KEY));
      }
    } catch (e) {
      console.error("Aerini: loading collections failed, starting empty", e);
    }
    this.collections = blob.collections;
    this.uncategorizedCollapsed = blob.uncategorizedCollapsed;
  }

  private async persistCollections(): Promise<void> {
    const blob: CollectionsBlob = { collections: this.collections, uncategorizedCollapsed: this.uncategorizedCollapsed };
    const json = JSON.stringify(blob);
    try {
      if (isTauri()) await setSetting(SETTINGS_COLLECTIONS_KEY, json);
      else localStorage.setItem(LS_COLLECTIONS_KEY, json);
    } catch (e) {
      console.error("Aerini: saving collections failed", e);
      this.onToast("Could not save collection changes", "error");
    }
  }

  // -- Selection -------------------------------------------------------------

  toggleSelectMode(): void {
    this.selectMode = !this.selectMode;
    if (!this.selectMode) { this.selectedIds.clear(); this.selectAnchorId = null; }
    this.refreshWorkflowList();
  }

  exitSelectMode(): void {
    this.selectMode = false;
    this.selectedIds.clear();
    this.selectAnchorId = null;
    this.refreshWorkflowList();
  }

  /** Ctrl/Cmd+Click, a checkbox click, or any click while select mode is on. */
  toggleSelected(id: string): void {
    if (this.selectedIds.has(id)) this.selectedIds.delete(id);
    else this.selectedIds.add(id);
    this.selectAnchorId = id;
    this.refreshWorkflowList();
  }

  /** Shift+Click: range-select from the last-clicked anchor. With no anchor
   *  yet it behaves like a plain toggle would with nothing to anchor on. */
  selectRange(toId: string): void {
    if (!this.selectAnchorId) { this.toggleSelected(toId); return; }
    const list = document.getElementById("workflow-list");
    const ids  = list ? Array.from(list.querySelectorAll<HTMLElement>(".workflow-item")).map(el => el.dataset.wfId!) : [];
    for (const id of computeSelectionRange(ids, this.selectAnchorId, toId)) this.selectedIds.add(id);
    this.refreshWorkflowList();
  }

  // -- Collection CRUD ---------------------------------------------------

  async createCollection(): Promise<void> {
    await this.ensureCollectionsLoaded();
    const maxOrder = this.collections.reduce((m, c) => Math.max(m, c.order), -1);
    const col: CollectionDef = { id: `col_${crypto.randomUUID()}`, name: "", color: "blue", order: maxOrder + 1, collapsed: false };
    this.collections.unshift(col);
    this.editingCollectionId = col.id;
    await this.refreshWorkflowList();
  }

  /** Commits or discards a collection's inline name input (new or renaming).
   *  Empty name on a not-yet-named (freshly created) collection discards it;
   *  empty name on an existing collection reverts to its current name rather
   *  than deleting the whole folder. */
  async commitCollectionRename(id: string, value: string): Promise<void> {
    const col = this.collections.find(c => c.id === id);
    this.editingCollectionId = null;
    if (!col) { await this.refreshWorkflowList(); return; }
    const trimmed = value.trim();
    if (!trimmed) {
      if (col.name === "") {
        this.collections = this.collections.filter(c => c.id !== id);
        if (this.pendingAssignAfterCreate === id) this.pendingAssignAfterCreate = null;
        await this.persistCollections();
        await this.refreshWorkflowList();
        return;
      }
      // Existing collection, cleared to empty on rename; keep its old name.
      await this.refreshWorkflowList();
      return;
    }
    const wasNew = col.name === "";
    col.name = trimmed;
    await this.persistCollections();
    if (wasNew && this.pendingAssignAfterCreate === id) {
      const ids = Array.from(this.selectedIds);
      this.pendingAssignAfterCreate = null;
      await this.moveWorkflowsToCollection(ids, id);
      return; // moveWorkflowsToCollection already refreshes and clears selection
    }
    this.onToast(wasNew ? `Created collection "${trimmed}"` : `Renamed to "${trimmed}"`, "success");
    await this.refreshWorkflowList();
  }

  /** Escape while renaming: discard-if-new, otherwise just close the input. */
  async cancelCollectionRename(id: string): Promise<void> {
    const col = this.collections.find(c => c.id === id);
    this.editingCollectionId = null;
    if (col && col.name === "") {
      this.collections = this.collections.filter(c => c.id !== id);
      if (this.pendingAssignAfterCreate === id) this.pendingAssignAfterCreate = null;
      await this.persistCollections();
    }
    await this.refreshWorkflowList();
  }

  startCollectionRename(id: string): void {
    this.editingCollectionId = id;
    this.refreshWorkflowList();
  }

  async setCollectionColor(id: string, color: CollectionColor): Promise<void> {
    const col = this.collections.find(c => c.id === id);
    if (!col) return;
    col.color = color;
    await this.persistCollections();
    await this.refreshWorkflowList();
  }

  toggleCollectionCollapsed(id: string | null): void {
    if (id === null) {
      this.uncategorizedCollapsed = !this.uncategorizedCollapsed;
    } else {
      const col = this.collections.find(c => c.id === id);
      if (!col) return;
      col.collapsed = !col.collapsed;
    }
    this.persistCollections();
    this.refreshWorkflowList();
  }

  /** Workflows inside move to Uncategorized; nothing is deleted. */
  async deleteCollection(id: string): Promise<void> {
    const col = this.collections.find(c => c.id === id);
    if (!col) return;
    const ok = await this.confirmFn(`Delete "${col.name}"? Workflows inside move to Uncategorized, nothing is deleted.`, true);
    if (!ok) return;
    const wfs = isTauri() ? await listWorkflows().catch(() => []) : lsList();
    const memberIds = wfs.filter(w => w.collection_id === id).map(w => w.id);
    this.collections = this.collections.filter(c => c.id !== id);
    await this.persistCollections();
    if (memberIds.length) await this.moveWorkflowsToCollection(memberIds, null);
    this.onToast(`Deleted collection "${col.name}"`, "success");
    await this.refreshWorkflowList();
  }

  /** Bulk/drag move: reassigns collection_id for each workflow. The
   *  currently-open workflow is re-serialized from the live canvas (so an
   *  in-progress unsaved edit to it isn't clobbered by its last-saved-on-disk
 *  JSON); every other workflow is loaded, patched, and re-saved, the same
   *  load→deserialize→mutate→serialize→save idiom duplicateWorkflow() already
   *  uses elsewhere in this file. */
  async moveWorkflowsToCollection(ids: string[], collectionId: string | null): Promise<void> {
    for (const id of ids) {
      try {
        if (id === this.currentId) {
          this.currentCollectionId = collectionId;
          const json = this.getCurrentJson();
          if (isTauri()) await saveWorkflow(json);
          else lsSave(id, this.currentName, json, this.currentTags, collectionId);
          continue;
        }
        const raw = isTauri() ? await loadWorkflow(id) : lsLoad(id);
        if (!raw) continue;
        const { name, nodes, connectors, parallelExecution, maxConcurrentNodes, unlimitedDuration, chatSettings, tags, maxDurationSecs } = deserialize(raw);
        const json = serialize(id, name, nodes, connectors, parallelExecution, maxConcurrentNodes, chatSettings, tags, unlimitedDuration, collectionId, maxDurationSecs);
        if (isTauri()) await saveWorkflow(json);
        else lsSave(id, name, json, tags, collectionId);
      } catch (e) {
        console.error("Aerini: failed to move workflow to a collection", id, e);
      }
    }
    const n = ids.length;
    const destName = collectionId === null ? "Uncategorized" : (this.collections.find(c => c.id === collectionId)?.name ?? "Uncategorized");
    if (n > 0) this.onToast(`Moved ${n} workflow${n > 1 ? "s" : ""} to "${destName}"`, "success");
    this.selectedIds.clear();
    await this.refreshWorkflowList();
  }

  /** Coalesces overlapping calls. The render below clears #workflow-list
   *  synchronously and appends only after an async gap (collections load,
   *  then the workflow-list IPC call) — two calls landing in that gap, e.g.
   *  back-to-back scheduler-status events, would otherwise each append their
   *  own copy of every group into the same emptied list. A call arriving
   *  while one is already in flight just flags another pass and shares the
   *  in-flight promise instead of starting its own render. */
  async refreshWorkflowList(): Promise<void> {
    if (this._refreshInFlight) {
      this._refreshQueued = true;
      return this._refreshInFlight;
    }
    this._refreshInFlight = (async () => {
      do {
        this._refreshQueued = false;
        await this.renderWorkflowList();
      } while (this._refreshQueued);
    })().finally(() => { this._refreshInFlight = null; });
    return this._refreshInFlight;
  }

  private async renderWorkflowList(): Promise<void> {
    await this.ensureCollectionsLoaded();
    const list = document.getElementById("workflow-list")!;
    list.innerHTML = "";
    list.classList.toggle("select-mode", this.selectMode);

    // Tauri's listWorkflows() can fail (DB read error). Swallowing that to
    // `[]` would render identically to a genuinely empty library -- "you
    // have no workflows" is a much scarier (and wrong) message than a load
    // error. loadFailed keeps the two apart.
    let wfs: WorkflowSummary[];
    let loadFailed = false;
    if (isTauri()) {
      try { wfs = await listWorkflows(); }
      catch (e) { console.error("Aerini: listWorkflows failed:", e); wfs = []; loadFailed = true; }
    } else {
      wfs = lsList();
    }

    if (loadFailed) {
      const err = document.createElement("div");
      err.className = "workflow-list-empty";
      const icon = document.createElement("div");
      icon.className = "workflow-list-empty-icon";
      icon.innerHTML = `<svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="9"/><line x1="12" y1="8" x2="12" y2="13"/><line x1="12" y1="16.5" x2="12.01" y2="16.5"/></svg>`;
      err.appendChild(icon);
      const text = document.createElement("div");
      text.className = "workflow-list-empty-text";
      text.textContent = "Couldn't load your workflows.";
      err.appendChild(text);
      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "btn-sm";
      retry.textContent = "Retry";
      retry.addEventListener("click", () => this.refreshWorkflowList());
      err.appendChild(retry);
      list.appendChild(err);
      return;
    }

    // Running workflows appear only in Background Runs: exclude from this list
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

    updateRunningPill();
    this.updateBulkBar();

    if (!wfs.length) {
      const empty = document.createElement("div");
      empty.className = "workflow-list-empty";
      const icon = document.createElement("div");
      icon.className = "workflow-list-empty-icon";
      icon.innerHTML = `<svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="8.5" y="14" width="7" height="7" rx="1.5"/><path d="M6.5 10v1.5A2.5 2.5 0 0 0 9 14M17.5 10v1.5A2.5 2.5 0 0 1 15 14"/></svg>`;
      empty.appendChild(icon);
      const text = document.createElement("div");
      text.className = "workflow-list-empty-text";
      text.textContent = "No workflows yet — click + to create one.";
      empty.appendChild(text);
      list.appendChild(empty);
      return;
    }

    // Any selected id that no longer exists in the current list (deleted by
    // another path, or filtered out because it started running) shouldn't
    // linger in the bulk bar's count.
    const liveIds = new Set(wfs.map(w => w.id));
    for (const id of this.selectedIds) if (!liveIds.has(id)) this.selectedIds.delete(id);

    for (const { collection, items } of visibleCollectionGroups(groupWorkflowsByCollection(wfs, this.collections))) {
      list.appendChild(this.buildCollectionGroup(collection, items));
    }

    // Roving tabindex: buildWorkflowItem only marks the *open* workflow
    // tabbable. If none of the rendered items is the open one (e.g. it's
    // filtered out, or nothing is open yet), the list would have no
    // keyboard entry point at all -- fall back to the first row.
    if (!list.querySelector('.workflow-item[tabindex="0"]')) {
      list.querySelector<HTMLElement>(".workflow-item")?.setAttribute("tabindex", "0");
    }

    this.updateBulkBar();
  }

  // -- Collection group (header + body) ------------------------------------

  private buildCollectionGroup(collection: CollectionDef | null, items: WorkflowSummary[]): HTMLElement {
    const isUncategorized = collection === null;
    const collapsed = isUncategorized ? this.uncategorizedCollapsed : collection.collapsed;
    const isEditing = !isUncategorized && this.editingCollectionId === collection.id;

    const wrap = document.createElement("div");
    wrap.className = "workflow-collection-group" + (collapsed ? " collapsed" : "");
    wrap.dataset.collectionId = collection?.id ?? "";

    const header = document.createElement("div");
    header.className = "workflow-collection-header";
    header.setAttribute("role", "button");
    header.tabIndex = 0;
    header.setAttribute("aria-expanded", String(!collapsed));

    const chev = document.createElement("span");
    chev.className = "workflow-collection-chev";
    chev.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 9l6 6 6-6"/></svg>`;
    header.appendChild(chev);

    const dot = document.createElement("span");
    dot.className = "workflow-collection-dot";
    dot.style.background = collectionColorVar(collection?.color ?? "slate");
    if (isUncategorized) {
      dot.classList.add("workflow-collection-dot-muted");
    } else {
      dot.title = "Change color";
      dot.setAttribute("data-tooltip", "Change color");
      dot.addEventListener("click", e => { e.stopPropagation(); this.openColorMenu(dot, collection.id); });
    }
    header.appendChild(dot);

    if (isEditing) {
      const inp = document.createElement("input");
      inp.className = "workflow-collection-rename-input";
      inp.value = collection.name;
      inp.placeholder = "Collection name";
      inp.autocomplete = "off";
      header.appendChild(inp);
      setTimeout(() => { inp.focus(); inp.select(); }, 0);

      let settled = false;
      let dismiss: (ev: MouseEvent) => void;
      const commit = () => {
        if (settled) return;
        settled = true;
        document.removeEventListener("mousedown", dismiss, true);
        this.commitCollectionRename(collection.id, inp.value);
      };
      dismiss = (ev: MouseEvent) => { if (ev.target !== inp) commit(); };
      setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);
      inp.addEventListener("blur", commit);
      inp.addEventListener("keydown", e => {
        if (e.key === "Enter") { inp.blur(); }
        if (e.key === "Escape") {
          settled = true;
          document.removeEventListener("mousedown", dismiss, true);
          this.cancelCollectionRename(collection.id);
        }
      });
      // The input sits inside the header; without this, blur-then-click-elsewhere
      // would also fire the header's own collapse-toggle click handler below.
      header.addEventListener("click", e => e.stopPropagation());
    } else {
      const name = document.createElement("span");
      name.className = "workflow-collection-name" + (isUncategorized ? " workflow-collection-name-muted" : "");
      name.textContent = collection?.name ?? "Uncategorized";
      // Full name in the tooltip too, not just the rename hint -- otherwise
      // a name long enough to ellipsis has no way to be read in full.
      name.title = isUncategorized ? "Uncategorized" : `${collection.name} • Double-click to rename`;
      name.setAttribute("data-tooltip", name.title);
      if (!isUncategorized) {
        name.addEventListener("dblclick", e => { e.stopPropagation(); this.startCollectionRename(collection.id); });
      }
      header.appendChild(name);

      const count = document.createElement("span");
      count.className = "workflow-collection-count";
      count.textContent = String(items.length);
      header.appendChild(count);

      if (!isUncategorized) {
        const newBtn = document.createElement("button");
        newBtn.className = "zone-action-btn workflow-collection-new-btn";
        newBtn.title = `New workflow in ${collection.name}`;
        newBtn.setAttribute("data-tooltip", `New workflow in ${collection.name}`);
        newBtn.setAttribute("aria-label", `New workflow in ${collection.name}`);
        newBtn.innerHTML = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>`;
        newBtn.addEventListener("click", e => { e.stopPropagation(); this.handleNew(collection.id); });
        header.appendChild(newBtn);

        const menuBtn = document.createElement("button");
        menuBtn.className = "zone-action-btn workflow-collection-menu-btn";
        menuBtn.title = "Collection options";
        menuBtn.setAttribute("data-tooltip", "Collection options");
        menuBtn.setAttribute("aria-label", `Options for ${collection.name}`);
        menuBtn.innerHTML = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="5" cy="12" r="1.5"/><circle cx="12" cy="12" r="1.5"/><circle cx="19" cy="12" r="1.5"/></svg>`;
        menuBtn.addEventListener("click", e => { e.stopPropagation(); this.openCollectionMenu(menuBtn, collection.id); });
        header.appendChild(menuBtn);
      }

      const toggle = () => this.toggleCollectionCollapsed(collection?.id ?? null);
      header.addEventListener("click", (e: MouseEvent) => {
        // Double-clicking the name to rename it fires two ordinary clicks
        // before the dblclick handler above runs, each toggling
        // collapse/expand -- a visible flicker right before the rename input
        // appears. e.detail (click count) skips the repeats.
        if (e.detail > 1) return;
        toggle();
      });
      header.addEventListener("keydown", e => {
        if (e.key === "Enter" || e.key === " ") { e.preventDefault(); toggle(); }
      });
    }

    wrap.appendChild(header);

    if (!collapsed) {
      const body = document.createElement("div");
      body.className = "workflow-collection-body";
      if (!items.length) {
        const empty = document.createElement("div");
        empty.className = "workflow-collection-empty";
        empty.textContent = isUncategorized ? "No uncategorized workflows" : "Drag workflows here, or use \u201cMove to\u201d";
        body.appendChild(empty);
      } else {
        for (const wf of items) body.appendChild(this.buildWorkflowItem(wf));
      }
      wrap.appendChild(body);
    }

    // Drop target: the whole group (header + collapsed-or-not) accepts drops.
    wrap.addEventListener("dragover", e => { e.preventDefault(); wrap.classList.add("drop-target"); });
    wrap.addEventListener("dragleave", () => wrap.classList.remove("drop-target"));
    wrap.addEventListener("drop", e => {
      e.preventDefault();
      wrap.classList.remove("drop-target");
      let ids: string[] = [];
      try { ids = JSON.parse(e.dataTransfer?.getData("text/plain") || "[]"); } catch { /* ignore malformed payload */ }
      if (Array.isArray(ids) && ids.length) this.moveWorkflowsToCollection(ids, collection?.id ?? null);
    });

    return wrap;
  }

  private buildWorkflowItem(wf: WorkflowSummary): HTMLElement {
    const item = document.createElement("div");
    const selected = this.selectedIds.has(wf.id);
    item.className = "workflow-item" + (selected ? " selected" : "");
    item.dataset.wfId = wf.id;
    item.draggable = true;
    // Roving tabindex: only the open workflow is tabbable by default (falls
    // back to the first row in renderWorkflowList if none is open/visible).
    // aria-current marks it as the current item in the set for assistive tech.
    item.tabIndex = -1;
    if (wf.id === this.currentId) {
      item.classList.add("active");
      item.tabIndex = 0;
      item.setAttribute("aria-current", "true");
    }

    // Run state indicator dot. wf is never running here (filtered above in
    // refreshWorkflowList), so the wrapper starts empty; setWorkflowRunning()
    // patches it live via [data-wf-id] if this workflow starts running before
    // the next full refresh removes it from this list.
    const runWrap = document.createElement("span");
    runWrap.className = "workflow-item-run-state";
    item.appendChild(runWrap);

    const handle = document.createElement("span");
    handle.className = "workflow-item-drag-handle";
    handle.textContent = "⠿";
    item.appendChild(handle);

    const check = document.createElement("input");
    check.type = "checkbox";
    check.className = "workflow-item-check";
    check.checked = selected;
    check.setAttribute("aria-label", `Select ${wf.name}`);
    check.addEventListener("click", e => { e.stopPropagation(); this.toggleSelected(wf.id); });
    item.appendChild(check);

    const nameEl = document.createElement("span");
    nameEl.className = "workflow-item-name";
    nameEl.textContent = wf.name;
    // Double-click to rename in place -- mirrors the collection header's own
    // rename gesture exactly, so a workflow can be renamed without opening it.
    nameEl.addEventListener("dblclick", e => { e.stopPropagation(); this.startItemRename(nameEl, wf); });

    const tags = wf.tags ?? [];
    item.dataset.wfTags = tags.join(" ").toLowerCase();

    const updatedAt = new Date(wf.updated_at);
    const savedLabel = !isNaN(updatedAt.getTime())
      ? `Last saved: ${updatedAt.toLocaleDateString([], { month:"short", day:"numeric", hour:"2-digit", minute:"2-digit" })}`
      : "";
    // Full name first -- otherwise a name long enough to ellipsis in the row
    // has no way to be read in full (the rest of the tooltip only ever
    // carried save-date/tags, never the name itself).
    item.title = [wf.name, savedLabel, tags.length ? `Tags: ${tags.join(", ")}` : ""].filter(Boolean).join(" • ");
    item.setAttribute("data-tooltip", item.title);

    let tagsEl: HTMLElement | null = null;
    if (tags.length) {
      tagsEl = document.createElement("span");
      tagsEl.className = "workflow-item-tags";
      // Cap visible chips so a long tag list can't push the name out or
      // overflow the row; the full list is still in item.title above.
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
    delBtn.innerHTML = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M18 6 6 18"/><path d="M6 6l12 12"/></svg>`;
    delBtn.title = "Delete workflow";
    delBtn.setAttribute("data-tooltip", "Delete workflow");
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
      delBtn.disabled = true;
      if (isTauri()) {
        try {
          await deleteWorkflow(wf.id);
        } catch (err) {
          delBtn.disabled = false;
          this.onToast(`Delete failed: ${err}`, "error");
          return;
        }
      } else if (!lsDelete(wf.id)) {
        delBtn.disabled = false;
        this.onToast("Delete failed: browser storage could not be updated", "error");
        return;
      }
      this.selectedIds.delete(wf.id);
      if (wf.id === this.currentId) this.handleNew();
      else await this.refreshWorkflowList();
    });

    item.appendChild(nameEl);
    if (tagsEl) item.appendChild(tagsEl);

    const dupBtn = document.createElement("button");
    dupBtn.className = "workflow-item-dup";
    dupBtn.title = "Duplicate workflow";
    dupBtn.setAttribute("data-tooltip", "Duplicate workflow");
    dupBtn.innerHTML = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>`;
    dupBtn.addEventListener("click", async e => {
      e.stopPropagation();
      dupBtn.disabled = true;
      await this.duplicateWorkflow(wf.id, wf.name);
      dupBtn.disabled = false;
    });

    item.appendChild(dupBtn);
    item.appendChild(delBtn);

    // Shared by mouse click and Enter/Space so keyboard activation matches
    // click behavior exactly (select-mode toggle / ctrl-toggle / shift-range
    // / plain load), instead of duplicating the branching twice.
    const activate = (mods: { ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }) => {
      if (this.selectMode) { this.toggleSelected(wf.id); return; }
      if (mods.ctrlKey || mods.metaKey) { this.toggleSelected(wf.id); return; }
      if (mods.shiftKey) { this.selectRange(wf.id); return; }
      // Plain click/Enter, not in select mode: existing behavior is to open onto canvas.
      this.handleLoad(wf.id);
    };

    item.addEventListener("click", (e: MouseEvent) => {
      // e.detail is the click count within the double-click window (1, then
      // 2, ...). Without this guard, double-clicking the name to rename it
      // would first fire two ordinary clicks -- each one reloading the
      // workflow onto the canvas -- before the dblclick handler below ever
      // runs. Only the first click of any click/dblclick sequence activates;
      // the row's own dblclick listener (name) handles the rest.
      if (e.detail > 1) return;
      activate(e);
    });

    item.addEventListener("keydown", (e: KeyboardEvent) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        activate(e);
      } else if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Home" || e.key === "End") {
        e.preventDefault();
        this.moveItemFocus(item, e.key);
      } else if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
        e.preventDefault();
        const r = item.getBoundingClientRect();
        this.openItemContextMenu(r.left + 24, r.top + r.height / 2, wf, item);
      }
    });

    item.addEventListener("contextmenu", (e: MouseEvent) => {
      e.preventDefault();
      this.openItemContextMenu(e.clientX, e.clientY, wf, item);
    });

    item.addEventListener("dragstart", (e: DragEvent) => {
      const ids = this.selectedIds.has(wf.id) && this.selectedIds.size > 1
        ? Array.from(this.selectedIds)
        : [wf.id];
      e.dataTransfer?.setData("text/plain", JSON.stringify(ids));
      if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
      item.classList.add("dragging");
    });
    item.addEventListener("dragend", () => item.classList.remove("dragging"));

    return item;
  }

  /** Roving-tabindex focus move for the flat, currently-rendered `.workflow-item`
   *  order. `offsetParent !== null` filters out rows hidden by the search
   *  filter (sidebar-sections.ts sets style.display = "none" on both the item
   *  and, when appropriate, its whole collection group) without duplicating
   *  that visibility logic here. Collapsed collections don't need filtering:
   *  their items simply aren't in the DOM. */
  private moveItemFocus(current: HTMLElement, key: string): void {
    const list = document.getElementById("workflow-list");
    if (!list) return;
    const visible = Array.from(list.querySelectorAll<HTMLElement>(".workflow-item"))
      .filter(el => el.offsetParent !== null);
    if (!visible.length) return;
    const idx = visible.indexOf(current);
    let next: HTMLElement | undefined;
    if      (key === "ArrowDown") next = visible[Math.min(idx + 1, visible.length - 1)];
    else if (key === "ArrowUp")   next = visible[Math.max(idx - 1, 0)];
    else if (key === "Home")      next = visible[0];
    else if (key === "End")       next = visible[visible.length - 1];
    if (!next || next === current) return;
    current.tabIndex = -1;
    next.tabIndex = 0;
    next.focus();
  }

  /** Double-click-to-rename for a sidebar row, mirroring
   *  buildCollectionGroup's inline collection-rename input one-for-one. */
  private startItemRename(nameEl: HTMLElement, wf: WorkflowSummary): void {
    const inp = document.createElement("input");
    inp.className = "workflow-item-rename-input";
    inp.value = wf.name;
    inp.autocomplete = "off";
    nameEl.replaceWith(inp);
    inp.focus(); inp.select();

    let settled = false;
    let dismiss: (ev: MouseEvent) => void;
    const restore = (text: string): HTMLElement => {
      const span = document.createElement("span");
      span.className = "workflow-item-name";
      span.textContent = text;
      span.addEventListener("dblclick", e => { e.stopPropagation(); this.startItemRename(span, wf); });
      inp.replaceWith(span);
      return span;
    };
    const commit = () => {
      if (settled) return;
      settled = true;
      document.removeEventListener("mousedown", dismiss, true);
      const value = inp.value.trim();
      if (!value || value === wf.name) { restore(wf.name); return; }
      restore(value); // optimistic: renameWorkflowById re-syncs from disk if the save actually fails
      void this.renameWorkflowById(wf.id, value);
    };
    dismiss = (ev: MouseEvent) => { if (ev.target !== inp) commit(); };
    setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);
    inp.addEventListener("blur", commit);
    inp.addEventListener("keydown", e => {
      e.stopPropagation(); // don't let Enter/Escape reach the row's own roving-nav handler
      if (e.key === "Enter") inp.blur();
      if (e.key === "Escape") {
        settled = true;
        document.removeEventListener("mousedown", dismiss, true);
        restore(wf.name);
      }
    });
  }

  /** Renames a workflow by id from the sidebar list, whether or not it's
   *  currently open.
   *
   *  For the *open* workflow this only updates in-memory state and marks it
   *  unsaved -- mirroring startRename (the canvas title-bar rename), which
   *  never eagerly persists either. Eagerly saving here would also silently
   *  persist whatever unsaved canvas edits happen to be live, which a
   *  sidebar-row rename click doesn't imply. The row's own text was already
   *  updated optimistically by startItemRename's caller, and a refresh here
   *  would revert it to the still-on-disk old name, so this branch does not
   *  refresh the list.
   *
   *  For any other workflow there's no live canvas at risk, so this persists
   *  immediately via the same load -> deserialize -> mutate -> serialize ->
   *  save idiom duplicateWorkflow/moveWorkflowsToCollection already use, then
   *  refreshes -- needed so a name-based sort order picks up the new name. */
  async renameWorkflowById(id: string, trimmedName: string): Promise<void> {
    if (id === this.currentId) {
      this.currentName = trimmedName;
      this.markUnsaved(true);
      this.onTitleChange(trimmedName);
      return;
    }
    try {
      const raw = isTauri() ? await loadWorkflow(id) : lsLoad(id);
      if (!raw) {
        this.onToast("Could not find workflow to rename", "error");
        await this.refreshWorkflowList();
        return;
      }
      const { name, nodes, connectors, parallelExecution, maxConcurrentNodes, unlimitedDuration, chatSettings, tags, collectionId, maxDurationSecs } = deserialize(raw);
      if (trimmedName === name) return;
      const json = serialize(id, trimmedName, nodes, connectors, parallelExecution, maxConcurrentNodes, chatSettings, tags, unlimitedDuration, collectionId, maxDurationSecs);
      if (isTauri()) await saveWorkflow(json);
      else lsSave(id, trimmedName, json, tags, collectionId);
      await this.refreshWorkflowList();
    } catch (e) {
      this.onToast(`Rename failed: ${e}`, "error");
      await this.refreshWorkflowList(); // re-sync the row back to its real, on-disk name
    }
  }

  /** Right-click / ContextMenu-key menu for a single sidebar row. Reuses the
   *  .ctx-menu styling and icon set canvas/ContextMenu.ts already defines for
   *  node right-click menus, so both of the app's context menus look and
   *  behave identically -- this file doesn't import that class itself since
   *  it's typed against Canvas/CanvasNode, not a workflow row. */
  private openItemContextMenu(x: number, y: number, wf: WorkflowSummary, item: HTMLElement): void {
    document.getElementById("wf-item-ctx-menu")?.remove();
    const menu = document.createElement("div");
    menu.id = "wf-item-ctx-menu";
    menu.className = "ctx-menu";
    menu.setAttribute("role", "menu");

    const addItem = (label: string, icon: string, danger: boolean, action: () => void) => {
      const btn = document.createElement("button");
      btn.className = "ctx-menu-item" + (danger ? " ctx-menu-item--danger" : "");
      btn.setAttribute("role", "menuitem");
      btn.innerHTML = `<span class="ctx-menu-icon">${icon}</span><span>${label}</span>`;
      btn.addEventListener("mousedown", ev => { ev.preventDefault(); menu.remove(); action(); });
      menu.appendChild(btn);
    };
    const addSep = () => {
      const sep = document.createElement("div");
      sep.className = "ctx-menu-sep";
      menu.appendChild(sep);
    };

    addItem(
      "Rename",
      `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>`,
      false,
      () => {
        const nameEl = item.querySelector<HTMLElement>(".workflow-item-name");
        if (nameEl) this.startItemRename(nameEl, wf);
      }
    );
    addItem(
      "Duplicate",
      `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>`,
      false,
      () => { void this.duplicateWorkflow(wf.id, wf.name); }
    );
    addSep();
    addItem(
      "Delete",
      `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/><path d="M9 6V4h6v2"/></svg>`,
      true,
      // Reuses the row's own delete button click handler (running-check +
      // confirm dialog included) instead of re-implementing it here.
      () => item.querySelector<HTMLButtonElement>(".workflow-item-del")?.click()
    );

    document.body.appendChild(menu);
    const mw = menu.offsetWidth  || 178;
    const mh = menu.offsetHeight || 120;
    menu.style.left = `${x + mw > window.innerWidth  - 8 ? x - mw : x}px`;
    menu.style.top  = `${y + mh > window.innerHeight - 8 ? y - mh : y}px`;

    const dismiss = (ev: MouseEvent) => {
      if (!menu.contains(ev.target as Node)) {
        menu.remove();
        document.removeEventListener("mousedown", dismiss, true);
      }
    };
    setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);
  }

  // -- Bulk action bar ------------------------------------------------------

  private updateBulkBar(): void {
    const bar = document.getElementById("wf-bulk-bar");
    const count = document.getElementById("wf-bulk-count");
    if (!bar || !count) return;
    const n = this.selectedIds.size;
    bar.classList.toggle("hidden", n === 0);
    if (n > 0) count.textContent = `${n} selected`;
  }

  /** One-time wiring for the bulk bar's static buttons (index.html), called
   *  once from the constructor. Reads this.selectedIds live at click time, so
   *  it never needs rebinding on refresh (unlike the per-item buttons, which
   *  are rebuilt every render because they're one per row). */
  private bindBulkBar(): void {
    document.getElementById("wf-bulk-clear")?.addEventListener("click", () => this.exitSelectMode());

    document.getElementById("wf-bulk-delete")?.addEventListener("click", async () => {
      const ids = Array.from(this.selectedIds);
      if (!ids.length) return;
      const running = ids.filter(id => isWorkflowRunning(id));
      if (running.length) {
        this.onToast(`${running.length} selected workflow${running.length > 1 ? "s are" : " is"} running in the background. Stop before deleting.`, "error");
        return;
      }
      const ok = await this.confirmFn(`Delete ${ids.length} workflow${ids.length > 1 ? "s" : ""}? This cannot be undone.`, true);
      if (!ok) return;
      const btn = document.getElementById("wf-bulk-delete") as HTMLButtonElement;
      const btnLabel = btn.textContent;
      btn.disabled = true;
      btn.textContent = "Deleting…";
      try {
        let deleted = 0;
        let openedWasDeleted = false;
        for (const id of ids) {
          try {
            let removed: boolean;
            if (isTauri()) { await deleteWorkflow(id); removed = true; }
            else removed = lsDelete(id);
            if (removed) {
              deleted++;
              if (id === this.currentId) openedWasDeleted = true;
            }
          } catch (e) {
            console.error("Aerini: failed to delete workflow", id, e);
          }
        }
        if (deleted === ids.length) {
          this.onToast(`Deleted ${ids.length} workflow${ids.length > 1 ? "s" : ""}`, "success");
        } else {
          this.onToast(`Deleted ${deleted} of ${ids.length} workflows; ${ids.length - deleted} could not be deleted`, "error");
        }
        this.selectedIds.clear();
        if (openedWasDeleted) this.handleNew();
        else await this.refreshWorkflowList();
      } finally {
        btn.disabled = false;
        btn.textContent = btnLabel;
      }
    });

    document.getElementById("wf-bulk-duplicate")?.addEventListener("click", async () => {
      const ids = Array.from(this.selectedIds);
      if (!ids.length) return;
      const btn = document.getElementById("wf-bulk-duplicate") as HTMLButtonElement;
      const btnLabel = btn.textContent;
      btn.disabled = true;
      btn.textContent = "Duplicating…";
      try {
        const wfs = isTauri() ? await listWorkflows().catch(() => []) : lsList();
        const byId = new Map(wfs.map(w => [w.id, w]));
        for (const id of ids) {
          const wf = byId.get(id);
          if (wf) await this.duplicateWorkflow(id, wf.name);
        }
        this.selectedIds.clear();
        await this.refreshWorkflowList();
      } finally {
        btn.disabled = false;
        btn.textContent = btnLabel;
      }
    });

    document.getElementById("wf-bulk-move")?.addEventListener("click", e => {
      e.stopPropagation();
      const anchor = document.getElementById("wf-bulk-move")!;
      this.openMoveToMenu(anchor);
    });
  }

  // -- Dropdown menus (collection "...", color picker, bulk "Move to") ------
  // All three share the same append-to-body / position-under-anchor /
  // dismiss-on-outside-click shape already used by the sort and filter
  // dropdowns in sidebar-sections.ts.

  private openCollectionMenu(anchor: HTMLElement, collectionId: string): void {
    document.getElementById("wf-dropdown")?.remove();
    const rect = anchor.getBoundingClientRect();
    const dd = document.createElement("div");
    dd.id = "wf-dropdown";
    dd.className = "filter-dropdown";
    dd.style.top = `${rect.bottom + 4}px`;
    dd.style.left = `${Math.max(4, rect.right - 150)}px`;

    const mkItem = (label: string, onClick: () => void, danger = false) => {
      const btn = document.createElement("button");
      btn.className = "filter-dropdown-item" + (danger ? " filter-dropdown-item-danger" : "");
      btn.textContent = label;
      btn.addEventListener("mousedown", ev => { ev.preventDefault(); dd.remove(); onClick(); });
      dd.appendChild(btn);
    };
    mkItem("New workflow here", () => this.handleNew(collectionId));
    const sep1 = document.createElement("div");
    sep1.className = "filter-dropdown-sep";
    dd.appendChild(sep1);
    mkItem("Rename", () => this.startCollectionRename(collectionId));
    mkItem("Change color", () => this.openColorMenu(anchor, collectionId));
    const sep2 = document.createElement("div");
    sep2.className = "filter-dropdown-sep";
    dd.appendChild(sep2);
    mkItem("Delete collection", () => this.deleteCollection(collectionId), true);

    document.body.appendChild(dd);
    const dismiss = (ev: MouseEvent) => {
      if (!dd.contains(ev.target as Node) && ev.target !== anchor) {
        dd.remove(); document.removeEventListener("mousedown", dismiss, true);
      }
    };
    setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);
  }

  private openColorMenu(anchor: HTMLElement, collectionId: string): void {
    document.getElementById("wf-dropdown")?.remove();
    const rect = anchor.getBoundingClientRect();
    const dd = document.createElement("div");
    dd.id = "wf-dropdown";
    dd.className = "filter-dropdown workflow-collection-swatch-row";
    dd.style.top = `${rect.bottom + 4}px`;
    dd.style.left = `${rect.left}px`;

    const current = this.collections.find(c => c.id === collectionId)?.color;
    for (const color of COLLECTION_COLORS) {
      const sw = document.createElement("button");
      sw.className = "workflow-collection-swatch" + (color === current ? " selected" : "");
      sw.style.background = collectionColorVar(color);
      sw.title = color;
      sw.setAttribute("data-tooltip", color);
      sw.setAttribute("aria-label", `Set color ${color}`);
      sw.addEventListener("mousedown", ev => { ev.preventDefault(); dd.remove(); this.setCollectionColor(collectionId, color); });
      dd.appendChild(sw);
    }

    document.body.appendChild(dd);
    const dismiss = (ev: MouseEvent) => {
      if (!dd.contains(ev.target as Node) && ev.target !== anchor) {
        dd.remove(); document.removeEventListener("mousedown", dismiss, true);
      }
    };
    setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);
  }

  private openMoveToMenu(anchor: HTMLElement): void {
    document.getElementById("wf-dropdown")?.remove();
    const rect = anchor.getBoundingClientRect();
    const dd = document.createElement("div");
    dd.id = "wf-dropdown";
    dd.className = "filter-dropdown";
    dd.style.top = `${rect.bottom + 4}px`;
    dd.style.left = `${rect.left}px`;

    const mkItem = (label: string, color: string | null, onClick: () => void) => {
      const btn = document.createElement("button");
      btn.className = "filter-dropdown-item workflow-collection-move-item";
      if (color) {
        const sw = document.createElement("span");
        sw.className = "workflow-collection-swatch-inline";
        sw.style.background = color;
        btn.appendChild(sw);
      }
      btn.appendChild(document.createTextNode(label));
      btn.addEventListener("mousedown", ev => { ev.preventDefault(); dd.remove(); onClick(); });
      dd.appendChild(btn);
    };

    const ids = Array.from(this.selectedIds);
    mkItem("Uncategorized", collectionColorVar("slate"), () => this.moveWorkflowsToCollection(ids, null));
    for (const c of [...this.collections].sort((a, b) => a.order - b.order)) {
      mkItem(c.name, collectionColorVar(c.color), () => this.moveWorkflowsToCollection(ids, c.id));
    }
    const sep = document.createElement("div");
    sep.className = "filter-dropdown-sep";
    dd.appendChild(sep);
    mkItem("+ New collection…", null, async () => {
      this.pendingAssignAfterCreate = null; // set below once the collection exists
      const maxOrder = this.collections.reduce((m, c) => Math.max(m, c.order), -1);
      const col: CollectionDef = { id: `col_${crypto.randomUUID()}`, name: "", color: "blue", order: maxOrder + 1, collapsed: false };
      this.collections.unshift(col);
      this.editingCollectionId = col.id;
      this.pendingAssignAfterCreate = col.id;
      await this.refreshWorkflowList();
    });

    document.body.appendChild(dd);
    const dismiss = (ev: MouseEvent) => {
      if (!dd.contains(ev.target as Node) && ev.target !== anchor) {
        dd.remove(); document.removeEventListener("mousedown", dismiss, true);
      }
    };
    setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);
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
      const { id: wfId, name, nodes, connectors, parallelExecution, maxConcurrentNodes, unlimitedDuration, chatSettings, tags, collectionId, maxDurationSecs } = deserialize(json);
      this.parallelExecution = parallelExecution;
      this.maxConcurrentNodes = maxConcurrentNodes;
      this.unlimitedDuration = unlimitedDuration;
      this.maxDurationSecs = maxDurationSecs;
      this.chatSettings = chatSettings;
      this.currentTags = tags;
      this.currentCollectionId = collectionId;
      this.canvas.nodes = nodes; this.canvas.connectors = connectors;
      this.canvas.clearSelection();
      this.canvas.warnArityViolations();
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

      // Routes through deserialize/serialize, the same pattern every other
      // load/save path in this file uses, instead of hand-editing the raw
      // parsed JSON, so any validation deserialize() performs (e.g. dropping
      // edges whose from_node/to_node isn't present in the node list) is
      // applied here too.
      const { nodes, connectors, parallelExecution, maxConcurrentNodes, unlimitedDuration, chatSettings, tags, collectionId, maxDurationSecs } = deserialize(json);

      const newId   = `wf_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
      const newName = `${name} (copy)`;

      // Re-assign all node IDs to avoid collisions, re-keying the Map so
      // each entry's key still matches its own data.id (CanvasNode/Connector
      // objects returned by deserialize() are safe to mutate in place, since this
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

      const dupJson = serialize(newId, newName, newNodes, newConnectors, parallelExecution, maxConcurrentNodes, chatSettings, tags, unlimitedDuration, collectionId, maxDurationSecs);
      if (isTauri()) await saveWorkflow(dupJson);
      else lsSave(newId, newName, dupJson, tags, collectionId);
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
      const { id: wfId, name, nodes, connectors, parallelExecution, maxConcurrentNodes, unlimitedDuration, chatSettings, tags, collectionId, maxDurationSecs } = deserialize(json);
      this.parallelExecution = parallelExecution;
      this.maxConcurrentNodes = maxConcurrentNodes;
      this.unlimitedDuration = unlimitedDuration;
      this.maxDurationSecs = maxDurationSecs;
      this.chatSettings = chatSettings;
      this.currentTags = tags;
      this.currentCollectionId = collectionId;
      this.canvas.nodes = nodes; this.canvas.connectors = connectors;
      this.canvas.clearSelection();
      this.canvas.warnArityViolations();
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
      if (!titleEl) { this.onStatusChange("Save cancelled, title element not found"); return; }
      const name = await this.startRename(titleEl);
      if (!name?.trim()) { this.onStatusChange("Save cancelled"); return; }
      this.onTitleChange(this.currentName);
    }
    const json = serialize(this.currentId, this.currentName, this.canvas.nodes, this.canvas.connectors, this.parallelExecution, this.maxConcurrentNodes, this.chatSettings, this.currentTags, this.unlimitedDuration, this.currentCollectionId, this.maxDurationSecs);
    this.onStatusChange("Saving…");
    try {
      if (isTauri()) {
        await saveWorkflow(json);
        // Save a version snapshot for history, fire-and-forget, don't block save
        saveVersion(this.currentId, json).catch(e =>
          console.warn("Aerini: saveVersion failed:", e)
        );
      } else {
        lsSave(this.currentId, this.currentName, json, this.currentTags, this.currentCollectionId);
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

  async handleNew(collectionId: string | null = null): Promise<void> {
    if (this.hasUnsaved) {
      const ok = await this.confirmFn(`Start a new workflow? Unsaved changes to "${this.currentName}" will be lost.`);
      if (!ok) return;
    }
    this.currentId          = `wf_${crypto.randomUUID()}`;
    this.currentName        = "Untitled";
    this.currentTags        = [];
    this.currentCollectionId = collectionId;
    this.parallelExecution  = false;
    this.maxConcurrentNodes = 8;
    this.unlimitedDuration  = false;
    this.maxDurationSecs    = undefined;
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
    return serialize(this.currentId, this.currentName, this.canvas.nodes, this.canvas.connectors, this.parallelExecution, this.maxConcurrentNodes, this.chatSettings, this.currentTags, this.unlimitedDuration, this.currentCollectionId, this.maxDurationSecs);
  }

  /** Restore a version by ID; loads its snapshot onto the canvas. */
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
      const { id: wfId, name, nodes, connectors, parallelExecution, maxConcurrentNodes, unlimitedDuration, chatSettings, tags, collectionId, maxDurationSecs } = deserialize(snapshot);
      this.parallelExecution = parallelExecution;
      this.maxConcurrentNodes = maxConcurrentNodes;
      this.unlimitedDuration = unlimitedDuration;
      this.maxDurationSecs = maxDurationSecs;
      this.chatSettings = chatSettings;
      this.currentTags = tags;
      this.currentCollectionId = collectionId;
      this.canvas.nodes = nodes;
      this.canvas.connectors = connectors;
      this.canvas.clearSelection();
      this.canvas.warnArityViolations();
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
   * saved yet: workflow_versions.workflow_id has an ON DELETE CASCADE FK
   * into workflows(id), so a version row can't exist for a workflow that
   * isn't persisted.
   */
  async saveNamedVersion(message: string): Promise<boolean> {
    if (!isTauri()) return false;
    const persisted = await loadWorkflow(this.currentId);
    if (!persisted) return false;
    const json = serialize(this.currentId, this.currentName, this.canvas.nodes, this.canvas.connectors, this.parallelExecution, this.maxConcurrentNodes, this.chatSettings, this.currentTags, this.unlimitedDuration, this.currentCollectionId, this.maxDurationSecs);
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
    // Capture snapshot first, before any save I/O
    const id   = this.currentId;
    const name = this.currentName;
    const json = serialize(id, name, this.canvas.nodes, this.canvas.connectors,
      this.parallelExecution, this.maxConcurrentNodes, this.chatSettings, this.currentTags, this.unlimitedDuration, this.currentCollectionId, this.maxDurationSecs);
    // Persist to storage. This is required, not optional.
    // The scheduler daemon looks up the workflow from the DB by ID;
    // if the save fails the scheduler will immediately error on first run.
    try {
      if (isTauri()) {
        await saveWorkflow(json);
        saveVersion(id, json).catch(e => console.warn("Aerini: saveVersion failed:", e));
      } else {
        lsSave(id, name, json, this.currentTags, this.currentCollectionId);
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
      this.onToast("Nothing to export, add some nodes first", "error");
      return;
    }

    const json = serialize(this.currentId, this.currentName, this.canvas.nodes, this.canvas.connectors, this.parallelExecution, this.maxConcurrentNodes, this.chatSettings, this.currentTags, this.unlimitedDuration, this.currentCollectionId, this.maxDurationSecs);
    const obj  = JSON.parse(json) as WorkflowDocument;
    // collection_id is deliberately not copied into the exported file; it's
    // this sidebar's local folder organization, not a portable property of
    // the workflow itself. Every other field mirrors the native schema so
    // re-importing this file is lossless.
    obj.metadata.collection_id = null;
    const file = { aerini_version: "1", ...obj };
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
      let dismiss: (ev: MouseEvent) => void;

      const commit = () => {
        if (settled) return;
        settled = true;
        document.removeEventListener("mousedown", dismiss, true);
        const v = cancelled ? null : (inp.value.trim() || null);
        this.currentName = v ?? this.currentName;
        if (v) this.markUnsaved(true);
        const lbl = document.createElement("span");
        lbl.id = "workflow-name-label"; lbl.className = "workflow-title";
        lbl.title = "Double-click to rename"; lbl.setAttribute("data-tooltip", "Double-click to rename"); lbl.textContent = this.currentName;
        lbl.addEventListener("dblclick", () => this.startRename(lbl));
        inp.replaceWith(lbl);
        resolve(v);
      };

      dismiss = (ev: MouseEvent) => { if (ev.target !== inp) commit(); };
      setTimeout(() => document.addEventListener("mousedown", dismiss, true), 0);

      inp.addEventListener("blur", commit);
      inp.addEventListener("keydown", e => {
        if (e.key === "Enter")  { inp.blur(); }
        if (e.key === "Escape") { cancelled = true; inp.blur(); }
      });
    }); // end new Promise
  } // end startRename
}
