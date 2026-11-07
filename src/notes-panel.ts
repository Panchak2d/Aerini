import type { Canvas } from "./canvas/Canvas";
import type { CanvasNode } from "./canvas/Node";
import type { NoteStyle } from "./canvas/note-geometry";

const REFRESH_MS = 500;
const MAX_LABEL = 80;

export interface AnnotationEntry {
  id: string;
  style: NoteStyle;
  label: string;
  x: number;
  y: number;
}

/** A one-line label for a note or frame: its first non-empty line with the lightweight markup removed. */
export function annotationLabel(style: NoteStyle, text: string): string {
  const first = text.split(/\r?\n/).map(l => l.trim()).find(l => l.length > 0) ?? "";
  const plain = first
    .replace(/^#\s+/, "")
    .replace(/^- \[[ xX]\]\s*/, "")
    .replace(/\[([^\][]+)\]\([^()\s]*\)/g, "$1")
    .replace(/\*\*/g, "")
    .trim();
  if (!plain) return style === "frame" ? "Untitled frame" : "Empty note";
  return plain.length > MAX_LABEL ? `${plain.slice(0, MAX_LABEL - 1)}…` : plain;
}

/** Every note and frame, frames first, each group in reading order (top to bottom, then left to right). Hidden ones are included. */
export function listAnnotations(nodes: Iterable<CanvasNode>): AnnotationEntry[] {
  const out: AnnotationEntry[] = [];
  for (const n of nodes) {
    if (!n.isNote) continue;
    const style = n.noteStyle;
    out.push({ id: n.data.id, style, label: annotationLabel(style, String(n.data.config["text"] ?? "")), x: n.data.position.x, y: n.data.position.y });
  }
  const rank = (e: AnnotationEntry): number => (e.style === "frame" ? 0 : 1);
  return out.sort((a, b) => rank(a) - rank(b) || a.y - b.y || a.x - b.x);
}

function entryKey(entries: AnnotationEntry[], notesVisible: boolean): string {
  return `${notesVisible ? 1 : 0}|${entries.map(e => `${e.id}\u0000${e.style}\u0000${e.label}`).join("\u0001")}`;
}

/**
 * Fills the Notes sidebar zone with the canvas's notes and frames. The list is
 * rebuilt only while the zone is showing, and only when its content changed.
 * Returns a function that stops the refresh timer.
 */
export function initNotesPanel(canvas: Canvas): () => void {
  const zone = document.getElementById("zone-notes");
  const list = document.getElementById("notes-list");
  const hint = document.getElementById("notes-hidden-hint");
  if (!zone || !list) return () => {};

  let lastKey = "";

  const render = (): void => {
    const entries = listAnnotations(canvas.nodes.values());
    const visible = canvas.notesVisible;
    const key = entryKey(entries, visible);
    if (key === lastKey) return;
    lastKey = key;
    hint?.toggleAttribute("hidden", visible || entries.length === 0);

    list.replaceChildren();
    if (!entries.length) {
      const empty = document.createElement("div");
      empty.className = "settings-desc";
      empty.textContent = "No notes or frames yet. Right-click the canvas to add one.";
      list.appendChild(empty);
      return;
    }
    let group: NoteStyle | null = null;
    for (const e of entries) {
      if (e.style !== group) {
        group = e.style;
        const title = document.createElement("div");
        title.className = "notes-list-group";
        title.textContent = e.style === "frame" ? "Frames" : "Notes";
        list.appendChild(title);
      }
      const row = document.createElement("button");
      row.type = "button";
      row.className = "notes-list-item";
      row.dataset["id"] = e.id;
      row.dataset["style"] = e.style;
      row.title = e.label;
      row.textContent = e.label;
      list.appendChild(row);
    }
  };

  list.addEventListener("click", ev => {
    const row = (ev.target as Element | null)?.closest<HTMLElement>(".notes-list-item");
    const node = row?.dataset["id"] ? canvas.nodes.get(row.dataset["id"]) : undefined;
    if (!node) return;
    if (!canvas.notesVisible) canvas.notesVisible = true;
    canvas.revealNode(node);
  });

  document.querySelector('.activity-btn[data-zone="notes"]')?.addEventListener("click", render);

  const timer = window.setInterval(() => {
    if (zone.classList.contains("active") && !document.hidden) render();
  }, REFRESH_MS);

  render();
  return () => window.clearInterval(timer);
}
