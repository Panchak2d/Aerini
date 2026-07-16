/**
 * @vitest-environment jsdom
 *
 * Batch P — regression tests, one describe block per finding.
 *
 * DEVIATION NOTE: `WorkflowManager` is not unit-testable in isolation per
 * the existing precedent stated in workflow-manager.test.ts ("requires a
 * live Canvas, DOM, localStorage, and Tauri IPC"). S11-14's `currentId`
 * fix is therefore verified by direct source inspection (both call sites
 * now read `wf_${crypto.randomUUID()}`, confirmed in this same patch's
 * manual trace) rather than a runtime WorkflowManager test — consistent
 * with that file's own stated testing boundary, not a gap introduced here.
 * S11-14's sibling fix in this same file family (the node-config slot-id
 * generators) *is* independently testable and is covered below.
 *
 * Module-level state note: `popover-utils.ts`'s delegated-listener registry
 * is module-scoped by design (that's the whole fix). Its describe block
 * uses `vi.resetModules()` + dynamic `import()` per test so each test case
 * observes a fresh module instance instead of leaking state between cases.
 */
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

// T2-14 drag-drop mocks: vi.mock() is hoisted above all imports, so any
// variable a factory references must come from vi.hoisted() — a bare
// module-scope `const` declared after the vi.mock() call would still be
// uninitialized ("Cannot access before initialization") at the time the
// hoisted factory runs. Declared here, at module scope, per Vitest's
// documented mocking requirements (vi.mock cannot live inside describe()).
const dragDropMocks = vi.hoisted(() => ({
  getSetting: vi.fn(),
  installPluginFromPath: vi.fn(),
  showConfirm: vi.fn(),
  showRestartBanner: vi.fn(),
}));
vi.mock("../ipc/workflow", () => ({
  getSetting: dragDropMocks.getSetting,
  installPluginFromPath: dragDropMocks.installPluginFromPath,
}));
vi.mock("../confirm", () => ({
  showConfirm: dragDropMocks.showConfirm,
}));
vi.mock("../plugin-settings", () => ({
  showRestartBanner: dragDropMocks.showRestartBanner,
}));

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
// S10-5 — CredentialPanel.buildCredTypeSelect document-listener leak
// ---------------------------------------------------------------------------

describe("CredentialPanel buildCredTypeSelect — S10-5 listener leak", () => {
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
// S10-6 — popover-utils mkCustomSelect document-listener leak
// ---------------------------------------------------------------------------

describe("popover-utils mkCustomSelect — S10-6 dropdown leak", () => {
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
// S9-6 — Canvas.destroy() dead/incomplete code, removed (Rule 25)
// ---------------------------------------------------------------------------

describe("Canvas — S9-6 dead destroy() removed (Rule 25)", () => {
  it("no longer has a destroy() method", async () => {
    const { Canvas } = await import("../canvas/Canvas");
    expect("destroy" in Canvas.prototype).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// T2-14 / S9-5 — drag-drop.ts handleWasmDrop plugin-install confirm gate
// ---------------------------------------------------------------------------

describe("drag-drop handleWasmDrop — T2-14 confirm gate", () => {
  const { getSetting, installPluginFromPath, showConfirm, showRestartBanner } = dragDropMocks;

  beforeEach(() => {
    getSetting.mockReset();
    installPluginFromPath.mockReset();
    showConfirm.mockReset();
    showRestartBanner.mockReset();
  });

  it("does not prompt or install when no plugin directory is configured", async () => {
    getSetting.mockResolvedValue(null);
    const { handleWasmDrop } = await import("../drag-drop");
    const toast = vi.fn();

    await handleWasmDrop("/home/user/my-plugin.wasm", toast);

    expect(showConfirm).not.toHaveBeenCalled();
    expect(installPluginFromPath).not.toHaveBeenCalled();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("plugin directory"), "info");
  });

  it("does not install when the user cancels the confirmation", async () => {
    getSetting.mockResolvedValue("/plugins");
    showConfirm.mockResolvedValue(false);
    const { handleWasmDrop } = await import("../drag-drop");
    const toast = vi.fn();

    await handleWasmDrop("/home/user/my-plugin.wasm", toast);

    expect(showConfirm).toHaveBeenCalledOnce();
    expect(installPluginFromPath).not.toHaveBeenCalled();
  });

  it("installs and shows the restart banner only after the user confirms", async () => {
    getSetting.mockResolvedValue("/plugins");
    showConfirm.mockResolvedValue(true);
    installPluginFromPath.mockResolvedValue(undefined);
    const { handleWasmDrop } = await import("../drag-drop");
    const toast = vi.fn();

    await handleWasmDrop("/home/user/my-plugin.wasm", toast);

    expect(showConfirm).toHaveBeenCalledOnce();
    expect(showConfirm.mock.calls[0][0]).toContain("my-plugin.wasm");
    expect(installPluginFromPath).toHaveBeenCalledWith("/home/user/my-plugin.wasm", "/plugins");
    expect(showRestartBanner).toHaveBeenCalledOnce();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("installed"), "success");
  });
});

// ---------------------------------------------------------------------------
// S11-11 — run-manager/state-machine.ts dead RunEvent/transition() (Rule 25)
// ---------------------------------------------------------------------------

describe("run-manager/state-machine — S11-11 dead code removed (Rule 25)", () => {
  it("RunStateMachine no longer has a transition() method", async () => {
    const { RunStateMachine } = await import("../run-manager/state-machine");
    expect("transition" in RunStateMachine.prototype).toBe(false);
  });

  it("existing named methods still work (no regression from the removal)", async () => {
    const { RunStateMachine } = await import("../run-manager/state-machine");
    const sm = new RunStateMachine();
    sm.start();
    expect(sm.isRunning).toBe(true);
    sm.setCurrentWorkflow("wf_1", "Test", true, 4);
    expect(sm.currentWorkflowId).toBe("wf_1");
    expect(sm.currentParallelExecution).toBe(true);
    expect(sm.currentMaxConcurrentNodes).toBe(4);
  });
});

// ---------------------------------------------------------------------------
// S12-10 — output-renderer.ts dead renderRunSummary export (Rule 25)
// ---------------------------------------------------------------------------

describe("output-renderer — S12-10 dead code removed (Rule 25)", () => {
  it("no longer exports renderRunSummary", async () => {
    const mod: Record<string, unknown> = await import("../output-renderer");
    expect(mod.renderRunSummary).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// S10-8 — save-to-folder-config.ts / collect-files-config.ts slot-id entropy
// ---------------------------------------------------------------------------

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

describe("save-to-folder-config — S10-8 subfolder slot id entropy", () => {
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

describe("collect-files-config — S10-8 source slot id entropy", () => {
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
