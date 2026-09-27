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

function makeNode(typeId: string, config: Record<string, unknown>, inputSchema: Record<string, unknown> = {}): CanvasNode {
  return new CanvasNode({
    id: "n1",
    node_type_id: typeId,
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

function register(typeId: string, isPlugin: boolean, properties: Record<string, unknown>): void {
  const desc: NodeDescriptor = {
    type_id: typeId,
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
const fakeCanvas = {} as unknown as Canvas;

async function openPopover(node: CanvasNode): Promise<void> {
  const p = showPopover(node, canvasEl, () => {}, fakeCanvas);
  await vi.advanceTimersByTimeAsync(200);
  await p;
}

const fieldLabels = (): string[] =>
  [...document.querySelectorAll(".popover-body .field-label")].map(l => (l.textContent ?? "").trim());

describe("popover -- custom-UI key exclusion is scoped to built-in nodes", () => {
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

  it("plugin node: properties named like built-in custom-UI keys get a normal field", async () => {
    register("custom_plugin_node", true, {
      files: { type: "string" },
      sources: { type: "string" },
      subfolders: { type: "string" },
      folder_path: { type: "string" },
      attachments: { type: "string" },
      overwrite: { type: "boolean" },
    });
    await openPopover(makeNode("custom_plugin_node", { files: "a", sources: "b", overwrite: true }));

    const labels = fieldLabels();
    for (const name of ["files", "sources", "subfolders", "folder_path", "attachments", "overwrite"]) {
      expect(labels.some(l => l.toLowerCase().replace(/[\s_]/g, "") === name.replace(/_/g, ""))).toBe(true);
    }
  });

  it("built-in collect_files: `sources` still gets no generic field", async () => {
    register("collect_files", false, {
      sources: { type: "array", items: { type: "object" } },
      note: { type: "string" },
    });
    await openPopover(makeNode("collect_files", { sources: [] }));

    const labels = fieldLabels().map(l => l.toLowerCase());
    expect(labels).toContain("note");
    expect(labels).not.toContain("sources");
  });

  it("built-in save_to_folder: keys from an older saved schema stay excluded", async () => {
    const legacySchema = {
      type: "object",
      properties: {
        filename_prefix: { type: "string" },
        folder_path: { type: "string" },
        overwrite: { type: "boolean" },
        subfolders: { type: "array", items: { type: "object" } },
      },
    };
    await openPopover(makeNode("save_to_folder", {}, legacySchema));

    const genericLabels = fieldLabels().map(l => l.toLowerCase());
    expect(genericLabels).toContain("filename prefix");
    expect(genericLabels).not.toContain("folder path");
    expect(genericLabels).not.toContain("subfolders");
  });
});
