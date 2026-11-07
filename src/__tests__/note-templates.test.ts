import { describe, it, expect } from "vitest";
import { NOTE_TEMPLATES } from "../canvas/note-templates";
import { parseNote, MAX_NOTE_TEXT, MAX_FRAME_TITLE } from "../canvas/note-format";

describe("note templates", () => {
  it("fit the text limits and the to-do template parses into unchecked task lines under a heading", () => {
    for (const t of NOTE_TEMPLATES) expect(t.text.length).toBeLessThanOrEqual(t.style === "frame" ? MAX_FRAME_TITLE : MAX_NOTE_TEXT);
    const todo = parseNote(NOTE_TEMPLATES.find(t => t.id === "todo")!.text);
    expect(todo[0].heading).toBe(true);
    expect(todo.slice(1).every(l => l.check && !l.checked)).toBe(true);
    expect(todo.length).toBeGreaterThan(1);
  });
});
