/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { CanvasNode } from "../canvas/Node";
import { renderConfigFieldsLoop, renderCredentialSection, getCredentialFieldKeys, type PropSchema } from "../popover/field-renderer";
import type { ExtensionContext } from "../node-configs/popover-utils";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level)
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeNode(id: string): CanvasNode {
  return new CanvasNode({
    id,
    node_type_id: "save_to_folder",
    node_type: "action",
    name: `Node ${id}`,
    config: {},
    credentials: {},
    position: { x: 0, y: 0 },
    ports: {
      inputs:  [{ id: "input",  label: "Input",  position: "left"  }],
      outputs: [{ id: "output", label: "Output", position: "right" }],
    },
    input_schema: {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  });
}

function makeCtx(node: CanvasNode): ExtensionContext {
  return {
    node,
    body:     document.createElement("div"),
    canvasEl: document.createElement("canvas"),
    onChange: () => {},
    creds:    [],
    rerender: () => {},
  };
}

describe("field-renderer — maxLength", () => {
  it("normal case: a maxLength-bearing prop sets the input's native maxLength and appends it to the hint", () => {
    const ctx = makeCtx(makeNode("n1"));
    const props: Array<[string, PropSchema]> = [
      ["filename_prefix", {
        type: "string",
        description: "Optional prefix prepended to every saved filename",
        maxLength: 50,
      }],
    ];

    renderConfigFieldsLoop(ctx, props, []);

    const input = ctx.body.querySelector("input") as HTMLInputElement;
    expect(input.maxLength).toBe(50);

    const hint = ctx.body.querySelector(".field-hint") as HTMLElement;
    expect(hint.textContent).toBe(
      "Optional prefix prepended to every saved filename · Max 50 characters"
    );
  });

  it("edge case: a prop with no maxLength leaves the native maxLength unset and the hint as the bare description", () => {
    const ctx = makeCtx(makeNode("n1"));
    const props: Array<[string, PropSchema]> = [
      ["some_field", { type: "string", description: "Just a description" }],
    ];

    renderConfigFieldsLoop(ctx, props, []);

    const input = ctx.body.querySelector("input") as HTMLInputElement;
    expect(input.hasAttribute("maxlength")).toBe(false);

    const hint = ctx.body.querySelector(".field-hint") as HTMLElement;
    expect(hint.textContent).toBe("Just a description");
  });
});

describe("field-renderer — model picker", () => {
  it("normal case: x-aerini-model-picker renders the free-text field plus a Fetch Models button, and a successful fetch offers a dropdown that writes into the field", async () => {
    const ctx = makeCtx(makeNode("n1"));
    ctx.fetchModels = vi.fn().mockResolvedValue(["gpt-5.6", "gpt-5.6-mini"]);
    const props: Array<[string, PropSchema]> = [
      ["model", { type: "string", description: "Model name", "x-aerini-model-picker": true }],
    ];

    renderConfigFieldsLoop(ctx, props, []);

    const input = ctx.body.querySelector("input") as HTMLInputElement;
    expect(input).toBeTruthy(); // manual entry is present from the start, not gated on a fetch

    const fetchBtn = Array.from(ctx.body.querySelectorAll("button"))
      .find(b => b.textContent === "Fetch Models") as HTMLButtonElement;
    expect(fetchBtn).toBeTruthy();

    fetchBtn.click();
    await vi.waitFor(() => expect(ctx.fetchModels).toHaveBeenCalledTimes(1));
    await vi.waitFor(() => expect(ctx.body.querySelectorAll(".csel-option").length).toBe(2));

    const secondOption = Array.from(ctx.body.querySelectorAll<HTMLElement>(".csel-option"))
      .find(el => el.textContent === "gpt-5.6-mini")!;
    secondOption.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));

    expect(ctx.node.data.config["model"]).toBe("gpt-5.6-mini");
    expect(input.value).toBe("gpt-5.6-mini"); // text field mirrors the dropdown pick
  });

  it("edge case: a failed fetch leaves the free-text field exactly as it was — no dropdown, no error banner in the form, value untouched", async () => {
    const ctx = makeCtx(makeNode("n1"));
    ctx.fetchModels = vi.fn().mockRejectedValue(new Error("network error"));
    const node = ctx.node;
    node.data.config["model"] = "llama3"; // pre-existing manual entry
    const props: Array<[string, PropSchema]> = [
      ["model", { type: "string", "x-aerini-model-picker": true }],
    ];

    renderConfigFieldsLoop(ctx, props, []);

    const input = ctx.body.querySelector("input") as HTMLInputElement;
    expect(input.value).toBe("llama3");

    const fetchBtn = Array.from(ctx.body.querySelectorAll("button"))
      .find(b => b.textContent === "Fetch Models") as HTMLButtonElement;
    fetchBtn.click();
    await vi.waitFor(() => expect(fetchBtn.disabled).toBe(false));

    expect(ctx.body.querySelectorAll(".csel-option").length).toBe(0);
    expect(input.value).toBe("llama3"); // never touched by the failed fetch
    expect(ctx.node.data.config["model"]).toBe("llama3");
  });

  it("edge case: with no fetchModels wired (a node type that didn't opt in), the field silently degrades to plain text — no Fetch Models button at all", () => {
    const ctx = makeCtx(makeNode("n1")); // makeCtx doesn't set fetchModels
    const props: Array<[string, PropSchema]> = [
      ["model", { type: "string", "x-aerini-model-picker": true }],
    ];

    renderConfigFieldsLoop(ctx, props, []);

    expect(ctx.body.querySelector("input")).toBeTruthy();
    const fetchBtn = Array.from(ctx.body.querySelectorAll("button"))
      .find(b => b.textContent === "Fetch Models");
    expect(fetchBtn).toBeUndefined();
  });
});

describe("field-renderer — credential fields", () => {
  it("normal case: x-aerini-credential with a cred_type renders one picker per field and hard-filters each by type", () => {
    const ctx = makeCtx(makeNode("n1"));
    ctx.creds.push(
      { id: "c1", name: "AWS prod",   cred_type: "api_key" },
      { id: "c2", name: "GH token",   cred_type: "oauth"   },
    );
    const props: Record<string, PropSchema> = {
      access_key_id:     { type: "string", "x-aerini-credential": { cred_type: "api_key" } },
      secret_access_key: { type: "string", "x-aerini-credential": { cred_type: "oauth" } },
    };

    renderCredentialSection(ctx, props, undefined, () => {});

    const fieldLabels = Array.from(ctx.body.querySelectorAll(".field-label")).map(el => el.textContent);
    expect(fieldLabels).toContain("Use Saved Credential — Access Key ID");
    expect(fieldLabels).toContain("Use Saved Credential — Secret Access Key");
    expect(ctx.body.querySelectorAll(".config-hint-warn").length).toBe(0);
    expect(getCredentialFieldKeys(props)).toEqual(new Set(["access_key_id", "secret_access_key"]));
  });

  it("edge case: a declared cred_type with no matching saved credential shows an empty picker and a type-specific warning, not a fallback to all credentials", () => {
    const ctx = makeCtx(makeNode("n1"));
    ctx.creds.push({ id: "c1", name: "Only a bearer token", cred_type: "bearer" });
    const props: Record<string, PropSchema> = {
      api_secret: { type: "string", "x-aerini-credential": { cred_type: "oauth" } },
    };

    renderCredentialSection(ctx, props, undefined, () => {});

    const options = Array.from(ctx.body.querySelectorAll(".csel-option")).map(el => el.textContent);
    expect(options).toEqual(["— none —"]);
    const warn = ctx.body.querySelector(".config-hint-warn") as HTMLElement;
    expect(warn.textContent).toBe('No saved credentials of type "oauth". Add one in Credentials in the toolbar.');
  });

  it("backward compat: a bare 'api_key' field with no annotation still renders unfiltered, exactly as before this change", () => {
    const ctx = makeCtx(makeNode("n1"));
    ctx.creds.push({ id: "c1", name: "Any old key", cred_type: "basic" });
    const props: Record<string, PropSchema> = { api_key: { type: "string" } };

    renderCredentialSection(ctx, props, undefined, () => {});

    const options = Array.from(ctx.body.querySelectorAll(".csel-option")).map(el => el.textContent);
    expect(options).toEqual(["— none —", "Any old key"]);
    expect(getCredentialFieldKeys(props)).toEqual(new Set(["api_key"]));
  });
});

function makeAiNode(id: string, provider: string): CanvasNode {
  return new CanvasNode({
    id,
    node_type_id: "ai_prompt",
    node_type: "ai",
    name: `Node ${id}`,
    config: { provider },
    credentials: {},
    position: { x: 0, y: 0 },
    ports: {
      inputs:  [{ id: "input",  label: "Input",  position: "left"  }],
      outputs: [{ id: "output", label: "Output", position: "right" }],
    },
    input_schema: {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  });
}

const AI_API_KEY_PROPS: Record<string, PropSchema> = { api_key: { type: "string" } };

describe("field-renderer — AI-node credential provider filtering", () => {
  it("normal case: the api_key picker on an AI node hard-filters to credentials matching the node's Provider, plus wildcard (no-metadata) credentials, and labels matched entries with their provider", () => {
    const ctx = makeCtx(makeAiNode("n1", "anthropic"));
    ctx.creds.push(
      { id: "c1", name: "Claude key",    cred_type: "api_key" },
      { id: "c2", name: "OpenAI key",    cred_type: "api_key" },
      { id: "c3", name: "Unlabeled key", cred_type: "api_key" },
    );
    const providerMap = new Map<string, string | undefined>([
      ["c1", "anthropic"], ["c2", "openai"], ["c3", undefined],
    ]);

    renderCredentialSection(ctx, AI_API_KEY_PROPS, undefined, () => {}, providerMap);

    const options = Array.from(ctx.body.querySelectorAll(".csel-option")).map(el => el.textContent);
    expect(options).toEqual(["— none —", "Claude key — Anthropic", "Unlabeled key"]);
  });

  it("edge case: no credential matches the node's Provider — pool is empty and the warning names the provider, not the generic cred-type message", () => {
    const ctx = makeCtx(makeAiNode("n1", "gemini"));
    ctx.creds.push({ id: "c1", name: "OpenAI key", cred_type: "api_key" });
    const providerMap = new Map<string, string | undefined>([["c1", "openai"]]);

    renderCredentialSection(ctx, AI_API_KEY_PROPS, undefined, () => {}, providerMap);

    const options = Array.from(ctx.body.querySelectorAll(".csel-option")).map(el => el.textContent);
    expect(options).toEqual(["— none —"]);
    const warn = ctx.body.querySelector(".config-hint-warn") as HTMLElement;
    expect(warn.textContent).toBe("No saved credentials for provider Gemini. Add one in Credentials in the toolbar.");
  });

  it("edge case: a non-AI node's api_key field is unaffected by provider filtering, even with a provider map supplied", () => {
    const ctx = makeCtx(makeNode("n1")); // node_type_id "save_to_folder", not an AI node
    ctx.creds.push({ id: "c1", name: "Some key", cred_type: "api_key" });
    const providerMap = new Map<string, string | undefined>([["c1", "openai"]]);

    renderCredentialSection(ctx, AI_API_KEY_PROPS, undefined, () => {}, providerMap);

    const options = Array.from(ctx.body.querySelectorAll(".csel-option")).map(el => el.textContent);
    expect(options).toEqual(["— none —", "Some key"]); // no provider suffix, not filtered
  });
});

describe("field-renderer — Provider field rebuilds the popover on AI nodes", () => {
  it("normal case: changing Provider on an AI node calls ctx.rerender (so the Connection section's filter re-runs against the new value)", () => {
    const ctx = makeCtx(makeAiNode("n1", "openai"));
    const rerenderSpy = vi.fn();
    ctx.rerender = rerenderSpy;
    const props: Array<[string, PropSchema]> = [
      ["provider", { type: "string", enum: ["auto", "openai", "anthropic", "gemini", "local"] }],
    ];

    renderConfigFieldsLoop(ctx, props, []);
    const option = Array.from(ctx.body.querySelectorAll<HTMLElement>(".csel-option"))
      .find(el => el.textContent === "anthropic")!;
    option.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));

    expect(ctx.node.data.config["provider"]).toBe("anthropic");
    expect(rerenderSpy).toHaveBeenCalledTimes(1);
  });

  it("edge case: changing an unrelated enum field on a non-AI node does not call ctx.rerender", () => {
    const ctx = makeCtx(makeNode("n1")); // "save_to_folder", not an AI node
    const rerenderSpy = vi.fn();
    ctx.rerender = rerenderSpy;
    const props: Array<[string, PropSchema]> = [
      ["mode", { type: "string", enum: ["flat", "nested"] }],
    ];

    renderConfigFieldsLoop(ctx, props, []);
    const option = Array.from(ctx.body.querySelectorAll<HTMLElement>(".csel-option"))
      .find(el => el.textContent === "nested")!;
    option.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));

    expect(ctx.node.data.config["mode"]).toBe("nested");
    expect(rerenderSpy).not.toHaveBeenCalled();
  });
});
