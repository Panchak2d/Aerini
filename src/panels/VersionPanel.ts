import type { WorkflowManager } from "../workflow-manager";
import { showConfirm } from "../confirm";
import { escapeHtml as escHtml } from "../utils";

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
  hdr.innerHTML = `<span class="version-panel-title">Version History</span>`;
  const closeBtn = document.createElement("button");
  closeBtn.className = "popover-close";
  closeBtn.innerHTML = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
  closeBtn.addEventListener("click", () => overlay.remove());
  hdr.appendChild(closeBtn);
  panel.appendChild(hdr);

  const body = document.createElement("div");
  body.className = "version-panel-body";
  body.innerHTML = `<div class="version-empty">Loading…</div>`;
  panel.appendChild(body);
  overlay.appendChild(panel);
  document.body.appendChild(overlay);
  overlay.addEventListener("click", e => { if (e.target === overlay) overlay.remove(); });

  const versions = await wfManager.getVersions();

  body.innerHTML = "";
  if (!versions.length) {
    body.innerHTML = `<div class="version-empty">No saved versions yet.<br>Each time you save, a snapshot is created here.</div>`;
    return;
  }

  for (const v of versions) {
    const item = document.createElement("div");
    item.className = "version-item";
    const dt    = new Date(v.created_at);
    const label = dt.toLocaleDateString([], { month: "short", day: "numeric" })
      + " " + dt.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    const displayMsg = v.message ?? "Saved";
    item.innerHTML = `
      <div class="version-item-meta">
        <span class="version-item-name">${escHtml(displayMsg)}</span>
        <span class="version-item-date">${label}</span>
      </div>`;

    const actions = document.createElement("div");
    actions.className = "version-item-actions";

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
    deleteBtn.innerHTML = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
    deleteBtn.addEventListener("click", async () => {
      const ok = await showConfirm(`Delete version from ${label}? This cannot be undone.`, true, "Delete");
      if (!ok) return;
      try {
        await wfManager.deleteVersion(v.id);
        item.remove();
        if (!body.querySelector(".version-item")) {
          body.innerHTML = `<div class="version-empty">No saved versions yet.<br>Each time you save, a snapshot is created here.</div>`;
        }
      } catch (e) {
        toast(`Delete failed: ${e}`, "error");
      }
    });

    actions.appendChild(restoreBtn);
    actions.appendChild(deleteBtn);
    item.appendChild(actions);
    body.appendChild(item);
  }
}
