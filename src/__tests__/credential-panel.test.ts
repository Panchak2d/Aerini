// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { CredentialEntry, CredentialMetadata } from "../ipc/credentials";

const credMock = vi.hoisted(() => ({
  listCredentials:       vi.fn(),
  listCredentialUsage:   vi.fn(),
  getCredentialMetadata: vi.fn(),
  saveCredential:        vi.fn(),
  deleteCredential:      vi.fn(),
  listCredentialModels:  vi.fn(),
  exportEncryptionKey:   vi.fn(),
}));
vi.mock("../ipc/credentials", () => credMock);

const confirmMock = vi.hoisted(() => ({ showConfirm: vi.fn() }));
vi.mock("../confirm", () => ({ showConfirm: confirmMock.showConfirm }));

const providersMock = vi.hoisted(() => ({
  listProviderModels: vi.fn(),
  probeOllama:        vi.fn(),
  pullOllamaModel:    vi.fn(),
  cancelOllamaPull:   vi.fn(),
}));
vi.mock("../ipc/providers", async () => {
  const actual = await vi.importActual<typeof import("../ipc/providers")>("../ipc/providers");
  return {
    ...actual,
    listProviderModels: providersMock.listProviderModels,
    probeOllama:        providersMock.probeOllama,
    pullOllamaModel:    providersMock.pullOllamaModel,
    cancelOllamaPull:   providersMock.cancelOllamaPull,
  };
});

import { CredentialPanel } from "../panels/CredentialPanel";

function entry(id: string, name: string, cred_type = "api_key"): CredentialEntry {
  return { id, name, cred_type };
}

async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  document.body.innerHTML = "";
  credMock.listCredentials.mockReset().mockResolvedValue([]);
  credMock.listCredentialUsage.mockReset().mockResolvedValue({});
  credMock.getCredentialMetadata.mockReset().mockResolvedValue(null);
  credMock.saveCredential.mockReset().mockResolvedValue(undefined);
  credMock.deleteCredential.mockReset().mockResolvedValue(undefined);
  credMock.listCredentialModels.mockReset().mockResolvedValue([]);
  credMock.exportEncryptionKey.mockReset().mockResolvedValue("KEY-BASE64");
  confirmMock.showConfirm.mockReset().mockResolvedValue(true);
  providersMock.listProviderModels.mockReset().mockResolvedValue([]);
  providersMock.probeOllama.mockReset().mockResolvedValue({ ollama: false, version: null, loopback: true, reason: "not_ollama" });
  providersMock.pullOllamaModel.mockReset();
  providersMock.cancelOllamaPull.mockReset().mockResolvedValue(undefined);
});

describe("CredentialPanel — scroll region structure", () => {
  it("keeps the Save button outside the scrolling body so it's always reachable", async () => {
    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    const body   = document.querySelector(".cred-panel-body")!;
    const footer = document.querySelector(".cred-panel-footer")!;
    expect(body.contains(document.getElementById("cred-save"))).toBe(false);
    expect(footer.contains(document.getElementById("cred-save"))).toBe(true);
  });

  it("puts the credential list and the add-credential form inside the single scrolling body", async () => {
    credMock.listCredentials.mockResolvedValue([entry("k1", "Key One")]);
    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    const body = document.querySelector(".cred-panel-body")!;
    expect(body.querySelector(".cred-list")).not.toBeNull();
    expect(body.querySelector(".cred-form")).not.toBeNull();
  });
});

describe("CredentialPanel — list rendering", () => {
  it("shows the empty state when there are no credentials", async () => {
    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    expect(document.querySelector(".cred-empty-state")).not.toBeNull();
    expect(document.querySelectorAll(".cred-item")).toHaveLength(0);
  });

  it("renders an Edit and a Delete button per credential", async () => {
    credMock.listCredentials.mockResolvedValue([entry("k1", "Key One"), entry("k2", "Key Two")]);
    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    expect(document.querySelectorAll(".cred-item-edit")).toHaveLength(2);
    expect(document.querySelectorAll(".cred-item-del")).toHaveLength(2);
  });
});

describe("CredentialPanel — usage visibility", () => {
  it("shows workflow count and names up front for a credential in use", async () => {
    credMock.listCredentials.mockResolvedValue([entry("k1", "Key One")]);
    credMock.listCredentialUsage.mockResolvedValue({ k1: ["Alpha Flow", "Beta Flow"] });

    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    const usageEl = document.querySelector(".cred-item-usage")!;
    expect(usageEl.textContent).toBe("Used by 2 workflows");
    expect(usageEl.classList.contains("unused")).toBe(false);
    expect(usageEl.getAttribute("title")).toBe("Alpha Flow, Beta Flow");
  });

  it("shows an unused state for a credential no workflow references", async () => {
    credMock.listCredentials.mockResolvedValue([entry("k1", "Key One")]);
    credMock.listCredentialUsage.mockResolvedValue({});

    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    const usageEl = document.querySelector(".cred-item-usage")!;
    expect(usageEl.textContent).toBe("Not used by any workflow");
    expect(usageEl.classList.contains("unused")).toBe(true);
  });
});

describe("CredentialPanel — Secret Value requirement", () => {
  it("blocks save with an empty Secret Value when Advanced Provider is blank", async () => {
    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    (document.getElementById("cred-name") as HTMLInputElement).value = "New Key";
    (document.getElementById("cred-id") as HTMLInputElement).value = "new_key";
    (document.getElementById("cred-save") as HTMLButtonElement).click();
    await flush();

    expect(credMock.saveCredential).not.toHaveBeenCalled();
    expect(document.getElementById("cred-save-error")?.textContent).toBe("Secret value is required");
  });

  it("still blocks an empty Secret Value for every other Advanced Provider value", async () => {
    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    (document.getElementById("cred-name") as HTMLInputElement).value = "New Key";
    (document.getElementById("cred-id") as HTMLInputElement).value = "new_key";
    (document.getElementById("cred-meta-provider") as HTMLInputElement).value = "openai";
    (document.getElementById("cred-save") as HTMLButtonElement).click();
    await flush();

    expect(credMock.saveCredential).not.toHaveBeenCalled();
    expect(document.getElementById("cred-save-error")?.textContent).toBe("Secret value is required");
  });

  it("allows an empty Secret Value on create when Advanced Provider is 'local'", async () => {
    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    (document.getElementById("cred-name") as HTMLInputElement).value = "Local Model";
    (document.getElementById("cred-id") as HTMLInputElement).value = "local_model";
    (document.getElementById("cred-meta-provider") as HTMLInputElement).value = "local";
    (document.getElementById("cred-save") as HTMLButtonElement).click();
    await flush();

    expect(credMock.saveCredential).toHaveBeenCalledWith(
      expect.objectContaining({ id: "local_model", value: "", provider: "local" }),
      false,
    );
  });
});

describe("CredentialPanel — re-edit a saved credential", () => {
  async function openEdit(): Promise<CredentialPanel> {
    credMock.listCredentials.mockResolvedValue([entry("k1", "Key One", "bearer")]);
    credMock.getCredentialMetadata.mockResolvedValue({ provider: "openai", model: "gpt-4o" } as CredentialMetadata);
    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    (document.querySelector(".cred-item-edit") as HTMLButtonElement).click();
    await flush();
    return panel;
  }

  it("never loads the secret: the Secret Value field is blank, masked and marked unchanged, and the ID is locked", async () => {
    await openEdit();

    const valueInp = document.getElementById("cred-value") as HTMLInputElement;
    const idInp    = document.getElementById("cred-id")    as HTMLInputElement;
    expect(valueInp.value).toBe("");
    expect(valueInp.type).toBe("password");
    expect(valueInp.placeholder).toContain("unchanged");
    expect(idInp.value).toBe("k1");
    expect(idInp.readOnly).toBe(true);
    expect((document.getElementById("cred-meta-provider") as HTMLInputElement).value).toBe("openai");
    expect(document.getElementById("cred-cancel")).not.toBeNull();
    expect(document.querySelector(".cred-item.editing")?.getAttribute("data-id")).toBe("k1");
  });

  it("saves an edit as an update that sends no secret unless one is typed, then leaves edit mode", async () => {
    await openEdit();

    (document.getElementById("cred-name") as HTMLInputElement).value = "Key One Renamed";
    (document.getElementById("cred-save") as HTMLButtonElement).click();
    await flush();
    expect(credMock.saveCredential).toHaveBeenLastCalledWith(
      expect.objectContaining({ id: "k1", name: "Key One Renamed", value: "" }),
      true,
    );
    expect(document.getElementById("cred-cancel")).toBeNull();

    (document.querySelector(".cred-item-edit") as HTMLButtonElement).click();
    await flush();
    (document.getElementById("cred-value") as HTMLInputElement).value = "rotated-secret";
    (document.getElementById("cred-save") as HTMLButtonElement).click();
    await flush();
    expect(credMock.saveCredential).toHaveBeenLastCalledWith(
      expect.objectContaining({ id: "k1", value: "rotated-secret" }),
      true,
    );
  });

  it("creating sends update=false", async () => {
    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    (document.getElementById("cred-name") as HTMLInputElement).value = "New Key";
    (document.getElementById("cred-id") as HTMLInputElement).value = "new_key";
    (document.getElementById("cred-value") as HTMLInputElement).value = "sk-new";
    (document.getElementById("cred-save") as HTMLButtonElement).click();
    await flush();
    expect(credMock.saveCredential).toHaveBeenCalledWith(
      expect.objectContaining({ id: "new_key", value: "sk-new" }),
      false,
    );
  });

  it("Cancel exits edit mode without saving", async () => {
    await openEdit();
    (document.getElementById("cred-cancel") as HTMLButtonElement).click();
    await flush();

    expect(credMock.saveCredential).not.toHaveBeenCalled();
    expect(document.getElementById("cred-cancel")).toBeNull();
    expect((document.getElementById("cred-value") as HTMLInputElement).value).toBe("");
  });

  it("ignores a second Edit click on another item while the first load is still in flight (race guard)", async () => {
    credMock.listCredentials.mockResolvedValue([entry("k1", "Key One"), entry("k2", "Key Two")]);
    let resolveFirst: (v: CredentialMetadata | null) => void;
    credMock.getCredentialMetadata.mockImplementation((id: string) =>
      id === "k1" ? new Promise<CredentialMetadata | null>(res => { resolveFirst = res; }) : Promise.resolve(null)
    );

    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    const [editK1, editK2] = Array.from(document.querySelectorAll<HTMLButtonElement>(".cred-item-edit"));
    editK1.click();
    await flush();
    editK2.click();
    await flush();

    expect(credMock.getCredentialMetadata).toHaveBeenCalledTimes(1);
    expect(credMock.getCredentialMetadata).toHaveBeenCalledWith("k1");

    resolveFirst!(null);
    await flush();
    expect((document.getElementById("cred-id") as HTMLInputElement).value).toBe("k1");
  });
});

describe("CredentialPanel — encryption key backup", () => {
  afterEach(() => { vi.useRealTimers(); });

  async function revealKey(): Promise<CredentialPanel> {
    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    (document.getElementById("cred-key-backup") as HTMLButtonElement).click();
    await flush();
    return panel;
  }

  it("hides the revealed key after 60 seconds, and immediately when the panel is hidden", async () => {
    vi.useFakeTimers();
    const panel = await revealKey();
    const slot = document.getElementById("cred-key-banner-slot")!;
    expect(slot.querySelector(".esp-run-secret-value")?.textContent).toBe("KEY-BASE64");

    vi.advanceTimersByTime(59_000);
    expect(slot.querySelector(".esp-run-secret-value")).not.toBeNull();
    vi.advanceTimersByTime(1_000);
    expect(slot.querySelector(".esp-run-secret-value")).toBeNull();

    (document.getElementById("cred-key-backup") as HTMLButtonElement).click();
    await flush();
    expect(slot.querySelector(".esp-run-secret-value")).not.toBeNull();
    panel.hide();
    expect(slot.querySelector(".esp-run-secret-value")).toBeNull();
  });

  it("shows nothing and no error when the native confirmation is declined", async () => {
    credMock.exportEncryptionKey.mockRejectedValue("CANCELLED");
    await revealKey();
    expect(document.querySelector(".esp-run-secret-value")).toBeNull();
    expect(confirmMock.showConfirm).not.toHaveBeenCalled();
  });

  it("clears the clipboard 60 seconds after the key was copied", async () => {
    vi.useFakeTimers();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText: vi.fn().mockResolvedValue("KEY-BASE64") },
    });
    await revealKey();
    (document.querySelector(".esp-run-secret-copy") as HTMLButtonElement).click();
    await flush();
    expect(writeText).toHaveBeenLastCalledWith("KEY-BASE64");

    await vi.advanceTimersByTimeAsync(60_000);
    expect(writeText).toHaveBeenLastCalledWith("");
  });
});

describe("CredentialPanel — Advanced Provider picker", () => {
  it("normal case: offers the four known providers, and clicking one fills the free-text Provider field", async () => {
    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    const providerInp = document.getElementById("cred-meta-provider") as HTMLInputElement;
    expect(providerInp.value).toBe("");

    const optionTexts = Array.from(document.querySelectorAll("#cred-provider-picker-slot .csel-option"))
      .map(o => o.textContent);
    expect(optionTexts).toEqual(["openai", "anthropic", "gemini", "local"]);

    const geminiOpt = Array.from(document.querySelectorAll("#cred-provider-picker-slot .csel-option"))
      .find(o => o.textContent === "gemini")!;
    geminiOpt.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));

    expect(providerInp.value).toBe("gemini");
  });

  it("edge case: picking 'local' reveals the Ollama Base URL quick-fill; any other provider does not", async () => {
    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    const baseUrlInp = document.getElementById("cred-meta-base-url") as HTMLInputElement;
    expect(document.querySelector("#cred-base-url-help-slot button")).toBeNull();

    const localOpt = Array.from(document.querySelectorAll("#cred-provider-picker-slot .csel-option"))
      .find(o => o.textContent === "local")!;
    localOpt.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));

    const fillBtn = document.querySelector("#cred-base-url-help-slot button") as HTMLButtonElement;
    expect(fillBtn?.textContent).toBe("Use Ollama defaults");
    fillBtn.click();
    expect(baseUrlInp.value).toBe("http://localhost:11434/v1");

    const openaiOpt = Array.from(document.querySelectorAll("#cred-provider-picker-slot .csel-option"))
      .find(o => o.textContent === "openai")!;
    openaiOpt.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(document.querySelector("#cred-base-url-help-slot button")).toBeNull();
  });
});

describe("CredentialPanel — Advanced 'Fetch Models'", () => {
  it("populates a dropdown on a successful fetch, reading Provider/Base URL/Secret Value from this form", async () => {
    providersMock.listProviderModels.mockResolvedValue(["gpt-4o", "gpt-4o-mini"]);

    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    (document.getElementById("cred-meta-provider") as HTMLInputElement).value = "openai";
    (document.getElementById("cred-meta-base-url") as HTMLInputElement).value = "https://api.openai.com/v1";
    (document.getElementById("cred-value") as HTMLInputElement).value = "sk-test";
    (document.getElementById("cred-fetch-models") as HTMLButtonElement).click();
    await flush();

    expect(providersMock.listProviderModels).toHaveBeenCalledWith("openai", "https://api.openai.com/v1", "sk-test");
    const options = document.querySelectorAll("#cred-model-list-slot .csel-option");
    expect(options.length).toBe(2);
    expect(Array.from(options).map(o => o.textContent)).toEqual(["gpt-4o", "gpt-4o-mini"]);
    expect((document.getElementById("cred-fetch-models") as HTMLButtonElement).disabled).toBe(false);

    options[0].dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect((document.getElementById("cred-meta-model") as HTMLInputElement).value).toBe("gpt-4o");
  });

  it("degrades to the plain text field on a fetch error, without throwing, and shows the backend's specific reason", async () => {
    providersMock.listProviderModels.mockRejectedValue(new Error("MISSING_API_KEY: Anthropic requires an API key"));

    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    const modelInp = document.getElementById("cred-meta-model") as HTMLInputElement;
    modelInp.value = "typed-manually";
    const fetchBtn = document.getElementById("cred-fetch-models") as HTMLButtonElement;
    fetchBtn.click();
    await flush();

    expect(fetchBtn.textContent).toBe("Couldn't fetch — try again");
    expect(fetchBtn.disabled).toBe(false);
    expect(modelInp.value).toBe("typed-manually");
    expect(document.querySelector("#cred-model-list-slot .csel-option")).toBeNull();

    const errHint = document.querySelector("#cred-model-list-slot .config-hint-warn") as HTMLElement;
    expect(errHint?.textContent).toBe("Anthropic requires an API key");
  });

  it("degrades silently on an empty model list, same as any other fetch failure", async () => {
    providersMock.listProviderModels.mockResolvedValue([]);

    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    (document.getElementById("cred-fetch-models") as HTMLButtonElement).click();
    await flush();

    expect(document.getElementById("cred-fetch-models")?.textContent).toBe("Couldn't fetch — try again");
    expect(document.querySelector("#cred-model-list-slot .csel-option")).toBeNull();
  });

  it("while editing with a blank Secret Value, asks the backend to use the saved secret", async () => {
    credMock.listCredentials.mockResolvedValue([entry("k1", "Key One")]);
    credMock.listCredentialModels.mockResolvedValue(["gpt-4o"]);
    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    (document.querySelector(".cred-item-edit") as HTMLButtonElement).click();
    await flush();

    (document.getElementById("cred-meta-provider") as HTMLInputElement).value = "openai";
    (document.getElementById("cred-fetch-models") as HTMLButtonElement).click();
    await flush();

    expect(credMock.listCredentialModels).toHaveBeenCalledWith("k1", "openai", "");
    expect(providersMock.listProviderModels).not.toHaveBeenCalled();
  });

  it("resolves a blank Provider to 'auto', matching the node popover's own default", async () => {
    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    (document.getElementById("cred-fetch-models") as HTMLButtonElement).click();
    await flush();

    expect(providersMock.listProviderModels).toHaveBeenCalledWith("auto", "", "");
  });
});

describe("CredentialPanel — Ollama pull/delete control", () => {
  const OLLAMA = { ollama: true, version: "0.30.0", loopback: true, reason: null };

  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  async function openPanel(): Promise<CredentialPanel> {
    const panel = new CredentialPanel();
    await panel.show();
    await vi.advanceTimersByTimeAsync(0);
    return panel;
  }

  async function typeInto(id: string, value: string): Promise<void> {
    const inp = document.getElementById(id) as HTMLInputElement;
    inp.value = value;
    inp.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(300);
  }

  const ctl = () => document.querySelector<HTMLElement>("#cred-ollama-slot .ollama-ctl");
  const pullBtn = () => document.querySelector<HTMLButtonElement>("#cred-ollama-slot .ollama-ctl-row button")!;

  it("stays hidden for a cloud provider without probing, and appears for local once the probe says Ollama", async () => {
    providersMock.probeOllama.mockResolvedValue(OLLAMA);
    await openPanel();
    await typeInto("cred-meta-provider", "openai");
    await typeInto("cred-meta-base-url", "https://api.openai.com/v1");
    expect(providersMock.probeOllama).not.toHaveBeenCalled();
    expect(ctl()?.hidden).toBe(true);

    await typeInto("cred-meta-provider", "local");
    await typeInto("cred-meta-base-url", "http://localhost:11434/v1");
    expect(providersMock.probeOllama).toHaveBeenLastCalledWith({ provider: "local", base_url: "http://localhost:11434/v1" });
    expect(ctl()?.hidden).toBe(false);
  });

  it("stays hidden when the server does not answer as Ollama", async () => {
    await openPanel();
    await typeInto("cred-meta-provider", "local");
    await typeInto("cred-meta-base-url", "http://localhost:1234/v1");
    expect(providersMock.probeOllama).toHaveBeenCalled();
    expect(ctl()?.hidden).toBe(true);
  });

  it("targets the saved credential while editing with a blank Secret Value, and the typed secret otherwise", async () => {
    providersMock.probeOllama.mockResolvedValue(OLLAMA);
    credMock.listCredentials.mockResolvedValue([entry("k1", "Local")]);
    credMock.getCredentialMetadata.mockResolvedValue({ provider: "local", model: "llama3.2", base_url: "http://localhost:11434/v1" });
    await openPanel();
    document.querySelector<HTMLButtonElement>(".cred-item-edit")!.click();
    await vi.advanceTimersByTimeAsync(300);
    expect(providersMock.probeOllama).toHaveBeenLastCalledWith({ provider: "local", base_url: "http://localhost:11434/v1", credential_id: "k1" });

    await typeInto("cred-value", "typed-secret");
    expect(providersMock.probeOllama).toHaveBeenLastCalledWith({ provider: "local", base_url: "http://localhost:11434/v1", api_key: "typed-secret" });
  });

  it("cancels a running pull when the panel is hidden", async () => {
    providersMock.probeOllama.mockResolvedValue(OLLAMA);
    providersMock.pullOllamaModel.mockReturnValue(new Promise(() => {}));
    const panel = await openPanel();
    await typeInto("cred-meta-provider", "local");
    await typeInto("cred-meta-base-url", "http://localhost:11434/v1");
    await typeInto("cred-meta-model", "llama3.2");
    pullBtn().click();
    await vi.advanceTimersByTimeAsync(0);
    const pullId = providersMock.pullOllamaModel.mock.calls[0][0];

    panel.hide();
    expect(providersMock.cancelOllamaPull).toHaveBeenCalledWith(pullId);
  });

  it("lists models again after a pull completes", async () => {
    providersMock.probeOllama.mockResolvedValue(OLLAMA);
    providersMock.pullOllamaModel.mockResolvedValue("done");
    providersMock.listProviderModels.mockResolvedValue(["llama3.2:latest"]);
    await openPanel();
    await typeInto("cred-meta-provider", "local");
    await typeInto("cred-meta-base-url", "http://localhost:11434/v1");
    await typeInto("cred-meta-model", "llama3.2");
    pullBtn().click();
    await vi.advanceTimersByTimeAsync(50);
    expect(providersMock.listProviderModels).toHaveBeenCalledWith("local", "http://localhost:11434/v1", "");
    expect(document.querySelector("#cred-model-list-slot")?.textContent).toContain("1 model found");
  });
});
