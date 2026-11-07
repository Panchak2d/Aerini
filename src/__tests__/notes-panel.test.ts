// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import type { CanvasNode } from "../canvas/Node";
import { annotationLabel, listAnnotations, initNotesPanel } from "../notes-panel";

function note(id: string, style: "note" | "frame", text: string, x = 0, y = 0, isNote = true): CanvasNode {
  return { isNote, noteStyle: style, data: { id, config: { text }, position: { x, y } } } as unknown as CanvasNode;
}

describe("annotationLabel", () => {
  it("uses the first non-empty line without heading, task or emphasis markup", () => {
    expect(annotationLabel("note", "\n# **Plan** for [docs](https://example.com)\nmore")).toBe("Plan for docs");
    expect(annotationLabel("note", "- [x] ship it")).toBe("ship it");
  });

  it("falls back per kind when empty and shortens long text", () => {
    expect(annotationLabel("frame", "  ")).toBe("Untitled frame");
    expect(annotationLabel("note", "")).toBe("Empty note");
    expect(annotationLabel("note", "a".repeat(200))).toHaveLength(80);
  });
});

describe("listAnnotations", () => {
  it("skips real nodes and lists frames first, each group top to bottom then left to right", () => {
    const out = listAnnotations([
      note("n2", "note", "b", 50, 100), note("n1", "note", "a", 200, 10), note("real", "note", "x", 0, 0, false),
      note("f1", "frame", "F", 500, 500), note("n3", "note", "c", 10, 100),
    ]);
    expect(out.map(e => e.id)).toEqual(["f1", "n1", "n3", "n2"]);
  });
});

describe("notes panel", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = `
      <div id="zone-notes" class="sidebar-zone active">
        <div id="notes-hidden-hint" hidden></div><div id="notes-list"></div>
      </div>`;
  });
  afterEach(() => { vi.useRealTimers(); });

  function fakeCanvas(nodes: CanvasNode[], notesVisible = true) {
    return {
      nodes: new Map(nodes.map(n => [n.data.id, n])),
      notesVisible,
      revealNode: vi.fn(),
    };
  }

  it("lists hidden notes, shows them and centres the clicked one", () => {
    const canvas = fakeCanvas([note("a", "note", "Alpha"), note("f", "frame", "Zone")], false);
    initNotesPanel(canvas as unknown as Canvas);

    const rows = [...document.querySelectorAll<HTMLElement>(".notes-list-item")];
    expect(rows.map(r => r.textContent)).toEqual(["Zone", "Alpha"]);
    expect(document.getElementById("notes-hidden-hint")!.hasAttribute("hidden")).toBe(false);

    rows[1].click();
    expect(canvas.notesVisible).toBe(true);
    expect(canvas.revealNode).toHaveBeenCalledWith(canvas.nodes.get("a"));
  });

  it("rebuilds when a note's text changes and shows an empty state with none", () => {
    const a = note("a", "note", "Alpha");
    const canvas = fakeCanvas([a]);
    const stop = initNotesPanel(canvas as unknown as Canvas);

    (a.data.config as Record<string, unknown>)["text"] = "Beta";
    vi.advanceTimersByTime(600);
    expect(document.querySelector(".notes-list-item")!.textContent).toBe("Beta");

    canvas.nodes.clear();
    vi.advanceTimersByTime(600);
    expect(document.querySelector(".notes-list-item")).toBeNull();
    expect(document.getElementById("notes-list")!.textContent).toContain("No notes or frames yet");
    stop();
  });
});
