// @vitest-environment jsdom

import { describe, it, expect, vi } from "vitest";
import { parseNote, layoutNote, safeHref, toggleCheckbox, MAX_NOTE_TEXT, MAX_FRAME_TITLE } from "../canvas/note-format";
import { resizeRect, frameHit, handleAt, clampDim, rectContains } from "../canvas/note-geometry";
import { toggleBold, insertLink, toggleChecklist, continueChecklist } from "../note-editing";
import { serialize, deserialize } from "../canvas/CanvasSerializer";
import { CanvasNode } from "../canvas/Node";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level)
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function note(id: string, config: Record<string, unknown>, x = 0, y = 0): CanvasNode {
  return new CanvasNode({
    id, node_type_id: "note", node_type: "utility", name: "Note", config,
    credentials: {}, position: { x, y }, ports: { inputs: [], outputs: [] },
    input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: false,
  });
}

const mono = (text: string, bold: boolean) => text.length * (bold ? 8 : 7);

describe("note-format", () => {
  it("parses bold, links and checkboxes, and leaves everything else as text", () => {
    const [a, b, c] = parseNote("**hi** [docs](https://example.com/a)\n- [x] done\n<b>x</b> [bad](javascript:alert(1))");
    expect(a.spans.map(s => [s.text, s.bold, s.href])).toEqual([
      ["hi", true, null], [" ", false, null], ["docs", false, "https://example.com/a"],
    ]);
    expect([b.check, b.checked]).toEqual([true, true]);
    expect(c.spans.map(s => s.href)).toEqual([null]);
    expect(c.spans[0].text).toBe("<b>x</b> [bad](javascript:alert(1))");
  });

  it("treats an unclosed ** as literal text", () => {
    expect(parseNote("a ** b")[0].spans[0]).toMatchObject({ text: "a ** b", bold: false });
  });

  it("splits Windows line endings without leaving a carriage return in the text", () => {
    const lines = parseNote("one\r\n- [ ] two");
    expect(lines.map(l => l.spans.map(s => s.text).join(""))).toEqual(["one", "two"]);
    expect(lines[1].check).toBe(true);
  });

  it("reads a leading # as a heading and measures it with the heading font", () => {
    const [h, plain] = parseNote("# Title **x**\n#nospace");
    expect(h.heading).toBe(true);
    expect(h.spans.map(sp => sp.text)).toEqual(["Title ", "x"]);
    expect(plain.heading).toBe(false);

    const seen: boolean[] = [];
    const laid = layoutNote(parseNote("# Big\nsmall"), (t, _bold, heading) => { seen.push(heading); return t.length * 7; }, 200);
    expect(laid.map(l => l.heading)).toEqual([true, false]);
    expect(seen).toContain(true);
    expect(seen).toContain(false);
  });

  it("accepts only plain http(s) links without credentials", () => {
    expect(safeHref("https://example.com/x?y=1")).toBe("https://example.com/x?y=1");
    for (const bad of ["javascript:alert(1)", "data:text/html,x", "file:///etc/passwd", "https://good.com@evil.com", "ftp://x.com", "not a url"]) {
      expect(safeHref(bad)).toBeNull();
    }
  });

  it("toggles only the requested checkbox line", () => {
    expect(toggleCheckbox("- [ ] a\n- [x] b", 0)).toBe("- [x] a\n- [x] b");
    expect(toggleCheckbox("- [ ] a\n- [x] b", 1)).toBe("- [ ] a\n- [ ] b");
    expect(toggleCheckbox("plain", 0)).toBe("plain");
    expect(toggleCheckbox("- [ ] a", 5)).toBe("- [ ] a");
  });

  it("wraps long text and hard-breaks a word wider than the line", () => {
    const wrapped = layoutNote(parseNote("aaa bbb ccc"), mono, 40);
    expect(wrapped.map(l => l.runs.map(r => r.text).join(""))).toEqual(["aaa", "bbb", "ccc"]);
    const broken = layoutNote(parseNote("abcdefghij"), mono, 28);
    expect(broken.length).toBeGreaterThan(2);
    expect(broken.every(l => l.runs.every(r => r.w <= 28))).toBe(true);
  });

  it("marks the checkbox only on the first visual line of an item", () => {
    const lines = layoutNote(parseNote("- [ ] one two three four"), mono, 70);
    expect(lines[0].box).toBe(true);
    expect(lines.slice(1).every(l => !l.box && l.indent > 0)).toBe(true);
  });
});

describe("note-geometry", () => {
  const r = { x: 100, y: 100, w: 200, h: 100 };
  const min = { w: 50, h: 40 };

  it("resizes from the east and south edges without moving the origin", () => {
    expect(resizeRect(r, "se", 30, 20, min)).toEqual({ x: 100, y: 100, w: 230, h: 120 });
  });

  it("keeps the far edge fixed when dragging west or north", () => {
    const out = resizeRect(r, "nw", 40, 10, min);
    expect(out.x + out.w).toBe(300);
    expect(out.y + out.h).toBe(200);
  });

  it("stops at the minimum size and anchors the opposite edge", () => {
    const out = resizeRect(r, "w", 500, 0, min);
    expect(out).toMatchObject({ w: 50, x: 250 });
  });

  it("picks corners over edges and ignores the interior", () => {
    expect(handleAt(r, 100, 100, 1)).toBe("nw");
    expect(handleAt(r, 200, 200, 1)).toBe("s");
    expect(handleAt(r, 200, 150, 1)).toBeNull();
  });

  it("lets a frame be grabbed by its title bar and border but not its inside", () => {
    expect(frameHit(r, 150, 110)).toBe(true);
    expect(frameHit(r, 102, 160)).toBe(true);
    expect(frameHit(r, 200, 160)).toBe(false);
    expect(frameHit(r, 500, 500)).toBe(false);
  });

  it("falls back for non-numeric sizes read from a hand-edited file", () => {
    expect(clampDim("wide", 10, 100, 40)).toBe(40);
    expect(clampDim(NaN, 10, 100, 40)).toBe(40);
    expect(clampDim(1e9, 10, 100, 40)).toBe(100);
  });

  it("detects full containment only", () => {
    expect(rectContains(r, { x: 110, y: 110, w: 50, h: 50 })).toBe(true);
    expect(rectContains(r, { x: 250, y: 110, w: 100, h: 50 })).toBe(false);
  });
});

describe("note-editing", () => {
  it("toggles bold around a selection and back", () => {
    const on = toggleBold("say hi now", 4, 6);
    expect(on.value).toBe("say **hi** now");
    const off = toggleBold(on.value, on.start - 2, on.end + 2);
    expect(off.value).toBe("say hi now");
  });

  it("builds a link with the address selected", () => {
    const e = insertLink("see docs", 4, 8);
    expect(e.value).toBe("see [docs](https://)");
    expect(e.value.slice(e.start, e.end)).toBe("https://");
  });

  it("adds and removes checklist markers across selected lines", () => {
    const added = toggleChecklist("a\nb", 0, 3);
    expect(added.value).toBe("- [ ] a\n- [ ] b");
    expect(toggleChecklist(added.value, 0, added.value.length).value).toBe("a\nb");
  });

  it("continues a checklist on Enter and ends it on an empty item", () => {
    const next = continueChecklist("- [ ] a", 7);
    expect(next?.value).toBe("- [ ] a\n- [ ] ");
    expect(continueChecklist("- [ ] ", 6)?.value).toBe("");
    expect(continueChecklist("plain", 5)).toBeNull();
  });
});

describe("annotation persistence", () => {
  const roundTripDoc = () => {
    const sticky = note("n_a", { style: "note", text: "**hi**", color: "blue", width: 300, height: 140 }, 10, 20);
    const frame = note("n_b", { style: "frame", text: "Fetch → Parse", color: "green", width: 500, height: 300 }, -50, -60);
    return JSON.parse(serialize("wf", "WF", new Map([["n_a", sticky], ["n_b", frame]]), new Map()));
  };

  it("round-trips notes and frames through annotations[] without touching nodes[]", () => {
    const doc = roundTripDoc();
    expect(doc.nodes).toEqual([]);
    expect(doc.notes).toBeUndefined();
    expect(doc.annotations).toEqual([
      { id: "n_a", kind: "note", x: 10, y: 20, w: 300, h: 140, text: "**hi**", color: "blue" },
      { id: "n_b", kind: "frame", x: -50, y: -60, w: 500, h: 300, title: "Fetch → Parse", color: "green" },
    ]);

    const back = deserialize(JSON.stringify(doc));
    const f = back.nodes.get("n_b")!;
    expect([f.isNote, f.noteStyle, f.width, f.height, f.data.config["text"]]).toEqual([true, "frame", 500, 300, "Fetch → Parse"]);
    expect(f.data.position).toEqual({ x: -50, y: -60 });
    expect(back.nodes.get("n_a")!.data.config["text"]).toBe("**hi**");
  });

  it("omits annotations when there are none", () => {
    expect(JSON.parse(serialize("wf", "WF", new Map(), new Map())).annotations).toBeUndefined();
  });

  it("sanitises hostile annotation data and skips an unknown kind with a warning", () => {
    const json = JSON.stringify({
      id: "wf", name: "WF", nodes: [], edges: [],
      annotations: [
        { id: "x", kind: "note", x: "a", y: null, w: -5, h: 1e12, text: 42, color: "<script>" },
        { id: "z", kind: "sticker", x: 1, y: 2, text: "from a newer build" },
        "junk", null, [],
      ],
    });
    const back = deserialize(json);
    expect([...back.nodes.keys()]).toEqual(["x"]);
    const n = back.nodes.get("x")!;
    expect([n.noteStyle, n.data.config["color"], n.data.config["text"]]).toEqual(["note", "default", ""]);
    expect(n.data.position).toEqual({ x: 0, y: 0 });
    expect(n.width).toBe(220);
    expect(n.height).toBeLessThanOrEqual(6000);
    expect(back.importWarnings.some(w => w.includes("4 notes or frames could not be read"))).toBe(true);
  });

  it("caps text, title and count in both directions", () => {
    const longText = "x".repeat(MAX_NOTE_TEXT + 50);
    const longTitle = "t".repeat(MAX_FRAME_TITLE + 50);
    const doc = JSON.parse(serialize("wf", "WF", new Map([
      ["a", note("a", { style: "note", text: longText })],
      ["b", note("b", { style: "frame", text: longTitle })],
    ]), new Map()));
    expect(doc.annotations[0].text).toHaveLength(MAX_NOTE_TEXT);
    expect(doc.annotations[1].title).toHaveLength(MAX_FRAME_TITLE);

    const many = Array.from({ length: 520 }, (_, i) => ({ id: `a${i}`, kind: "note", x: i, y: 0, text: longText }));
    const back = deserialize(JSON.stringify({ id: "wf", name: "WF", nodes: [], edges: [], annotations: many }));
    expect(back.nodes.size).toBe(500);
    expect(String(back.nodes.get("a0")!.data.config["text"])).toHaveLength(MAX_NOTE_TEXT);
    expect(back.importWarnings.some(w => w.includes("20 notes or frames"))).toBe(true);
  });

  it("moves a legacy note found inside nodes into an annotation and drops the wires that touched it", () => {
    const json = JSON.stringify({
      id: "wf", name: "WF",
      nodes: [
        { id: "old_note", node_type_id: "note", position: { x: 5, y: 6 }, config: { style: "frame", text: "Group", width: 400, height: 200, color: "green" } },
        { id: "n1", node_type_id: "manual_trigger", position: { x: 0, y: 0 }, config: {} },
      ],
      edges: [{ id: "e1", from_node: "n1", from_port: "output", to_node: "old_note", to_port: "input" }],
    });
    const back = deserialize(json);
    const f = back.nodes.get("old_note")!;
    expect([f.noteStyle, f.width, f.height, f.data.config["text"], f.data.config["color"]]).toEqual(["frame", 400, 200, "Group", "green"]);
    expect(back.connectors.size).toBe(0);
    expect(back.importWarnings.some(w => w.includes("dropped"))).toBe(true);
    expect(JSON.parse(serialize("wf", "WF", back.nodes, back.connectors)).annotations).toHaveLength(1);
  });
});

describe("note nodes", () => {
  it("uses its own width, and a frame only reacts on its header and border", () => {
    const frame = note("f", { style: "frame", width: 400, height: 300 }, 0, 0);
    expect(frame.width).toBe(400);
    expect(frame.containsPoint(200, 10)).toBe(true);
    expect(frame.containsPoint(200, 150)).toBe(false);
  });

  it("never lets a note shrink below its wrapped text", () => {
    const n = note("n", { text: Array(30).fill("word").join("\n"), height: 10 });
    expect(n.height).toBe(n.minHeight);
    expect(n.minHeight).toBeGreaterThan(60);
  });

  it("finds a checkbox under the pointer", () => {
    const n = note("n", { text: "- [ ] task" }, 0, 0);
    expect(n.noteTargetAt(14, 20)).toEqual({ kind: "check", src: 0 });
    expect(n.noteTargetAt(200, 20)).toBeNull();
  });
});
