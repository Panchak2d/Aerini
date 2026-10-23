/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";
import type { OllamaProbe, PullProgress } from "../ipc/providers";

const invokeMock = vi.fn((cmd: string, args?: Record<string, unknown>) => {
  if (cmd === "list_credentials") return Promise.resolve([]);
  if (cmd === "list_provider_models") return Promise.resolve(["llama3.2:latest"]);
  return Promise.resolve(args);
});
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...a: [string, Record<string, unknown>?]) => invokeMock(...a),
  convertFileSrc: (p: string) => p,
}));

const probeMock = vi.fn<(t: unknown) => Promise<OllamaProbe>>();
const pullMock = vi.fn<(id: string, t: unknown, m: string, cb: (p: PullProgress) => void) => Promise<"done" | "cancelled">>();
const cancelMock = vi.fn<(id: string) => Promise<void>>();
vi.mock("../ipc/providers", async () => {
  const actual = await vi.importActual<typeof import("../ipc/providers")>("../ipc/providers");
  return {
    ...actual,
    probeOllama: (t: unknown) => probeMock(t),
    pullOllamaModel: (...a: Parameters<typeof pullMock>) => pullMock(...a),
    cancelOllamaPull: (id: string) => cancelMock(id),
  };
});
vi.mock("../confirm", () => ({ showConfirm: vi.fn(() => Promise.resolve(true)) }));

import { showPopover, closePopover } from "../popover/lifecycle";

const OLLAMA: OllamaProbe = { ollama: true, version: "0.30.0", loopback: true, reason: null };
const SCHEMA = {
  type: "object",
  properties: {
    provider: { type: "string", enum: ["auto", "openai", "local"] },
    model:    { type: "string", "x-aerini-model-picker": true },
    base_url: { type: "string" },
    api_key:  { type: "string" },
  },
};

function makeNode(config: Record<string, unknown>, credentials: Record<string, string> = {}): CanvasNode {
  return new CanvasNode({
    id: "n1",
    node_type_id: "ai_prompt",
    node_type: "ai",
    name: "AI",
    config,
    credentials,
    position: { x: 0, y: 0 },
    ports: { inputs: [], outputs: [{ id: "output", label: "Output", position: "right" }] },
    input_schema: SCHEMA,
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  });
}

const canvasEl = document.createElement("canvas");
const fakeCanvas = { beginNodeEdit: () => {}, commitNodeEdit: () => {} } as unknown as Canvas;

async function openPopover(node: CanvasNode): Promise<void> {
  const p = showPopover(node, canvasEl, () => {}, fakeCanvas);
  await vi.advanceTimersByTimeAsync(600);
  await p;
}

const ctl = () => document.querySelector<HTMLElement>(".ollama-ctl");
const button = (label: string) =>
  Array.from(document.querySelectorAll("button")).find(b => b.textContent === label) as HTMLButtonElement;

describe("popover Ollama control wiring", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
    invokeMock.mockClear();
    probeMock.mockReset().mockResolvedValue(OLLAMA);
    pullMock.mockReset();
    cancelMock.mockReset().mockResolvedValue();
  });
  afterEach(() => {
    closePopover(false);
    vi.useRealTimers();
  });

  it("shows the control for a probed local server, targeting the saved credential when one is selected", async () => {
    await openPopover(makeNode(
      { provider: "local", base_url: "http://localhost:11434/v1", api_key: "one-off", model: "llama3.2" },
      { api_key: "cred1" },
    ));
    expect(ctl()?.hidden).toBe(false);
    expect(probeMock).toHaveBeenCalledWith({ provider: "local", base_url: "http://localhost:11434/v1", credential_id: "cred1" });
  });

  it("sends no probe for a cloud provider, and keeps the control hidden", async () => {
    await openPopover(makeNode({ provider: "openai", base_url: "https://api.openai.com/v1", model: "gpt" }));
    expect(probeMock).not.toHaveBeenCalled();
    expect(ctl()?.hidden).toBe(true);
  });

  it("probes again only when the server settings change, not for unrelated edits", async () => {
    await openPopover(makeNode({ provider: "local", base_url: "http://localhost:11434/v1", model: "llama3.2" }));
    probeMock.mockClear();

    const nameInput = Array.from(document.querySelectorAll<HTMLInputElement>("input")).find(i => i.value === "AI")!;
    nameInput.value = "Renamed";
    nameInput.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(400);
    expect(probeMock).not.toHaveBeenCalled();

    const urlInput = document.querySelector<HTMLInputElement>('input[placeholder^="e.g. http"]')!;
    urlInput.value = "http://localhost:9999/v1";
    urlInput.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(400);
    expect(probeMock).toHaveBeenCalledTimes(1);
    expect(probeMock.mock.calls[0][0]).toMatchObject({ base_url: "http://localhost:9999/v1" });
  });

  it("cancels a running pull when the popover closes", async () => {
    pullMock.mockReturnValue(new Promise(() => {}));
    await openPopover(makeNode({ provider: "local", base_url: "http://localhost:11434/v1", model: "llama3.2" }));
    button("Pull model").click();
    await vi.advanceTimersByTimeAsync(0);
    const pullId = pullMock.mock.calls[0][0];

    closePopover(false);
    expect(cancelMock).toHaveBeenCalledWith(pullId);
  });

  it("lists models again once a pull completes", async () => {
    pullMock.mockResolvedValue("done");
    await openPopover(makeNode({ provider: "local", base_url: "http://localhost:11434/v1", model: "llama3.2" }));
    invokeMock.mockClear();
    button("Pull model").click();
    await vi.advanceTimersByTimeAsync(50);
    expect(invokeMock).toHaveBeenCalledWith("list_provider_models", { provider: "local", baseUrl: "http://localhost:11434/v1", apiKey: "" });
    expect(document.body.textContent).toContain("1 model found");
  });
});
