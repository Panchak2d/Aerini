import type { WorkflowManager } from "../workflow-manager";
import { showConfirm } from "../confirm";
import { escapeHtml as escHtml } from "../utils";
import type { VersionRow } from "../ipc/workflow";
import { showDiffPanel } from "./DiffPanel";

type Toast = (m: string, t: "success" | "error" | "info") => void;

export async function showVersionPanel(
  wfManager: WorkflowManager,
  toast: Toast,
): Promise<void> {
  document.getElementById("version-panel-overlay")?.remove();

  const overlay = document.createElement("div");
  overlay.id = "version-panel-overlay";
  overlay.className = "version-overlay";

  const panel = document.createElement("div");
  panel.className = "version-panel";

  const hdr = document.createElement("div");
  hdr.className = "version-panel-header";
  hdr.innerHTML = `
    <span class="version-panel-title">Version History</span>
    <span class="unsaved-dot${wfManager.hasUnsaved ? " visible" : ""}" title="Unsaved changes on canvas" data-tooltip="Unsaved changes on canvas"></span>`;
  const closeBtn = document.createElement("button");
  closeBtn.className = "popover-close";
  closeBtn.innerHTML = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
  closeBtn.addEventListener("click", () => overlay.remove());
  hdr.appendChild(closeBtn);
  panel.appendChild(hdr);

  const toolbar = document.createElement("div");
  toolbar.className = "version-panel-toolbar";
  toolbar.innerHTML = `
    <div class="version-snapshot-row">
      <input class="version-snapshot-input" type="text" placeholder="Snapshot message (optional)" aria-label="Snapshot message" autocomplete="off" spellcheck="false" />
      <button class="version-snapshot-btn">Save Snapshot</button>
    </div>
    <div class="version-search-row hidden">
      <input class="version-search-input" type="text" placeholder="Filter versions…" aria-label="Filter versions" autocomplete="off" spellcheck="false" />
    </div>`;
  panel.appendChild(toolbar);

  const body = document.createElement("div");
  body.className = "version-panel-body";
  body.innerHTML = `<div class="version-empty">Loading…</div>`;
  panel.appendChild(body);
  overlay.appendChild(panel);
  document.body.appendChild(overlay);
  overlay.addEventListener("click", e => { if (e.target === overlay) overlay.remove(); });

  const snapshotInput = toolbar.querySelector<HTMLInputElement>(".version-snapshot-input")!;
  const snapshotBtn   = toolbar.querySelector<HTMLButtonElement>(".version-snapshot-btn")!;
  const searchRow     = toolbar.querySelector<HTMLElement>(".version-search-row")!;
  const searchInput   = toolbar.querySelector<HTMLInputElement>(".version-search-input")!;

  let versions: VersionRow[] = await wfManager.getVersions();

  // The most recent version (versions[0] — list_versions orders DESC) is
  // "Current" exactly when the canvas has no unsaved changes: hasUnsaved
  // already tracks divergence from the last full save, the same moment
  // save_version() snapshots. Diffing serialized JSON instead isn't
  // reliable here — CanvasSerializer restamps created_at/updated_at on
  // every call, so two saves of identical content never come out byte-
  // identical.
  function isCurrent(index: number): boolean {
    return index === 0 && !wfManager.hasUnsaved;
  }

  function renderItem(v: VersionRow, current: boolean): HTMLElement {
    const item = document.createElement("div");
    item.className = "version-item";
    const dt    = new Date(v.created_at);
    const label = dt.toLocaleDateString([], { month: "short", day: "numeric" })
      + " " + dt.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    const displayMsg = v.message ?? "Saved";
    item.dataset.search = `${displayMsg} ${label}`.toLowerCase();
    item.innerHTML = `
      <div class="version-item-meta">
        <span class="version-item-name" title="${escHtml(displayMsg)}">${escHtml(displayMsg)}${current ? ' <span class="version-current-badge">Current</span>' : ""}</span>
        <span class="version-item-date">${label}</span>
      </div>`;

    const actions = document.createElement("div");
    actions.className = "version-item-actions";

    const compareBtn = document.createElement("button");
    compareBtn.className = "version-compare-btn";
    compareBtn.textContent = "Compare";
    // Diffs this version against the live canvas as it stands right now
    // (not versions[0]/the last save) — unambiguous regardless of
    // hasUnsaved, and needs no extra IPC round trip.
    compareBtn.addEventListener("click", async () => {
      compareBtn.disabled = true;
      const compareLabel = compareBtn.textContent;
      compareBtn.textContent = "Comparing…";
      try {
        const oldJson = await wfManager.getVersionJson(v.id);
        if (!oldJson) {
          toast("Compare failed — version not found", "error");
          return;
        }
        showDiffPanel(`${displayMsg} — ${label}`, oldJson, "Current canvas", wfManager.getCurrentJson());
      } catch (e) {
        toast(`Compare failed: ${e}`, "error");
      } finally {
        compareBtn.disabled = false;
        compareBtn.textContent = compareLabel;
      }
    });

    const restoreBtn = document.createElement("button");
    restoreBtn.className = "version-restore-btn";
    restoreBtn.textContent = "Restore";
    restoreBtn.addEventListener("click", async () => {
      const ok = await showConfirm(`Restore to version from ${label}? Current unsaved changes will be lost.`, false, "Restore");
      if (!ok) return;
      overlay.remove();
      try {
        const result = await wfManager.restoreVersion(v.id);
        if (result) {
          toast(`Restored to ${label}`, "success");
        } else {
          toast("Restore failed — version not found", "error");
        }
      } catch (e) {
        toast(`Restore failed: ${e}`, "error");
      }
    });

    const deleteBtn = document.createElement("button");
    deleteBtn.className = "version-delete-btn";
    deleteBtn.title = "Delete this version";
    deleteBtn.setAttribute("data-tooltip", "Delete this version");
    deleteBtn.innerHTML = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
    deleteBtn.addEventListener("click", async () => {
      const ok = await showConfirm(`Delete version from ${label}? This cannot be undone.`, true, "Delete");
      if (!ok) return;
      deleteBtn.disabled = true;
      try {
        await wfManager.deleteVersion(v.id);
        versions = versions.filter(x => x.id !== v.id);
        renderList();
      } catch (e) {
        deleteBtn.disabled = false;
        toast(`Delete failed: ${e}`, "error");
      }
    });

    actions.appendChild(compareBtn);
    actions.appendChild(restoreBtn);
    actions.appendChild(deleteBtn);
    item.appendChild(actions);
    return item;
  }

  function applyFilter(raw: string): void {
    const q = raw.trim().toLowerCase();
    const items = Array.from(body.querySelectorAll<HTMLElement>(".version-item"));
    let visible = 0;
    for (const item of items) {
      const match = !q || (item.dataset.search ?? "").includes(q);
      item.classList.toggle("hidden", !match);
      if (match) visible++;
    }
    body.querySelector(".version-filter-empty")?.classList.toggle("hidden", visible !== 0);
  }

  function renderList(): void {
    body.innerHTML = "";
    if (!versions.length) {
      searchRow.classList.add("hidden");
      const empty = document.createElement("div");
      empty.className = "version-empty";
      empty.innerHTML = `No saved versions yet.<br>Each time you save, a snapshot is created here.`;
      body.appendChild(empty);
      return;
    }
    searchRow.classList.remove("hidden");

    const listEl = document.createElement("div");
    listEl.className = "version-list";
    versions.forEach((v, i) => listEl.appendChild(renderItem(v, isCurrent(i))));
    body.appendChild(listEl);

    const filterEmpty = document.createElement("div");
    filterEmpty.className = "version-empty version-filter-empty hidden";
    filterEmpty.textContent = "No versions match your filter.";
    body.appendChild(filterEmpty);

    applyFilter(searchInput.value);
  }

  searchInput.addEventListener("input", () => applyFilter(searchInput.value));

  async function handleSaveSnapshot(): Promise<void> {
    snapshotBtn.disabled = true;
    const snapshotLabel = snapshotBtn.textContent;
    snapshotBtn.textContent = "Saving…";
    try {
      const ok = await wfManager.saveNamedVersion(snapshotInput.value);
      if (!ok) {
        toast("Save the workflow before adding a named snapshot.", "error");
        return;
      }
      const before = versions.length;
      versions = await wfManager.getVersions();
      snapshotInput.value = "";
      renderList();
      toast(
        versions.length > before ? "✓ Snapshot saved" : "No changes since the last snapshot",
        versions.length > before ? "success" : "info",
      );
    } catch (e) {
      toast(`Snapshot failed: ${e}`, "error");
    } finally {
      snapshotBtn.disabled = false;
      snapshotBtn.textContent = snapshotLabel;
    }
  }
  snapshotBtn.addEventListener("click", handleSaveSnapshot);
  snapshotInput.addEventListener("keydown", e => { if (e.key === "Enter") handleSaveSnapshot(); });

  renderList();
}
