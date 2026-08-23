/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function setupConfirmDom(): void {
  document.body.innerHTML = `
    <div id="confirm-modal" class="hidden">
      <div class="confirm-backdrop"></div>
      <div class="confirm-message" id="confirm-message"></div>
      <button class="confirm-btn-cancel" id="confirm-cancel">Cancel</button>
      <button class="confirm-btn-ok" id="confirm-ok">Continue</button>
    </div>`;
}

async function flush(): Promise<void> {
  await new Promise(r => setTimeout(r, 0));
}

beforeEach(() => {
  vi.resetModules();
  vi.clearAllMocks();
  setupConfirmDom();
});

describe("VersionPanel — delete confirm gate", () => {
  function makeWfManager(overrides: Partial<{
    getVersions: () => Promise<unknown[]>;
    deleteVersion: (id: string) => Promise<void>;
    restoreVersion: (id: string) => Promise<{ id: string; name: string } | null>;
  }> = {}) {
    return {
      getVersions: overrides.getVersions ?? (async () => [
        { id: "v1", workflow_id: "wf1", message: "First save", created_at: "2026-01-01T00:00:00Z" },
      ]),
      deleteVersion: overrides.deleteVersion ?? vi.fn(async () => {}),
      restoreVersion: overrides.restoreVersion ?? vi.fn(async () => ({ id: "wf1", name: "wf" })),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;
  }

  it("normal case: OK confirms and deletes the version, removing the item", async () => {
    const { showVersionPanel } = await import("../panels/VersionPanel");
    const wfManager = makeWfManager();
    const toast = vi.fn();

    await showVersionPanel(wfManager, toast);

    const delBtn = document.querySelector<HTMLButtonElement>(".version-delete-btn")!;
    delBtn.click();
    await flush();
    expect(document.getElementById("confirm-message")!.textContent).toContain("Delete version from");

    document.getElementById("confirm-ok")!.click();
    await flush();

    expect(wfManager.deleteVersion).toHaveBeenCalledWith("v1");
    expect(document.querySelector(".version-item")).toBeNull();
  });

  it("edge case: Cancel leaves the version untouched — deleteVersion never called", async () => {
    const { showVersionPanel } = await import("../panels/VersionPanel");
    const wfManager = makeWfManager();
    const toast = vi.fn();

    await showVersionPanel(wfManager, toast);
    document.querySelector<HTMLButtonElement>(".version-delete-btn")!.click();
    await flush();
    document.getElementById("confirm-cancel")!.click();
    await flush();

    expect(wfManager.deleteVersion).not.toHaveBeenCalled();
    expect(document.querySelector(".version-item")).not.toBeNull();
  });
});

describe("VersionPanel — restore confirm-before-close", () => {
  function makeWfManager() {
    return {
      getVersions: async () => [
        { id: "v1", workflow_id: "wf1", message: "First save", created_at: "2026-01-01T00:00:00Z" },
      ],
      deleteVersion: vi.fn(async () => {}),
      restoreVersion: vi.fn(async () => ({ id: "wf1", name: "wf" })),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;
  }

  it("normal case: panel stays open while the dialog is up, closes only after OK, then restores", async () => {
    const { showVersionPanel } = await import("../panels/VersionPanel");
    const wfManager = makeWfManager();
    const toast = vi.fn();

    await showVersionPanel(wfManager, toast);
    document.querySelector<HTMLButtonElement>(".version-restore-btn")!.click();
    await flush();

    expect(document.getElementById("version-panel-overlay")).not.toBeNull();

    document.getElementById("confirm-ok")!.click();
    await flush();

    expect(wfManager.restoreVersion).toHaveBeenCalledWith("v1");
    expect(document.getElementById("version-panel-overlay")).toBeNull();
  });

  it("edge case: Cancel leaves the panel open and never restores", async () => {
    const { showVersionPanel } = await import("../panels/VersionPanel");
    const wfManager = makeWfManager();
    const toast = vi.fn();

    await showVersionPanel(wfManager, toast);
    document.querySelector<HTMLButtonElement>(".version-restore-btn")!.click();
    await flush();
    document.getElementById("confirm-cancel")!.click();
    await flush();

    expect(wfManager.restoreVersion).not.toHaveBeenCalled();
    expect(document.getElementById("version-panel-overlay")).not.toBeNull();
  });
});

describe("plugin-settings — remove confirm gate", () => {
  function setupPluginDom(): void {
    document.body.innerHTML += `
      <button id="btn-plugin-dir-browse"></button>
      <button id="btn-install-plugin"></button>
      <button id="tab-plugins"></button>
      <div id="plugin-dir-path"></div>
      <div id="plugin-list"></div>
      <div id="plugin-restart-banner" hidden></div>`;
  }

  async function loadWithOnePlugin() {
    const core = await import("@tauri-apps/api/core");
    const invoke = core.invoke as unknown as ReturnType<typeof vi.fn>;
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_setting") return Promise.resolve("/plugins");
      if (cmd === "list_installed_plugins") return Promise.resolve([
        { filename: "cool.wasm", display_name: "Cool Plugin", type_id: "cool", category: "action", load_error: null },
      ]);
      if (cmd === "remove_plugin") return Promise.resolve();
      return Promise.resolve(null);
    });

    setupPluginDom();
    const { bindPluginSettings } = await import("../plugin-settings");
    const toast = vi.fn();
    bindPluginSettings(toast);
    document.getElementById("tab-plugins")!.click();
    await flush();
    await flush(); // second tick: getSetting resolves, then refreshPlugins' own await
    return { invoke, toast };
  }

  it("normal case: OK confirms and removes the plugin, message names it by display_name", async () => {
    const { invoke, toast } = await loadWithOnePlugin();

    const removeBtn = document.querySelector<HTMLButtonElement>(".btn-plugin-remove")!;
    expect(removeBtn).not.toBeNull();
    removeBtn.click();
    await flush();

    expect(document.getElementById("confirm-message")!.textContent).toContain("Cool Plugin");

    document.getElementById("confirm-ok")!.click();
    await flush();
    await flush();

    expect(invoke).toHaveBeenCalledWith("remove_plugin", { filename: "cool.wasm", pluginDir: "/plugins" });
    expect(invoke).toHaveBeenCalledWith("reload_plugins", { pluginDir: "/plugins" });
    expect(toast).toHaveBeenCalledWith("Plugin removed.", "success");
  });

  it("edge case: Cancel leaves the plugin installed — remove_plugin never invoked", async () => {
    const { invoke, toast } = await loadWithOnePlugin();

    document.querySelector<HTMLButtonElement>(".btn-plugin-remove")!.click();
    await flush();
    document.getElementById("confirm-cancel")!.click();
    await flush();

    expect(invoke).not.toHaveBeenCalledWith("remove_plugin", expect.anything());
    expect(toast).not.toHaveBeenCalledWith(expect.stringContaining("removed"), expect.anything());
  });
});
