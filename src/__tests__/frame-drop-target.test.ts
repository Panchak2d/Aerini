/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { innermostContainingFrame } from "../canvas/note-geometry";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

describe("innermostContainingFrame", () => {
  const outer = { id: "outer", rect: { x: 0, y: 0, w: 1000, h: 800 } };
  const inner = { id: "inner", rect: { x: 100, y: 100, w: 400, h: 300 } };

  it("picks the smallest frame that holds every dragged rectangle", () => {
    const one = [{ x: 150, y: 150, w: 220, h: 60 }];
    expect(innermostContainingFrame([outer, inner], one)).toBe("inner");
    const spread = [...one, { x: 700, y: 500, w: 220, h: 60 }];
    expect(innermostContainingFrame([outer, inner], spread)).toBe("outer");
  });

  it("returns null when a dragged rectangle sticks out, or nothing is dragged", () => {
    expect(innermostContainingFrame([inner], [{ x: 450, y: 150, w: 220, h: 60 }])).toBeNull();
    expect(innermostContainingFrame([inner], [])).toBeNull();
  });
});

type Fake = {
  data: { id: string; position: { x: number; y: number } };
  width: number; height: number; isNote: boolean; noteStyle: string; dropTarget: boolean;
};

function fake(id: string, x: number, y: number, w: number, h: number, frame = false): Fake {
  return { data: { id, position: { x, y } }, width: w, height: h, isNote: frame, noteStyle: frame ? "frame" : "note", dropTarget: false };
}

describe("Canvas.setFrameDropTarget", () => {
  function run(nodes: Fake[], dragged: string[] | null, notesVisible = true): void {
    const self = { _nodes: new Map(nodes.map(n => [n.data.id, n])), _notesVisible: notesVisible };
    Canvas.prototype.setFrameDropTarget.call(self as unknown as Canvas, dragged);
  }

  it("marks only the innermost containing frame while dragging and clears it on null", () => {
    const outer = fake("outer", 0, 0, 1000, 800, true);
    const inner = fake("inner", 100, 100, 400, 300, true);
    const node = fake("n", 150, 150, 220, 60);
    run([outer, inner, node], ["n"]);
    expect([outer.dropTarget, inner.dropTarget]).toEqual([false, true]);
    run([outer, inner, node], null);
    expect([outer.dropTarget, inner.dropTarget]).toEqual([false, false]);
  });

  it("never targets a frame that is part of the dragged set, and ignores frames while notes are hidden", () => {
    const outer = fake("outer", 0, 0, 1000, 800, true);
    const inner = fake("inner", 100, 100, 400, 300, true);
    const node = fake("n", 150, 150, 220, 60);
    run([outer, inner, node], ["inner", "n"]);
    expect([outer.dropTarget, inner.dropTarget]).toEqual([true, false]);
    run([outer, inner, node], ["n"], false);
    expect([outer.dropTarget, inner.dropTarget]).toEqual([false, false]);
  });
});
