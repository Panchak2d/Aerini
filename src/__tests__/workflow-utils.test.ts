/* @vitest-environment jsdom */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { CanvasNode } from "../canvas/Node";
import type { ExtensionContext } from "../node-configs/popover-utils";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level);
// several files under test import Canvas.ts / output-renderer.ts, which
// pull in the same chain.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

// jsdom does not implement Element.scrollIntoView — stub it so
// popover-utils.ts's openDropdown() (unrelated pre-existing code, not part
// of this batch's fix) doesn't throw when a select is opened in tests.
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}

function makeNode(id: string, typeId: string, dynamicPorts = false): CanvasNode {
  return new CanvasNode({
    id,
    node_type_id: typeId,
    node_type: "action",
    name: `Node ${id}`,
    config: {},
    credentials: {},
    position: { x: 0, y: 0 },
    ports: { inputs: [], outputs: [{ id: "output", label: "Output", position: "right" }] },
    input_schema: {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: dynamicPorts,
  });
}

function makeCtx(node: CanvasNode): ExtensionContext {
  return {
    node,
    body: document.createElement("div"),
    canvasEl: document.createElement("canvas") as unknown as HTMLCanvasElement,
    onChange: () => {},
    creds: [],
    rerender: () => {},
  };
}

// ---------------------------------------------------------------------------
//  CredentialPanel.buildCredTypeSelect document-listener leak
// ---------------------------------------------------------------------------

describe("CredentialPanel buildCredTypeSelect, listener leak", () => {
  it("removes the previous outside-click listener before adding a new one on repeated calls", async () => {
    vi.resetModules();
    const { buildCredTypeSelect } = await import("../panels/CredentialPanel");
    const addSpy    = vi.spyOn(document, "addEventListener");
    const removeSpy = vi.spyOn(document, "removeEventListener");

    const types = [{ value: "a", label: "A", hint: "h", placeholder: "p" }];
    buildCredTypeSelect(types, () => {});
    buildCredTypeSelect(types, () => {});
    buildCredTypeSelect(types, () => {});

    const adds    = addSpy.mock.calls.filter(c => c[0] === "mousedown");
    const removes = removeSpy.mock.calls.filter(c => c[0] === "mousedown");
    // 3 renders → 3 listeners added, but the first 2 are cleaned up before
    // the next render adds its own. Pre-fix: 3 added, 0 ever removed.
    expect(adds.length).toBe(3);
    expect(removes.length).toBe(2);
  });
});

// ---------------------------------------------------------------------------
// popover-utils mkCustomSelect document-listener leak
// ---------------------------------------------------------------------------

describe("popover-utils mkCustomSelect, dropdown leak", () => {
  beforeEach(() => {
    vi.resetModules();
    document.body.innerHTML = "";
  });

  it("binds the shared outside-click listener at most once, regardless of how many selects open", async () => {
    const { mkCustomSelect } = await import("../node-configs/popover-utils");
    const addSpy = vi.spyOn(document, "addEventListener");

    const a = mkCustomSelect(["x", "y"], "x", () => {});
    const b = mkCustomSelect(["p", "q"], "p", () => {});
    document.body.appendChild(a);
    document.body.appendChild(b);

    a.querySelector<HTMLButtonElement>(".csel-trigger")!.click();
    b.querySelector<HTMLButtonElement>(".csel-trigger")!.click();

    const capturingMousedownAdds = addSpy.mock.calls.filter(c => c[0] === "mousedown" && c[2] === true);
    // Pre-fix: one new document listener per open() call (2 here, growing
    // unboundedly with usage). Post-fix: exactly one, shared.
    expect(capturingMousedownAdds.length).toBe(1);
  });

  it("closes an open dropdown on a real outside mousedown via the shared listener", async () => {
    const { mkCustomSelect } = await import("../node-configs/popover-utils");
    const a = mkCustomSelect(["x", "y"], "x", () => {});
    document.body.appendChild(a);

    a.querySelector<HTMLButtonElement>(".csel-trigger")!.click();
    expect(a.querySelector(".csel-dropdown")!.classList.contains("hidden")).toBe(false);

    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(a.querySelector(".csel-dropdown")!.classList.contains("hidden")).toBe(true);
  });

  it("does not throw when a wrap is disconnected from the DOM without closeDropdown() running first", async () => {
    const { mkCustomSelect } = await import("../node-configs/popover-utils");
    const a = mkCustomSelect(["x", "y"], "x", () => {});
    document.body.appendChild(a);
    a.querySelector<HTMLButtonElement>(".csel-trigger")!.click(); // open, register
    a.remove(); // simulate popover teardown that skips closeDropdown()

    expect(() => {
      document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    }).not.toThrow();
  });
});

// ---------------------------------------------------------------------------
// Canvas.destroy() dead/incomplete code, removed 
// ---------------------------------------------------------------------------

describe("Canvas — dead destroy() removed", () => {
  it("no longer has a destroy() method", async () => {
    const { Canvas } = await import("../canvas/Canvas");
    expect("destroy" in Canvas.prototype).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// run-manager/state-machine.ts dead RunEvent/transition() 
// ---------------------------------------------------------------------------

describe("run-manager/state-machine, dead code removed", () => {
  it("RunStateMachine no longer has a transition() method", async () => {
    const { RunStateMachine } = await import("../run-manager/state-machine");
    expect("transition" in RunStateMachine.prototype).toBe(false);
  });

  it("existing named methods still work (no regression from the removal)", async () => {
    const { RunStateMachine } = await import("../run-manager/state-machine");
    const sm = new RunStateMachine();
    sm.start("run_test");
    expect(sm.isRunning).toBe(true);
    sm.setCurrentWorkflow("wf_1", "Test", true, 4);
    expect(sm.currentWorkflowId).toBe("wf_1");
    expect(sm.currentParallelExecution).toBe(true);
    expect(sm.currentMaxConcurrentNodes).toBe(4);
  });
});

// ---------------------------------------------------------------------------
// output-renderer.ts dead renderRunSummary export
// ---------------------------------------------------------------------------

describe("output-renderer, dead code removed", () => {
  it("no longer exports renderRunSummary", async () => {
    const mod: Record<string, unknown> = await import("../output-renderer");
    expect(mod.renderRunSummary).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
//  save-to-folder-config.ts / collect-files-config.ts slot-id entropy
// ---------------------------------------------------------------------------

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

describe("save-to-folder-config, subfolder slot id entropy", () => {
  it("generates distinct UUID-based ids, not a Date.now() timestamp", async () => {
    const { renderSaveToFolderFields } = await import("../node-configs/save-to-folder-config");
    const node = makeNode("n1", "save_to_folder", true);
    const ctx = makeCtx(node);
    renderSaveToFolderFields(ctx);

    const addBtn = ctx.body.querySelector<HTMLButtonElement>(".subfolder-add-btn")!;
    addBtn.click();
    addBtn.click();

    const subfolders = node.data.config["subfolders"] as Array<{ id: string }>;
    expect(subfolders.length).toBe(2);
    expect(subfolders[0].id).not.toBe(subfolders[1].id);
    for (const sf of subfolders) {
      expect(sf.id.startsWith("sf_")).toBe(true);
      expect(UUID_RE.test(sf.id.slice(3))).toBe(true);
    }
  });
});

describe("collect-files-config, source slot id entropy", () => {
  it("generates distinct UUID-based ids, not a Date.now() timestamp", async () => {
    const { renderCollectFilesFields } = await import("../node-configs/collect-files-config");
    const node = makeNode("n1", "collect_files", true);
    const ctx = makeCtx(node);
    renderCollectFilesFields(ctx);

    const addBtn = ctx.body.querySelector<HTMLButtonElement>(".subfolder-add-btn")!;
    addBtn.click();
    addBtn.click();

    const sources = node.data.config["sources"] as Array<{ id: string }>;
    expect(sources.length).toBe(2);
    expect(sources[0].id).not.toBe(sources[1].id);
    for (const src of sources) {
      expect(src.id.startsWith("src_")).toBe(true);
      expect(UUID_RE.test(src.id.slice(4))).toBe(true);
    }
  });
});
