// @vitest-environment jsdom

import { describe, it, expect, vi } from "vitest";
import { NOTE_SIZES, noteMetrics, noteSizeOf } from "../canvas/note-geometry";
import { layoutNote, parseNote } from "../canvas/note-format";
import { serialize, deserialize } from "../canvas/CanvasSerializer";
import { CanvasNode } from "../canvas/Node";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function note(id: string, config: Record<string, unknown>): CanvasNode {
  return new CanvasNode({
    id, node_type_id: "note", node_type: "utility", name: "Note", config,
    credentials: {}, position: { x: 0, y: 0 }, ports: { inputs: [], outputs: [] },
    input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: false,
  });
}

describe("note text size", () => {
  it("medium keeps the original fixed metrics and unknown values read as medium", () => {
    expect(noteMetrics("m")).toMatchObject({ body: 12, heading: 14, line: 18, box: 12, indent: 20, title: 13 });
    expect(noteSizeOf({ size: "huge" })).toBe("m");
    expect(noteSizeOf({})).toBe("m");
    expect(NOTE_SIZES.map(s => noteSizeOf({ size: s }))).toEqual([...NOTE_SIZES]);
  });

  it("a frame title never outgrows the fixed title band", () => {
    for (const s of NOTE_SIZES) expect(noteMetrics(s).title).toBeLessThanOrEqual(20);
  });

  it("a larger size makes a note taller and wraps a checkbox line with the larger indent", () => {
    const text = "one\ntwo\n- [ ] three";
    const heights = NOTE_SIZES.map(s => note("n", { style: "note", text, size: s }).minHeight);
    expect([...heights].sort((a, b) => a - b)).toEqual(heights);
    expect(heights[3]).toBeGreaterThan(heights[1]);
    const lines = layoutNote(parseNote("- [ ] x"), (t) => t.length * 7, 200, noteMetrics("xl").indent);
    expect(lines[0].indent).toBe(36);
  });

  it("round-trips through a saved workflow, omitting the default size", () => {
    const sized = note("a", { style: "note", text: "hi", size: "xl" });
    const plain = note("b", { style: "frame", text: "T", size: "m" });
    const doc = JSON.parse(serialize("wf", "WF", new Map([["a", sized], ["b", plain]]), new Map()));
    expect(doc.annotations.find((x: { id: string }) => x.id === "a").size).toBe("xl");
    expect(doc.annotations.find((x: { id: string }) => x.id === "b")).not.toHaveProperty("size");
    const back = deserialize(JSON.stringify(doc));
    expect(back.nodes.get("a")?.data.config["size"]).toBe("xl");
    expect(back.nodes.get("b")?.data.config["size"]).toBeUndefined();
  });

  it("ignores an invalid size in a file", () => {
    const back = deserialize(JSON.stringify({ schema_version: "1.0", id: "w", name: "W", nodes: [], edges: [], annotations: [{ id: "a", x: 0, y: 0, text: "t", size: "enormous" }] }));
    expect([...back.nodes.values()][0].data.config["size"]).toBeUndefined();
  });
});
