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
