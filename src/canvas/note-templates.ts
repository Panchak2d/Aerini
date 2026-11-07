import type { NoteStyle } from "./note-geometry";

export interface NoteTemplate {
  id: string;
  label: string;
  style: NoteStyle;
  text: string;
}

/** Ready-made annotations offered beside the blank note and frame in the Add menu. */
export const NOTE_TEMPLATES: readonly NoteTemplate[] = [
  { id: "section", label: "Section frame", style: "frame", text: "Section" },
  { id: "todo", label: "To-do note", style: "note", text: "# To do\n- [ ] Task\n- [ ] Task\n- [ ] Task" },
];
