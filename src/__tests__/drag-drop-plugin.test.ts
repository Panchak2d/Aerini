/* @vitest-environment jsdom */

import { describe, it, expect, vi, beforeEach } from "vitest";

const mocks = vi.hoisted(() => ({
  getSetting: vi.fn(),
  showConfirm: vi.fn(),
  installPluginOrPackWithUpdatePrompt: vi.fn(),
}));
vi.mock("../ipc/workflow", () => ({
  getSetting: mocks.getSetting,
}));
vi.mock("../confirm", () => ({
  showConfirm: mocks.showConfirm,
}));
vi.mock("../plugin-settings", () => ({
  installPluginOrPackWithUpdatePrompt: mocks.installPluginOrPackWithUpdatePrompt,
}));

describe("drag-drop handlePluginDrop", () => {
  beforeEach(() => {
    mocks.getSetting.mockReset();
    mocks.showConfirm.mockReset();
    mocks.installPluginOrPackWithUpdatePrompt.mockReset();
  });

  it("prompts to set a plugin directory first when none is set, and never installs", async () => {
    mocks.getSetting.mockResolvedValue(null);
    const { handlePluginDrop } = await import("../drag-drop");
    const toast = vi.fn();

    await handlePluginDrop("/src/plugin.wasm", toast);

    expect(toast).toHaveBeenCalledWith(expect.stringContaining("plugin directory"), "info");
    expect(mocks.showConfirm).not.toHaveBeenCalled();
    expect(mocks.installPluginOrPackWithUpdatePrompt).not.toHaveBeenCalled();
  });

  it("confirms, then dispatches a dropped .wasm path through installPluginOrPackWithUpdatePrompt", async () => {
    mocks.getSetting.mockResolvedValue("/plugins");
    mocks.showConfirm.mockResolvedValue(true);
    const { handlePluginDrop } = await import("../drag-drop");
    const toast = vi.fn();

    await handlePluginDrop("/src/plugin.wasm", toast);

    expect(mocks.showConfirm.mock.calls[0][0]).toContain("plugin.wasm");
    expect(mocks.installPluginOrPackWithUpdatePrompt).toHaveBeenCalledWith("/src/plugin.wasm", "/plugins", toast);
  });

  it("confirms, then dispatches a dropped .aerinipkg path through installPluginOrPackWithUpdatePrompt unchanged", async () => {
    mocks.getSetting.mockResolvedValue("/plugins");
    mocks.showConfirm.mockResolvedValue(true);
    const { handlePluginDrop } = await import("../drag-drop");
    const toast = vi.fn();

    await handlePluginDrop("/src/mypack.aerinipkg", toast);

    expect(mocks.showConfirm.mock.calls[0][0]).toContain("mypack.aerinipkg");
    expect(mocks.installPluginOrPackWithUpdatePrompt).toHaveBeenCalledWith("/src/mypack.aerinipkg", "/plugins", toast);
  });

  it("does not install when the trust confirmation is declined", async () => {
    mocks.getSetting.mockResolvedValue("/plugins");
    mocks.showConfirm.mockResolvedValue(false);
    const { handlePluginDrop } = await import("../drag-drop");
    const toast = vi.fn();

    await handlePluginDrop("/src/plugin.wasm", toast);

    expect(mocks.installPluginOrPackWithUpdatePrompt).not.toHaveBeenCalled();
  });
});
