/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";
import { showPopover, closePopover } from "../popover/lifecycle";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level);
// lifecycle.ts's listCredentials()/getCredentialMetadata()/getCredentialSecret()/
// listProviderModels() all go through this same mocked `invoke`. Empty/reset
// by default (matching the original hardcoded behavior below); the provider-
// filtering tests populate these before opening a popover.
let mockCredentials: Array<{ id: string; name: string; cred_type: string }> = [];
let mockMetadata: Record<string, { provider?: string } | null> = {};

const invokeMock = vi.fn((cmd: string, args?: Record<string, unknown>) => {
  if (cmd === "list_credentials") return Promise.resolve(mockCredentials);
  if (cmd === "get_credential_metadata") return Promise.resolve(mockMetadata[(args as { id: string }).id] ?? null);
  if (cmd === "get_credential_secret") return Promise.resolve("secret-from-credential");
  if (cmd === "list_provider_models") return Promise.resolve(["model-a", "model-b"]);
  return Promise.resolve(args);
});
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...a: [string, Record<string, unknown>?]) => invokeMock(...a),
  convertFileSrc: (p: string) => p,
}));

const MODEL_PICKER_SCHEMA = {
  type: "object",
  properties: {
    provider: { type: "string", enum: ["auto", "openai", "anthropic", "gemini", "local"] },
    model:    { type: "string", "x-aerini-model-picker": true },
    base_url: { type: "string" },
    api_key:  { type: "string" },
  },
};

function makeAiNode(id: string, config: Record<string, unknown>, credentials: Record<string, string>): CanvasNode {
  return new CanvasNode({
    id,
    node_type_id: "ai_prompt",
    node_type: "ai",
    name: `Node ${id}`,
    config,
    credentials,
    position: { x: 0, y: 0 },
    ports: { inputs: [], outputs: [{ id: "output", label: "Output", position: "right" }] },
    input_schema: MODEL_PICKER_SCHEMA,
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
  await vi.advanceTimersByTimeAsync(200);
  await p;
}

describe("popover model-picker — api_key resolution precedence", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
    invokeMock.mockClear();
    mockCredentials = [];
    mockMetadata = {};
  });

  afterEach(() => {
    closePopover(false);
    vi.useRealTimers();
  });

  it("normal case: a saved credential is selected — its resolved secret is used, not the one-off inline key", async () => {
    const node = makeAiNode(
      "n1",
      { provider: "openai", base_url: "https://api.example.com", api_key: "one-off-key" },
      { api_key: "cred1" },
    );
    await openPopover(node);

    const fetchBtn = Array.from(document.querySelectorAll("button"))
      .find(b => b.textContent === "Fetch Models") as HTMLButtonElement;
    fetchBtn.click();
    await vi.advanceTimersByTimeAsync(50);

    expect(invokeMock).toHaveBeenCalledWith("get_credential_secret", { id: "cred1" });
    expect(invokeMock).toHaveBeenCalledWith("list_provider_models", {
      provider: "openai", baseUrl: "https://api.example.com", apiKey: "secret-from-credential",
    });
  });

  it("edge case: no saved credential selected — falls back to the one-off inline key in config", async () => {
    const node = makeAiNode(
      "n1",
      { provider: "openai", base_url: "https://api.example.com", api_key: "one-off-key" },
      {},
    );
    await openPopover(node);

    const fetchBtn = Array.from(document.querySelectorAll("button"))
      .find(b => b.textContent === "Fetch Models") as HTMLButtonElement;
    fetchBtn.click();
    await vi.advanceTimersByTimeAsync(50);

    expect(invokeMock).not.toHaveBeenCalledWith("get_credential_secret", expect.anything());
    expect(invokeMock).toHaveBeenCalledWith("list_provider_models", {
      provider: "openai", baseUrl: "https://api.example.com", apiKey: "one-off-key",
    });
  });
});

describe("popover credential picker — provider prefetch (lifecycle.ts -> field-renderer.ts)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
    invokeMock.mockClear();
    mockCredentials = [];
    mockMetadata = {};
  });

  afterEach(() => {
    closePopover(false);
    vi.useRealTimers();
  });

  it("normal case: opening an AI node's popover fetches metadata for every saved credential and the picker reflects only the matching + wildcard ones", async () => {
    mockCredentials = [
      { id: "cred-openai",    name: "Prod OpenAI",  cred_type: "api_key" },
      { id: "cred-anthropic", name: "Prod Claude",  cred_type: "api_key" },
      { id: "cred-unlabeled", name: "Old shared key", cred_type: "api_key" },
    ];
    mockMetadata = {
      "cred-openai":    { provider: "openai" },
      "cred-anthropic": { provider: "anthropic" },
      // "cred-unlabeled" intentionally has no entry -> get_credential_metadata resolves null
    };
    const node = makeAiNode("n1", { provider: "anthropic" }, {});
    await openPopover(node);

    expect(invokeMock).toHaveBeenCalledWith("get_credential_metadata", { id: "cred-openai" });
    expect(invokeMock).toHaveBeenCalledWith("get_credential_metadata", { id: "cred-anthropic" });
    expect(invokeMock).toHaveBeenCalledWith("get_credential_metadata", { id: "cred-unlabeled" });

    const options = Array.from(document.querySelectorAll(".csel-option")).map(el => el.textContent);
    expect(options).toContain("Prod Claude — Anthropic");
    expect(options).toContain("Old shared key");
    expect(options).not.toContain("Prod OpenAI — Openai");
  });

  it("edge case: a non-AI node never calls get_credential_metadata, even with saved credentials present", async () => {
    mockCredentials = [{ id: "cred-1", name: "Some token", cred_type: "api_key" }];
    const node = new CanvasNode({
      id: "n1", node_type_id: "send_email", node_type: "action", name: "Node n1",
      config: {}, credentials: {},
      position: { x: 0, y: 0 },
      ports: { inputs: [], outputs: [{ id: "output", label: "Output", position: "right" }] },
      input_schema: { type: "object", properties: { password: { type: "string" } } },
      output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 },
      fallback_node: null, dynamic_ports: false,
    });
    await openPopover(node);

    expect(invokeMock).not.toHaveBeenCalledWith("get_credential_metadata", expect.anything());
  });
});
