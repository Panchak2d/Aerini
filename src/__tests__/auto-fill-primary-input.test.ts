/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { InputHandler } from "../canvas/InputHandler";
import { UndoManager } from "../canvas/UndoManager";
import { CanvasNode } from "../canvas/Node";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

const PI = "x-aerini-primary-input";
const PO = "x-aerini-primary-output";

interface Opts { name?: string; inSchema?: Record<string, unknown>; outSchema?: Record<string, unknown>; outputs?: string[]; x?: number }

function node(id: string, typeId: string, o: Opts = {}): CanvasNode {
  return new CanvasNode({
    id, node_type_id: typeId, node_type: "action", name: o.name ?? id, config: {}, credentials: {},
    position: { x: o.x ?? 0, y: 0 },
    ports: {
      inputs: [{ id: "input", label: "input", position: "left" as const }],
      outputs: (o.outputs ?? ["output"]).map(p => ({ id: p, label: p, position: "right" as const })),
    },
    input_schema: o.inSchema ?? {}, output_schema: o.outSchema ?? {},
    retry: { max_attempts: 1, backoff_ms: 500 }, fallback_node: null, dynamic_ports: false,
  });
}

const aiSource = (name = "Summarize") => node("src", "ai_prompt", {
  name, outSchema: { properties: { content: { type: "string", [PO]: true }, model: { type: "string" } } },
});
const mailTarget = (name = "Send Email") => node("dst", "email_send", {
  name, x: 900, inSchema: { properties: { subject: { type: "string" }, body: { type: "string", [PI]: true } } },
});

function makeCanvas() {
  const canvas = Object.create(Canvas.prototype) as Canvas & Record<string, unknown>;
  canvas.nodes = new Map();
  canvas.connectors = new Map();
  canvas.selectedNodes = new Set();
  canvas.selectedNode = null;
  canvas.selectedConn = null;
  canvas.el = {
    classList: { add: vi.fn(), remove: vi.fn() }, style: { cursor: "" },
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 1400, height: 900 }),
  } as unknown as HTMLCanvasElement;
  canvas.zoom = 1; canvas.panX = 0; canvas.panY = 0;
  canvas.snap = { avoidOverlap: vi.fn(), computeSnap: vi.fn(), snapGuides: [] } as never;
  canvas.onCanvasChanged = vi.fn();
  canvas.onWarn = vi.fn();
  canvas.undoMgr = new UndoManager(canvas);
  canvas.input = new InputHandler(canvas);
  return canvas;
}

const add = (c: Canvas, ...ns: CanvasNode[]) => ns.forEach(n => c.nodes.set(n.data.id, n));
const depth = (c: Canvas) => (c.undoMgr as unknown as { stack: unknown[] }).stack.length;
const warns = (c: Canvas) => (c.onWarn as unknown as ReturnType<typeof vi.fn>).mock.calls.map(a => String(a[0]));

beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(1_000_000); localStorage.clear(); });
afterEach(() => { vi.useRealTimers(); });

describe("connect-time fill of a primary input", () => {
  it("fills_blank_primary_input_and_names_both_nodes_and_the_field", () => {
    const c = makeCanvas();
    const src = aiSource(), dst = mailTarget();
    add(c, src, dst);

    expect(c.connectOrReplace("src", "output", "dst", "input")).toBe(true);

    expect(dst.data.config.body).toBe("{{Summarize.output.content}}");
    const msgs = warns(c);
    expect(msgs).toHaveLength(1);
    expect(msgs[0]).toContain("Send Email");
    expect(msgs[0]).toContain("Summarize");
    expect(msgs[0]).toContain("body");
  });

  it("does_not_overwrite_typed_text", () => {
    const c = makeCanvas();
    const src = aiSource(), dst = mailTarget();
    dst.data.config.body = "Hello team";
    add(c, src, dst);

    c.connectOrReplace("src", "output", "dst", "input");

    expect(dst.data.config.body).toBe("Hello team");
    expect(warns(c)).toHaveLength(0);
  });

  it("does_not_fire_from_the_error_port", () => {
    const c = makeCanvas();
    const src = node("src", "ai_prompt", {
      name: "Summarize", outputs: ["output", "on_error"],
      outSchema: { properties: { content: { type: "string", [PO]: true } } },
    });
    const dst = mailTarget();
    add(c, src, dst);

    c.connectOrReplace("src", "on_error", "dst", "input");

    expect(dst.data.config.body).toBeUndefined();
    expect(warns(c)).toHaveLength(0);
  });

  it("does_nothing_when_the_source_declares_no_primary_output_even_with_one_property", () => {
    const c = makeCanvas();
    const src = node("src", "email_send", { name: "Mailer", outSchema: { properties: { sent: { type: "boolean" } } } });
    const dst = mailTarget();
    add(c, src, dst);

    c.connectOrReplace("src", "output", "dst", "input");

    expect(dst.data.config.body).toBeUndefined();
    expect(warns(c)).toHaveLength(0);
  });

  it("does_nothing_when_the_target_declares_no_primary_input", () => {
    const c = makeCanvas();
    const src = aiSource();
    const dst = node("dst", "email_send", { x: 900, inSchema: { properties: { body: { type: "string" } } } });
    add(c, src, dst);

    c.connectOrReplace("src", "output", "dst", "input");

    expect(dst.data.config.body).toBeUndefined();
  });

  it("undo_reverts_the_wire_and_the_fill_together_in_one_step", () => {
    const c = makeCanvas();
    const src = aiSource(), dst = mailTarget();
    add(c, src, dst);

    c.connectOrReplace("src", "output", "dst", "input");
    expect(depth(c)).toBe(1);

    c.undo();
    expect(c.connectors.size).toBe(0);
    expect(dst.data.config.body ?? "").toBe("");

    c.redo();
    expect(c.connectors.size).toBe(1);
    expect(dst.data.config.body).toBe("{{Summarize.output.content}}");
  });

  it("renaming_the_source_afterwards_updates_the_filled_reference", () => {
    const c = makeCanvas();
    const src = aiSource(), dst = mailTarget();
    add(c, src, dst);
    c.connectOrReplace("src", "output", "dst", "input");

    c.renameNode(src, "Digest Writer");

    expect(dst.data.config.body).toBe("{{Digest Writer.output.content}}");
  });

  it("webhook_chat_fill_wins_on_ai_prompt_and_is_not_filled_twice", () => {
    const c = makeCanvas();
    const hook = node("src", "webhook", {
      name: "Chat Hook", outSchema: { properties: { body: { type: "object", [PO]: true } } },
    });
    const ai = node("dst", "ai_prompt", {
      name: "Ask", x: 900, inSchema: { properties: { prompt: { type: "string", [PI]: true } } },
    });
    add(c, hook, ai);

    c.connectOrReplace("src", "output", "dst", "input");

    expect(ai.data.config.prompt).toBe("{{Chat Hook.output.body.message}}");
    expect(warns(c)).toHaveLength(1);
  });

  it("text_to_file_keeps_legacy_content_fill_for_sources_without_a_declared_output", () => {
    const c = makeCanvas();
    const src = node("src", "file", { name: "Read File", outSchema: { properties: { content: { type: "string" }, path: { type: "string" } } } });
    const dst = node("dst", "text_to_file", { x: 900, inSchema: { properties: { content: { type: "string", [PI]: true } } } });
    add(c, src, dst);

    c.connectOrReplace("src", "output", "dst", "input");

    expect(dst.data.config.content).toBe("{{Read File.output.content}}");
  });

  it("text_to_file_content_is_not_filled_from_an_error_port", () => {
    const c = makeCanvas();
    const src = node("src", "file", { name: "Read File", outputs: ["output", "on_error"], outSchema: { properties: { content: { type: "string" } } } });
    const dst = node("dst", "text_to_file", { x: 900, inSchema: { properties: { content: { type: "string", [PI]: true } } } });
    add(c, src, dst);

    c.connectOrReplace("src", "on_error", "dst", "input");

    expect(dst.data.config.content).toBeUndefined();
  });

  it("text_to_file_uses_the_declared_primary_output_of_the_source", () => {
    const c = makeCanvas();
    const src = node("src", "http_request", { name: "Fetch", outSchema: { properties: { status: { type: "number" }, body: { [PO]: true } } } });
    const dst = node("dst", "text_to_file", { x: 900, inSchema: { properties: { content: { type: "string", [PI]: true } } } });
    add(c, src, dst);

    c.connectOrReplace("src", "output", "dst", "input");

    expect(dst.data.config.content).toBe("{{Fetch.output.body}}");
    expect(warns(c)).toHaveLength(1);
  });
});
