import { escapeHtml as escHtml } from "../utils";
import { diffWorkflowJson, type FieldChange, type DiffStatus } from "../canvas/WorkflowDiff";

function fmtValue(v: unknown): string {
  if (v === undefined) return "(none)";
  if (v === null) return "null";
  if (typeof v === "string") return v === "" ? "(empty)" : v;
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

function renderFieldRow(c: FieldChange): HTMLElement {
  const row = document.createElement("div");
  row.className = "diff-field-row";
  row.innerHTML = `
    <span class="diff-field-label">${escHtml(c.field)}</span>
    <span class="diff-field-old">${escHtml(fmtValue(c.from))}</span>
    <span class="diff-field-arrow">→</span>
    <span class="diff-field-new">${escHtml(fmtValue(c.to))}</span>`;
  return row;
}

const STATUS_LABEL: Record<DiffStatus, string> = { added: "Added", removed: "Removed", changed: "Changed" };

function renderEntry(title: string, status: DiffStatus, changes: FieldChange[]): HTMLElement {
  const entry = document.createElement("div");
  entry.className = `diff-entry diff-status-${status}`;
  const header = document.createElement("div");
  header.className = "diff-entry-header";
  header.innerHTML = `
    <span class="diff-status-badge">${STATUS_LABEL[status]}</span>
    <span class="diff-entry-title" title="${escHtml(title)}">${escHtml(title)}</span>`;
  entry.appendChild(header);
  if (changes.length) {
    const rows = document.createElement("div");
    rows.className = "diff-entry-fields";
    for (const c of changes) rows.appendChild(renderFieldRow(c));
    entry.appendChild(rows);
  }
  return entry;
}

function renderSection(title: string, entries: HTMLElement[]): HTMLElement | null {
  if (!entries.length) return null;
  const section = document.createElement("div");
  section.className = "diff-section";
  const h = document.createElement("div");
  h.className = "diff-section-title";
  h.textContent = title;
  section.appendChild(h);
  for (const e of entries) section.appendChild(e);
  return section;
}

/** Shows a diff overlay comparing two workflow JSON snapshots. Stacks above
 *  whatever panel launched it rather than replacing it. */
export function showDiffPanel(labelA: string, jsonA: string, labelB: string, jsonB: string): void {
  document.getElementById("diff-panel-overlay")?.remove();

  const overlay = document.createElement("div");
  overlay.id = "diff-panel-overlay";
  overlay.className = "diff-overlay";

  const panel = document.createElement("div");
  panel.className = "diff-panel";

  const hdr = document.createElement("div");
  hdr.className = "diff-panel-header";
  hdr.innerHTML = `<span class="diff-panel-title" title="${escHtml(labelA)} → ${escHtml(labelB)}">${escHtml(labelA)} → ${escHtml(labelB)}</span>`;
  const closeBtn = document.createElement("button");
  closeBtn.className = "popover-close";
  closeBtn.innerHTML = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
  closeBtn.addEventListener("click", () => overlay.remove());
  hdr.appendChild(closeBtn);
  panel.appendChild(hdr);

  const body = document.createElement("div");
  body.className = "diff-panel-body";
  panel.appendChild(body);
  overlay.appendChild(panel);
  document.body.appendChild(overlay);
  overlay.addEventListener("click", e => { if (e.target === overlay) overlay.remove(); });

  try {
    const result = diffWorkflowJson(jsonA, jsonB);

    if (result.isEmpty) {
      body.innerHTML = `<div class="diff-empty">No differences — these versions are identical.</div>`;
      return;
    }

    if (result.metadata.length) {
      const metaSection = document.createElement("div");
      metaSection.className = "diff-section";
      const h = document.createElement("div");
      h.className = "diff-section-title";
      h.textContent = "Workflow settings";
      metaSection.appendChild(h);
      const rows = document.createElement("div");
      rows.className = "diff-entry-fields";
      for (const c of result.metadata) rows.appendChild(renderFieldRow(c));
      metaSection.appendChild(rows);
      body.appendChild(metaSection);
    }

    const nodeSection = renderSection(
      `Nodes (${result.nodes.length})`,
      result.nodes.map(n => renderEntry(n.name, n.status, n.changes)),
    );
    if (nodeSection) body.appendChild(nodeSection);

    const edgeSection = renderSection(
      `Connections (${result.edges.length})`,
      result.edges.map(e => renderEntry(`${e.from_node} → ${e.to_node}`, e.status, e.changes)),
    );
    if (edgeSection) body.appendChild(edgeSection);
  } catch (e) {
    body.innerHTML = `<div class="diff-empty">Could not compare versions: ${escHtml(String(e))}</div>`;
  }
}
