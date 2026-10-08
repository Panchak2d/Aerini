/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";
import { showPopover, closePopover } from "../popover/lifecycle";
import { registerNodeDescriptors } from "../canvas/node-registry";
import type { NodeDescriptor } from "../ipc/workflow";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeNode(config: Record<string, unknown>, inputSchema: Record<string, unknown> = {}): CanvasNode {
  return new CanvasNode({
    id: "n1",
    node_type_id: "custom_plugin_node",
    node_type: "action",
    name: "Node n1",
    config,
    credentials: {},
    position: { x: 0, y: 0 },
    ports: { inputs: [], outputs: [{ id: "output", label: "Output", position: "right" }] },
    input_schema: inputSchema,
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  });
}

function registerPlugin(properties: Record<string, unknown>, isPlugin = true): void {
  const desc: NodeDescriptor = {
    type_id: "custom_plugin_node",
    display_name: "Test Node",
    node_type: "action",
    version: "1.0",
    input_schema: { type: "object", properties },
    output_schema: {},
    ports: { inputs: [], outputs: [] },
    is_plugin: isPlugin,
  };
  registerNodeDescriptors([desc]);
}

const canvasEl = document.createElement("canvas");
const fakeCanvas = { beginNodeEdit: () => {}, commitNodeEdit: () => {} } as unknown as Canvas;

async function openPopover(node: CanvasNode): Promise<void> {
  const p = showPopover(node, canvasEl, () => {}, fakeCanvas);
  await vi.advanceTimersByTimeAsync(200);
  await p;
}

const sectionTitles = (): string[] =>
  [...document.querySelectorAll(".popover-section-title")].map(e => e.textContent ?? "");

describe("popover -- plugin JSON config editor", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });

  afterEach(() => {
    closePopover(false);
    registerNodeDescriptors([]);
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("object property: edited as JSON (credential fields excluded), no text input that would overwrite it", async () => {
    registerPlugin({
      greeting: { type: "string" },
      settings: { type: "object" },
      api_key: { type: "string" },
    });
    const config = { greeting: "hi", settings: { a: { b: 1 } }, api_key: "sk-inline" };
    await openPopover(makeNode(config));

    const labels = [...document.querySelectorAll(".popover-body .field-label")].map(l => l.textContent);
    expect(labels).toContain("Greeting");
    expect(labels).not.toContain("Settings");
    expect(sectionTitles()).toContain("Advanced: full config (JSON)");

    const ta = document.querySelector<HTMLTextAreaElement>("textarea.code-editor")!;
    expect(JSON.parse(ta.value)).toEqual({ greeting: "hi", settings: { a: { b: 1 } } });
    expect(ta.value).not.toContain("sk-inline");
    expect([...document.querySelectorAll("input")].some(i => i.value === "[object Object]")).toBe(false);
  });

  it("flat schema: no JSON editor", async () => {
    registerPlugin({ greeting: { type: "string" }, count: { type: "number" } });
    await openPopover(makeNode({ greeting: "hi" }));

    expect(document.querySelector("textarea.code-editor")).toBeNull();
    expect(sectionTitles().some(t => t.includes("JSON"))).toBe(false);
  });

  it("only unrenderable properties: the JSON editor sits under a plain Configuration heading", async () => {
    registerPlugin({ items: { type: "array", items: { type: "object" } } });
    await openPopover(makeNode({ items: [{ id: 1 }] }));

    expect(sectionTitles()).toContain("Configuration");
    expect(sectionTitles()).not.toContain("Advanced: full config (JSON)");
    const ta = document.querySelector<HTMLTextAreaElement>("textarea.code-editor")!;
    expect(JSON.parse(ta.value)).toEqual({ items: [{ id: 1 }] });
  });

  it("plugin no longer installed (no descriptor): saved-schema object property is edited as JSON, not overwritten by a text field", async () => {
    registerNodeDescriptors([]);
    const savedSchema = { type: "object", properties: { greeting: { type: "string" }, settings: { type: "object" } } };
    await openPopover(makeNode({ greeting: "hi", settings: { a: 1 } }, savedSchema));

    const labels = [...document.querySelectorAll(".popover-body .field-label")].map(l => l.textContent);
    expect(labels).toContain("Greeting");
    expect(labels).not.toContain("Settings");
    const ta = document.querySelector<HTMLTextAreaElement>("textarea.code-editor")!;
    expect(JSON.parse(ta.value)).toEqual({ greeting: "hi", settings: { a: 1 } });
    expect([...document.querySelectorAll("input")].some(i => i.value === "[object Object]")).toBe(false);
  });

  it("stored object/array that its property's control would corrupt: edited as JSON", async () => {
    registerPlugin({
      files: { type: "array" },
      note: { type: "string" },
      tags: { type: "array", items: { type: "string" } },
    });
    const config = { files: [{ filename: "a.png" }], note: { nested: true }, tags: ["x", "y"] };
    await openPopover(makeNode(config));

    const labels = [...document.querySelectorAll(".popover-body .field-label")].map(l => l.textContent);
    expect(labels).toEqual(expect.arrayContaining(["Tags"]));
    expect(labels).not.toContain("Files");
    expect(labels).not.toContain("Note");
    const ta = document.querySelector<HTMLTextAreaElement>("textarea.code-editor")!;
    expect(JSON.parse(ta.value)).toEqual(config);
  });

  it("values that fit their controls get no JSON editor, whether plugin or registered built-in", async () => {
    registerPlugin({ files: { type: "array" }, note: { type: "string" } });
    await openPopover(makeNode({ files: ["a.png"], note: "hi" }));
    expect(document.querySelector("textarea.code-editor")).toBeNull();

    closePopover(false);
    registerPlugin({ note: { type: "string" } }, false);
    await openPopover(makeNode({ note: "hi" }));
    expect(document.querySelector("textarea.code-editor")).toBeNull();
  });

  it("registered built-in: an object-typed field (e.g. HTTP Request's headers) is edited as JSON, not overwritten by a text field", async () => {
    registerPlugin({ url: { type: "string" }, headers: { type: "object" } }, false);
    const config = { url: "https://example.com", headers: { "content-type": "application/json" } };
    await openPopover(makeNode(config));

    const labels = [...document.querySelectorAll(".popover-body .field-label")].map(l => l.textContent);
    expect(labels).toContain("URL");
    expect(labels).not.toContain("Headers");
    const ta = document.querySelector<HTMLTextAreaElement>("textarea.code-editor")!;
    expect(JSON.parse(ta.value)).toEqual(config);
    expect([...document.querySelectorAll("input")].some(i => i.value === "[object Object]")).toBe(false);
  });
});
