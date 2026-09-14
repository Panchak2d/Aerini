/* @vitest-environment jsdom */

import { describe, it, expect, vi, beforeEach } from "vitest";

const mocks = vi.hoisted(() => ({
  installPluginFromPath: vi.fn(),
  installPluginPackFromPath: vi.fn(),
  removePluginPack: vi.fn(),
  reloadPlugins: vi.fn(),
  showConfirm: vi.fn(),
}));
vi.mock("../ipc/workflow", () => ({
  getSetting: vi.fn(),
  setSetting: vi.fn(),
  pickFolderDialog: vi.fn(),
  pickPluginFileDialog: vi.fn(),
  listInstalledPlugins: vi.fn(),
  installPluginFromPath: mocks.installPluginFromPath,
  removePlugin: vi.fn(),
  reloadPlugins: mocks.reloadPlugins,
  installPluginPackFromPath: mocks.installPluginPackFromPath,
  removePluginPack: mocks.removePluginPack,
}));
vi.mock("../confirm", () => ({
  showConfirm: mocks.showConfirm,
}));

describe("plugin-settings installWithUpdatePrompt, update flow", () => {
  beforeEach(() => {
    mocks.installPluginFromPath.mockReset();
    mocks.installPluginPackFromPath.mockReset();
    mocks.removePluginPack.mockReset();
    mocks.reloadPlugins.mockReset();
    mocks.reloadPlugins.mockResolvedValue({ loaded: [], builtin_rejected: [], plugin_collisions: [] });
    mocks.showConfirm.mockReset();
  });

  it("installs cleanly and does not prompt when nothing is already installed", async () => {
    mocks.installPluginFromPath.mockResolvedValue("plugin.wasm");
    const { installWithUpdatePrompt } = await import("../plugin-settings");
    const toast = vi.fn();

    await installWithUpdatePrompt("/src/plugin.wasm", "/plugins", toast);

    expect(mocks.installPluginFromPath).toHaveBeenCalledWith("/src/plugin.wasm", "/plugins", false);
    expect(mocks.showConfirm).not.toHaveBeenCalled();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("installed"), "success");
  });

  it("offers an update confirmation on a plugin_already_installed rejection, and retries with overwrite when confirmed", async () => {
    mocks.installPluginFromPath
      .mockRejectedValueOnce("plugin_already_installed: 'x' is already installed as 'x.wasm'")
      .mockResolvedValueOnce("x.wasm");
    mocks.showConfirm.mockResolvedValue(true);
    const { installWithUpdatePrompt } = await import("../plugin-settings");
    const toast = vi.fn();

    await installWithUpdatePrompt("/src/x.wasm", "/plugins", toast);

    expect(mocks.showConfirm.mock.calls[0][0]).toContain("already installed as 'x.wasm'");
    expect(mocks.installPluginFromPath).toHaveBeenNthCalledWith(2, "/src/x.wasm", "/plugins", true);
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("updated"), "success");
  });

  it("does not retry when the user declines the update confirmation", async () => {
    mocks.installPluginFromPath.mockRejectedValueOnce("plugin_already_installed: 'x' is already installed");
    mocks.showConfirm.mockResolvedValue(false);
    const { installWithUpdatePrompt } = await import("../plugin-settings");
    const toast = vi.fn();

    await installWithUpdatePrompt("/src/x.wasm", "/plugins", toast);

    expect(mocks.installPluginFromPath).toHaveBeenCalledTimes(1);
    expect(toast).not.toHaveBeenCalled();
  });

  it("reloads the registry live after a successful install, reporting it as activated rather than restart-required", async () => {
    mocks.installPluginFromPath.mockResolvedValue("plugin.wasm");
    const { installWithUpdatePrompt } = await import("../plugin-settings");
    const toast = vi.fn();

    await installWithUpdatePrompt("/src/plugin.wasm", "/plugins", toast);

    expect(mocks.reloadPlugins).toHaveBeenCalledWith("/plugins");
    expect(toast).toHaveBeenCalledWith("Plugin installed and activated.", "success");
  });

  it("falls back to restart messaging, without failing the install, when the post-install reload call errors", async () => {
    mocks.installPluginFromPath.mockResolvedValue("plugin.wasm");
    mocks.reloadPlugins.mockRejectedValue(new Error("engine busy"));
    const { installWithUpdatePrompt } = await import("../plugin-settings");
    const toast = vi.fn();

    await installWithUpdatePrompt("/src/plugin.wasm", "/plugins", toast);

    expect(toast).toHaveBeenCalledWith(expect.stringContaining("reload failed"), "info");
  });
});

describe("plugin-settings installPackWithUpdatePrompt, pack update flow", () => {
  beforeEach(() => {
    mocks.installPluginPackFromPath.mockReset();
    mocks.reloadPlugins.mockReset();
    mocks.reloadPlugins.mockResolvedValue({ loaded: [], builtin_rejected: [], plugin_collisions: [] });
    mocks.showConfirm.mockReset();
  });

  it("installs a pack cleanly and does not prompt when nothing is already installed", async () => {
    mocks.installPluginPackFromPath.mockResolvedValue("com.example.mypack");
    const { installPackWithUpdatePrompt } = await import("../plugin-settings");
    const toast = vi.fn();

    await installPackWithUpdatePrompt("/src/mypack.aerinipkg", "/plugins", toast);

    expect(mocks.installPluginPackFromPath).toHaveBeenCalledWith("/src/mypack.aerinipkg", "/plugins", false);
    expect(mocks.showConfirm).not.toHaveBeenCalled();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("pack installed"), "success");
  });

  it("offers an update confirmation on a pack_already_installed rejection, and retries with overwrite when confirmed", async () => {
    mocks.installPluginPackFromPath
      .mockRejectedValueOnce("pack_already_installed: 'com.example.mypack' is already installed")
      .mockResolvedValueOnce("com.example.mypack");
    mocks.showConfirm.mockResolvedValue(true);
    const { installPackWithUpdatePrompt } = await import("../plugin-settings");
    const toast = vi.fn();

    await installPackWithUpdatePrompt("/src/mypack.aerinipkg", "/plugins", toast);

    expect(mocks.showConfirm.mock.calls[0][0]).toContain("already installed");
    expect(mocks.installPluginPackFromPath).toHaveBeenNthCalledWith(2, "/src/mypack.aerinipkg", "/plugins", true);
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("pack updated"), "success");
  });

  it("does not retry when the user declines the update confirmation", async () => {
    mocks.installPluginPackFromPath.mockRejectedValueOnce("pack_already_installed: 'com.example.mypack' is already installed");
    mocks.showConfirm.mockResolvedValue(false);
    const { installPackWithUpdatePrompt } = await import("../plugin-settings");
    const toast = vi.fn();

    await installPackWithUpdatePrompt("/src/mypack.aerinipkg", "/plugins", toast);

    expect(mocks.installPluginPackFromPath).toHaveBeenCalledTimes(1);
    expect(toast).not.toHaveBeenCalled();
  });

  it("surfaces a non-pack_already_installed rejection (e.g. pack_member_conflict) as a plain error toast, no prompt", async () => {
    mocks.installPluginPackFromPath.mockRejectedValueOnce(
      "pack_member_conflict: 'com.example.node' is already provided by another installed plugin or pack",
    );
    const { installPackWithUpdatePrompt } = await import("../plugin-settings");
    const toast = vi.fn();

    await installPackWithUpdatePrompt("/src/mypack.aerinipkg", "/plugins", toast);

    expect(mocks.showConfirm).not.toHaveBeenCalled();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("pack_member_conflict"), "error");
  });
});

describe("plugin-settings installPluginOrPackWithUpdatePrompt, extension dispatch", () => {
  beforeEach(() => {
    mocks.installPluginFromPath.mockReset();
    mocks.installPluginPackFromPath.mockReset();
    mocks.reloadPlugins.mockReset();
    mocks.reloadPlugins.mockResolvedValue({ loaded: [], builtin_rejected: [], plugin_collisions: [] });
    mocks.installPluginFromPath.mockResolvedValue("plugin.wasm");
    mocks.installPluginPackFromPath.mockResolvedValue("com.example.mypack");
  });

  it("dispatches a .wasm path to installPluginFromPath, not the pack command", async () => {
    const { installPluginOrPackWithUpdatePrompt } = await import("../plugin-settings");
    await installPluginOrPackWithUpdatePrompt("/src/plugin.wasm", "/plugins", vi.fn());

    expect(mocks.installPluginFromPath).toHaveBeenCalledWith("/src/plugin.wasm", "/plugins", false);
    expect(mocks.installPluginPackFromPath).not.toHaveBeenCalled();
  });

  it("dispatches a .aerinipkg path to installPluginPackFromPath, not the single-plugin command", async () => {
    const { installPluginOrPackWithUpdatePrompt } = await import("../plugin-settings");
    await installPluginOrPackWithUpdatePrompt("/src/mypack.aerinipkg", "/plugins", vi.fn());

    expect(mocks.installPluginPackFromPath).toHaveBeenCalledWith("/src/mypack.aerinipkg", "/plugins", false);
    expect(mocks.installPluginFromPath).not.toHaveBeenCalled();
  });

  it("dispatches by extension case-insensitively", async () => {
    const { installPluginOrPackWithUpdatePrompt } = await import("../plugin-settings");
    await installPluginOrPackWithUpdatePrompt("/src/MyPack.AERINIPKG", "/plugins", vi.fn());

    expect(mocks.installPluginPackFromPath).toHaveBeenCalledWith("/src/MyPack.AERINIPKG", "/plugins", false);
    expect(mocks.installPluginFromPath).not.toHaveBeenCalled();
  });
});

describe("plugin-settings bindPluginSettings, node-descriptor refresh callback", () => {
  beforeEach(() => {
    mocks.installPluginFromPath.mockReset();
    mocks.reloadPlugins.mockReset();
  });

  it("runs onNodesReloaded after an install whose reload actually swapped the registry", async () => {
    mocks.installPluginFromPath.mockResolvedValue("plugin.wasm");
    mocks.reloadPlugins.mockResolvedValue({ loaded: [], builtin_rejected: [], plugin_collisions: [] });
    const { bindPluginSettings, installWithUpdatePrompt } = await import("../plugin-settings");
    const onNodesReloaded = vi.fn();
    bindPluginSettings(vi.fn(), onNodesReloaded);

    await installWithUpdatePrompt("/src/plugin.wasm", "/plugins", vi.fn());

    expect(onNodesReloaded).toHaveBeenCalledTimes(1);
  });

  it("does not run onNodesReloaded when the post-install reload call itself fails", async () => {
    mocks.installPluginFromPath.mockResolvedValue("plugin.wasm");
    mocks.reloadPlugins.mockRejectedValue(new Error("engine busy"));
    const { bindPluginSettings, installWithUpdatePrompt } = await import("../plugin-settings");
    const onNodesReloaded = vi.fn();
    bindPluginSettings(vi.fn(), onNodesReloaded);

    await installWithUpdatePrompt("/src/plugin.wasm", "/plugins", vi.fn());

    expect(onNodesReloaded).not.toHaveBeenCalled();
  });
});

describe("plugin-settings signatureStatusClass", () => {
  it("classifies the trusted-publisher status as positive", async () => {
    const { signatureStatusClass } = await import("../plugin-settings");
    expect(signatureStatusClass("Verified — trusted publisher")).toBe("positive");
  });

  it("classifies any Unsigned-prefixed status as neutral", async () => {
    const { signatureStatusClass } = await import("../plugin-settings");
    expect(signatureStatusClass("Unsigned — no publisher signature")).toBe("neutral");
  });

  it("classifies every other status (unrecognized, malformed, mismatch, invalid, untrusted key) as a warning", async () => {
    const { signatureStatusClass } = await import("../plugin-settings");
    expect(signatureStatusClass("Signature present but in an unrecognized format")).toBe("warning");
    expect(signatureStatusClass("Signature file present but malformed")).toBe("warning");
    expect(signatureStatusClass("File does not match its signed checksum — may be corrupted")).toBe("warning");
    expect(signatureStatusClass("Signature present but does not verify")).toBe("warning");
    expect(signatureStatusClass("Verified signature, but from a different key than the one currently trusted for this plugin")).toBe("warning");
    expect(signatureStatusClass("Verified signature (not yet trusted — reinstall via Plugins tab to establish trust)")).toBe("warning");
  });
});
