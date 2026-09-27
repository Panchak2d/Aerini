// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";
import type { CredentialEntry, CredentialMetadata } from "../ipc/credentials";

const credMock = vi.hoisted(() => ({
  listCredentials:       vi.fn(),
  listCredentialUsage:   vi.fn(),
  getCredentialMetadata: vi.fn(),
  getCredentialSecret:   vi.fn(),
  saveCredential:        vi.fn(),
  deleteCredential:      vi.fn(),
}));
vi.mock("../ipc/credentials", () => credMock);

const confirmMock = vi.hoisted(() => ({ showConfirm: vi.fn() }));
vi.mock("../confirm", () => ({ showConfirm: confirmMock.showConfirm }));

const providersMock = vi.hoisted(() => ({ listProviderModels: vi.fn() }));
vi.mock("../ipc/providers", async () => {
  const actual = await vi.importActual<typeof import("../ipc/providers")>("../ipc/providers");
  return { ...actual, listProviderModels: providersMock.listProviderModels };
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
  credMock.getCredentialSecret.mockReset().mockResolvedValue(null);
  credMock.saveCredential.mockReset().mockResolvedValue(undefined);
  credMock.deleteCredential.mockReset().mockResolvedValue(undefined);
  confirmMock.showConfirm.mockReset().mockResolvedValue(true);
  providersMock.listProviderModels.mockReset().mockResolvedValue([]);
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

  it("allows an empty Secret Value when Advanced Provider is 'local'", async () => {
    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    (document.getElementById("cred-name") as HTMLInputElement).value = "Local Model";
    (document.getElementById("cred-id") as HTMLInputElement).value = "local_model";
    (document.getElementById("cred-meta-provider") as HTMLInputElement).value = "local";
    (document.getElementById("cred-save") as HTMLButtonElement).click();
    await flush();

    expect(credMock.saveCredential).toHaveBeenCalledWith(
      expect.objectContaining({ id: "local_model", value: "", provider: "local" })
    );
  });
});

describe("CredentialPanel — view / re-edit a saved credential", () => {
  it("loads the decrypted secret into a masked (password) field, and locks the ID field", async () => {
    credMock.listCredentials.mockResolvedValue([entry("k1", "Key One", "bearer")]);
    credMock.getCredentialSecret.mockResolvedValue("sk-super-secret");
    credMock.getCredentialMetadata.mockResolvedValue({ provider: "openai", model: "gpt-4o" } as CredentialMetadata);

    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    (document.querySelector(".cred-item-edit") as HTMLButtonElement).click();
    await flush();

    expect(credMock.getCredentialSecret).toHaveBeenCalledWith("k1");
    const valueInp = document.getElementById("cred-value") as HTMLInputElement;
    const idInp    = document.getElementById("cred-id")    as HTMLInputElement;
    expect(valueInp.value).toBe("sk-super-secret");
    expect(valueInp.type).toBe("password");
    expect(idInp.value).toBe("k1");
    expect(idInp.readOnly).toBe(true);
    expect((document.getElementById("cred-meta-provider") as HTMLInputElement).value).toBe("openai");
    expect(document.getElementById("cred-cancel")).not.toBeNull();
    expect(document.querySelector(".cred-item.editing")?.getAttribute("data-id")).toBe("k1");
  });

  it("saves an edit as an update to the same ID, then leaves edit mode", async () => {
    credMock.listCredentials.mockResolvedValue([entry("k1", "Key One")]);
    credMock.getCredentialSecret.mockResolvedValue("old-secret");

    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    (document.querySelector(".cred-item-edit") as HTMLButtonElement).click();
    await flush();

    (document.getElementById("cred-name") as HTMLInputElement).value = "Key One Renamed";
    (document.getElementById("cred-save") as HTMLButtonElement).click();
    await flush();

    expect(credMock.saveCredential).toHaveBeenCalledWith(
      expect.objectContaining({ id: "k1", name: "Key One Renamed", value: "old-secret" })
    );
    expect(document.getElementById("cred-cancel")).toBeNull();
    expect(document.querySelector(".cred-add-title")?.textContent).not.toContain("Editing");
  });

  it("Cancel exits edit mode without saving", async () => {
    credMock.listCredentials.mockResolvedValue([entry("k1", "Key One")]);
    credMock.getCredentialSecret.mockResolvedValue("old-secret");

    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    (document.querySelector(".cred-item-edit") as HTMLButtonElement).click();
    await flush();
    (document.getElementById("cred-cancel") as HTMLButtonElement).click();
    await flush();

    expect(credMock.saveCredential).not.toHaveBeenCalled();
    expect(document.getElementById("cred-cancel")).toBeNull();
    expect((document.getElementById("cred-value") as HTMLInputElement).value).toBe("");
  });

  it("ignores a second Edit click on another item while the first load is still in flight (race guard)", async () => {
    credMock.listCredentials.mockResolvedValue([entry("k1", "Key One"), entry("k2", "Key Two")]);
    let resolveFirst: (v: string) => void;
    credMock.getCredentialSecret.mockImplementation((id: string) =>
      id === "k1" ? new Promise<string>(res => { resolveFirst = res; }) : Promise.resolve("k2-secret")
    );

    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    const [editK1, editK2] = Array.from(document.querySelectorAll<HTMLButtonElement>(".cred-item-edit"));
    editK1.click();
    await flush();
    editK2.click();
    await flush();

    expect(credMock.getCredentialSecret).toHaveBeenCalledTimes(1);
    expect(credMock.getCredentialSecret).toHaveBeenCalledWith("k1");

    resolveFirst!("k1-secret");
    await flush();
    expect((document.getElementById("cred-id") as HTMLInputElement).value).toBe("k1");
  });

  it("if the secret can no longer be loaded (e.g. deleted meanwhile), warns and does not enter edit mode", async () => {
    credMock.listCredentials.mockResolvedValue([entry("k1", "Key One")]);
    credMock.getCredentialSecret.mockResolvedValue(null);

    const panel = new CredentialPanel();
    await panel.show();
    await flush();
    (document.querySelector(".cred-item-edit") as HTMLButtonElement).click();
    await flush();

    expect(confirmMock.showConfirm).toHaveBeenCalled();
    expect(document.getElementById("cred-cancel")).toBeNull();
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

    // The whole point of this fix: the CODE: prefix is stripped so the
    // reader sees the reason, not the raw backend error string.
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

  it("resolves a blank Provider to 'auto', matching the node popover's own default", async () => {
    const panel = new CredentialPanel();
    await panel.show();
    await flush();

    (document.getElementById("cred-fetch-models") as HTMLButtonElement).click();
    await flush();

    expect(providersMock.listProviderModels).toHaveBeenCalledWith("auto", "", "");
  });
});
