import type { Canvas } from "../canvas/Canvas";
import type { CanvasNode } from "../canvas/Node";
import { MAX_NOTE_TEXT, MAX_FRAME_TITLE } from "../canvas/note-format";
import { NOTE_COLORS, NOTE_COLOR_VAR, noteColorLabel, noteColorOf } from "../canvas/note-colors";
import { NOTE_SIZES, NOTE_SIZE_LABEL, NOTE_SIZE_NAME, noteSizeOf } from "../canvas/note-geometry";
import { toggleBold, insertLink, toggleChecklist, continueChecklist, type TextEdit } from "../note-editing";

const MIN_EDITOR_W = 260;

function tool(label: string, title: string, run: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "note-tool-btn";
  b.textContent = label;
  b.title = title;
  b.setAttribute("aria-label", title);
  b.addEventListener("mousedown", e => e.preventDefault());
  b.addEventListener("click", run);
  return b;
}

export function showNoteEditor(node: CanvasNode, canvasEl: HTMLCanvasElement, onChange: () => void): void {
  document.getElementById("note-inline-editor")?.remove();

  const canvas = (canvasEl as unknown as { __canvas?: Canvas }).__canvas;
  if (!canvas) return;

  const frame = node.noteStyle === "frame";
  const startedEmpty = String(node.data.config["text"] ?? "") === "";
  const sh = Math.min(Math.max(node.height * canvas.zoom, 60), 320);

  const wrap = document.createElement("div");
  wrap.id = "note-inline-editor";
  const place = (): void => {
    const r = canvasEl.getBoundingClientRect();
    const sx = node.data.position.x * canvas.zoom + canvas.panX + r.left;
    const sy = node.data.position.y * canvas.zoom + canvas.panY + r.top;
    const sw = Math.max(node.width * canvas.zoom, MIN_EDITOR_W);
    const left = `${Math.max(8, Math.min(sx, window.innerWidth - sw - 8))}px`;
    const top = `${Math.max(8, Math.min(sy, window.innerHeight - 140))}px`;
    const width = `${sw}px`;
    if (wrap.style.left !== left) wrap.style.left = left;
    if (wrap.style.top !== top) wrap.style.top = top;
    if (wrap.style.width !== width) wrap.style.width = width;
  };
  place();

  const field: HTMLTextAreaElement | HTMLInputElement = frame ? document.createElement("input") : document.createElement("textarea");
  field.className = "note-inline-textarea";
  field.value = String(node.data.config["text"] ?? "");
  field.placeholder = frame ? "Group title" : "Write a note…";
  field.maxLength = frame ? MAX_FRAME_TITLE : MAX_NOTE_TEXT;
  field.setAttribute("aria-label", frame ? "Group title" : "Note text");
  if (field instanceof HTMLTextAreaElement) {
    field.style.minHeight = `${sh}px`;
    field.rows = 3;
  } else {
    field.type = "text";
  }

  canvas.beginNodeEdit(node);

  const save = (): void => {
    node.data.config["text"] = field.value;
    canvas.commitNodeEdit(node);
    onChange();
  };
  field.addEventListener("input", save);

  const apply = (edit: TextEdit): void => {
    field.value = edit.value;
    field.setSelectionRange(edit.start, edit.end);
    field.focus();
    save();
  };
  const selection = (): [number, number] => [field.selectionStart ?? field.value.length, field.selectionEnd ?? field.value.length];

  const bar = document.createElement("div");
  bar.className = "note-tool-row";
  if (!frame) {
    bar.append(
      tool("B", "Bold (Ctrl+B)", () => apply(toggleBold(field.value, ...selection()))),
      tool("Link", "Link (Ctrl+K)", () => apply(insertLink(field.value, ...selection()))),
      tool("☑", "Checklist", () => apply(toggleChecklist(field.value, ...selection()))),
    );
  }

  const colorRow = document.createElement("div");
  colorRow.className = "note-color-row";
  const current = noteColorOf(node.data.config);
  for (const c of NOTE_COLORS) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "note-color-btn";
    btn.style.background = NOTE_COLOR_VAR[c];
    btn.title = noteColorLabel(c);
    btn.setAttribute("aria-label", `${btn.title} colour`);
    if (current === c) btn.classList.add("active");
    btn.addEventListener("mousedown", e => e.preventDefault());
    btn.addEventListener("click", () => {
      node.data.config["color"] = c;
      colorRow.querySelectorAll(".note-color-btn").forEach(b => b.classList.remove("active"));
      btn.classList.add("active");
      canvas.commitNodeEdit(node);
      onChange();
    });
    colorRow.appendChild(btn);
  }
  bar.appendChild(colorRow);

  const sizeRow = document.createElement("div");
  sizeRow.className = "note-size-row";
  sizeRow.setAttribute("role", "group");
  sizeRow.setAttribute("aria-label", frame ? "Title size" : "Text size");
  const currentSize = noteSizeOf(node.data.config);
  for (const s of NOTE_SIZES) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "note-size-btn";
    btn.textContent = NOTE_SIZE_LABEL[s];
    btn.title = NOTE_SIZE_NAME[s];
    btn.setAttribute("aria-label", `${btn.title} ${frame ? "title" : "text"}`);
    if (currentSize === s) btn.classList.add("active");
    btn.addEventListener("mousedown", e => e.preventDefault());
    btn.addEventListener("click", () => {
      node.data.config["size"] = s;
      sizeRow.querySelectorAll(".note-size-btn").forEach(b => b.classList.remove("active"));
      btn.classList.add("active");
      canvas.commitNodeEdit(node);
      onChange();
    });
    sizeRow.appendChild(btn);
  }
  bar.appendChild(sizeRow);

  const hint = document.createElement("div");
  hint.className = "note-editor-hint";
  hint.textContent = frame ? "Enter or Esc to finish" : "Esc to finish · **bold** · [text](https://…) · - [ ] task";

  wrap.append(field, bar, hint);
  document.body.appendChild(wrap);
  field.focus();
  field.setSelectionRange(field.value.length, field.value.length);

  let closed = false;
  let raf = 0;
  const follow = (): void => {
    if (closed) return;
    place();
    raf = requestAnimationFrame(follow);
  };
  raf = requestAnimationFrame(follow);

  const close = (): void => {
    if (closed) return;
    closed = true;
    cancelAnimationFrame(raf);
    wrap.remove();
    document.removeEventListener("mousedown", dismiss, true);
    if (!frame && startedEmpty && field.value.trim() === "" && canvas.nodes.get(node.data.id) === node) {
      canvas.clearSelection();
      canvas.selectNode(node);
      canvas.deleteSelected();
    }
  };
  function dismiss(e: MouseEvent): void {
    if (!wrap.contains(e.target as Node)) close();
  }
  document.addEventListener("mousedown", dismiss, true);

  (field as HTMLElement).addEventListener("keydown", (e: KeyboardEvent) => {
    const mod = e.ctrlKey || e.metaKey;
    if (e.key === "Escape" || (frame && e.key === "Enter")) { e.preventDefault(); e.stopPropagation(); close(); return; }
    if (frame) return;
    if (mod && e.key.toLowerCase() === "b") { e.preventDefault(); apply(toggleBold(field.value, ...selection())); return; }
    if (mod && e.key.toLowerCase() === "k") { e.preventDefault(); e.stopPropagation(); apply(insertLink(field.value, ...selection())); return; }
    if (e.key === "Enter" && !e.shiftKey && !mod && !e.isComposing) {
      const [s, en] = selection();
      const edit = s === en ? continueChecklist(field.value, s) : null;
      if (edit) { e.preventDefault(); apply(edit); }
    }
  });
}
