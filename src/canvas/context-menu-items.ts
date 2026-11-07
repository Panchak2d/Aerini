import type { Canvas } from "./Canvas";
import type { CanvasNode } from "./Node";
import type { Connector } from "./Connector";
import { FRAME_DEFAULT_H, FRAME_DEFAULT_W, NOTE_SIZES, NOTE_SIZE_LABEL, NOTE_SIZE_NAME, noteSizeOf } from "./note-geometry";
import { NOTE_COLORS, NOTE_COLOR_VAR, noteColorLabel, noteColorOf } from "./note-colors";
import { gridSnapEnabled, snapToGrid } from "./grid";
import { hasClipboard } from "./node-clipboard";
import { NOTE_TEMPLATES } from "./note-templates";

export interface MenuSwatch { id: string; label: string; css: string }

export interface MenuChoice { id: string; label: string; title: string }

export interface MenuApi {
  item(label: string, icon: string, danger: boolean, action: () => void): void;
  sep(): void;
  swatches(label: string, options: readonly MenuSwatch[], current: string, pick: (id: string) => void): void;
  choices(label: string, options: readonly MenuChoice[], current: string, pick: (id: string) => void): void;
}

export interface MenuHooks {
  rename(node: CanvasNode): void;
}

const svg = (inner: string): string =>
  `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${inner}</svg>`;
const ICON_COPY   = svg('<rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>');
const ICON_RENAME = svg('<path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/>');
const ICON_CHECK  = svg('<polyline points="20 6 9 17 4 12"/>');
const ICON_X      = svg('<line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>');
const ICON_PLAY   = svg('<polygon points="5 3 19 12 5 21 5 3"/>');
const ICON_TRASH  = svg('<polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/><path d="M9 6V4h6v2"/>');
const ICON_NOTE   = svg('<path d="M15.5 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V8.5z"/><path d="M15 3v6h6"/>');
const ICON_FRAME  = svg('<rect x="3" y="4" width="18" height="16" rx="2" stroke-dasharray="3 2"/><line x1="3" y1="9" x2="21" y2="9"/>');
const ICON_EYE    = svg('<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>');
const ICON_PLUS   = svg('<line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>');
const ICON_SELECT = svg('<rect x="3" y="3" width="18" height="18" rx="2" stroke-dasharray="3 2"/><polyline points="8 12 11 15 16 9"/>');
const ICON_FIT    = svg('<path d="M4 9V4h5"/><path d="M20 9V4h-5"/><path d="M4 15v5h5"/><path d="M20 15v5h-5"/>');
const ICON_SCISSORS = svg('<circle cx="6" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><line x1="20" y1="4" x2="8.1" y2="15.9"/><line x1="14.5" y1="14.5" x2="20" y2="20"/><line x1="8.1" y1="8.1" x2="12" y2="12"/>');

const SIZE_CHOICES: readonly MenuChoice[] = NOTE_SIZES.map(id => ({ id, label: NOTE_SIZE_LABEL[id], title: NOTE_SIZE_NAME[id] }));

const COLOR_SWATCHES: readonly MenuSwatch[] = NOTE_COLORS.map(id => ({ id, label: noteColorLabel(id), css: NOTE_COLOR_VAR[id] }));

/** Canvas-relative pixels for a world point, as the palette's place-at-point mode expects. */
function toScreen(c: Canvas, world: { x: number; y: number }): { sx: number; sy: number } {
  return { sx: world.x * c.zoom + c.panX, sy: world.y * c.zoom + c.panY };
}

function snapWorld(world: { x: number; y: number }): { x: number; y: number } {
  return gridSnapEnabled() ? { x: snapToGrid(world.x), y: snapToGrid(world.y) } : world;
}

export function buildCanvasMenu(m: MenuApi, c: Canvas, world: { x: number; y: number }): void {
  m.item("Add node here…", ICON_PLUS, false, () => c.onPaletteRequest?.(toScreen(c, world)));
  m.item("Add sticky note here", ICON_NOTE, false, () => c.addNote("note", snapWorld(world)));
  m.item("Add group frame here", ICON_FRAME, false, () => c.addNote("frame", snapWorld(world)));
  for (const t of NOTE_TEMPLATES) {
    m.item(`Add ${t.label.toLowerCase()} here`, t.style === "frame" ? ICON_FRAME : ICON_NOTE, false, () => c.addNote(t.style, snapWorld(world), t));
  }
  m.sep();
  if (hasClipboard()) m.item("Paste", ICON_COPY, false, () => { c.pasteClipboard(world); });
  if (c.hasShownNodes()) {
    m.item("Select all", ICON_SELECT, false, () => c.selectAll());
    m.item("Fit to screen", ICON_FIT, false, () => c.fitToScreen());
  }
  m.item(c.notesVisible ? "Hide notes" : "Show notes", ICON_EYE, false, () => { c.notesVisible = !c.notesVisible; });
}

export function buildWireMenu(m: MenuApi, c: Canvas, conn: Connector, world: { x: number; y: number }): void {
  m.item("Insert node here…", ICON_PLUS, false, () => c.onInsertOnWireRequest?.(conn, toScreen(c, world)));
  m.sep();
  m.item("Delete connection", ICON_SCISSORS, true, () => c.deleteConnector(conn));
}

function buildAnnotationMenu(m: MenuApi, c: Canvas, node: CanvasNode): void {
  const frame = node.noteStyle === "frame";
  m.item(frame ? "Rename" : "Edit text", ICON_RENAME, false, () => c.openNodeConfig(node));
  m.swatches("Colour", COLOR_SWATCHES, noteColorOf(node.data.config), (id) => {
    c.editNode(node, () => { node.data.config["color"] = id; });
  });
  m.choices(frame ? "Title size" : "Text size", SIZE_CHOICES, noteSizeOf(node.data.config), (id) => {
    c.editNode(node, () => { node.data.config["size"] = id; });
  });
  if (frame && c.containedIds(node).length) {
    m.item("Fit to contents", ICON_FIT, false, () => { c.fitFrameToContents(node); });
  }
  m.item(frame ? "Convert to sticky note" : "Convert to group frame", frame ? ICON_NOTE : ICON_FRAME, false, () => {
    const height = node.height;
    c.editNode(node, () => {
      if (frame) { node.data.config["style"] = "note"; delete node.data.config["height"]; }
      else {
        node.data.config["style"] = "frame";
        node.data.config["height"] = Math.max(height, FRAME_DEFAULT_H);
        node.data.config["width"] = Math.max(node.width, FRAME_DEFAULT_W);
      }
    });
  });
  m.sep();
  m.item("Duplicate", ICON_COPY, false, () => { c.dupSelected(); c.onCanvasChanged?.(); });
  m.item("Delete", ICON_TRASH, true, () => { c.deleteSelected(); });
}

function buildNodeMenu(m: MenuApi, c: Canvas, node: CanvasNode, hooks: MenuHooks): void {
  const count = c.selectedNodes.size;
  const multi = count > 1;
  const hasRealNode = [...c.selectedNodes].some(id => { const n = c.nodes.get(id); return !!n && !n.isNote; });

  m.item(multi ? `Duplicate (${count})` : "Duplicate", ICON_COPY, false, () => { c.dupSelected(); c.onCanvasChanged?.(); });
  if (hasRealNode) m.item("Group in frame", ICON_FRAME, false, () => { c.frameSelection(); });

  if (!node.isNote) {
    if (!multi) m.item("Rename", ICON_RENAME, false, () => hooks.rename(node));
    m.item(
      node.disabled ? "Enable" : "Disable",
      node.disabled ? ICON_CHECK : ICON_X,
      false,
      () => {
        const ids = multi ? [...c.selectedNodes] : [node.data.id];
        const targets = ids.map(id => c.nodes.get(id)).filter((n): n is CanvasNode => !!n && !n.isNote);
        const verb = node.disabled ? "Enable" : "Disable";
        c.transact(targets.length > 1 ? `${verb} ${targets.length} nodes` : `${verb} "${node.data.name}"`, () => {
          for (const n of targets) c.editNode(n, () => { n.disabled = !n.disabled; });
        });
      },
    );
    if (!multi) m.item("Run from here", ICON_PLAY, false, () => { c.onRunNode?.(node.data.id); });
  }

  m.sep();
  m.item(multi ? `Delete (${count})` : "Delete", ICON_TRASH, true, () => { c.deleteSelected(); });
}

/** The menu for the current selection, anchored on `node`: annotation rows when exactly one note or frame is selected, node rows otherwise. */
export function buildSelectionMenu(m: MenuApi, c: Canvas, node: CanvasNode, hooks: MenuHooks): void {
  if (node.isNote && c.selectedNodes.size === 1) buildAnnotationMenu(m, c, node);
  else buildNodeMenu(m, c, node, hooks);
}
