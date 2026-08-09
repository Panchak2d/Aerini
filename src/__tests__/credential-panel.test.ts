// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";
import type { CredentialEntry, CredentialMetadata } from "../ipc/credentials";

const credMock = vi.hoisted(() => ({
  listCredentials:       vi.fn(),
  getCredentialMetadata: vi.fn(),
  getCredentialSecret:   vi.fn(),
  saveCredential:        vi.fn(),
  deleteCredential:      vi.fn(),
}));
vi.mock("../ipc/credentials", () => credMock);

const confirmMock = vi.hoisted(() => ({ showConfirm: vi.fn() }));
vi.mock("../confirm", () => ({ showConfirm: confirmMock.showConfirm }));

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
  credMock.getCredentialMetadata.mockReset().mockResolvedValue(null);
  credMock.getCredentialSecret.mockReset().mockResolvedValue(null);
  credMock.saveCredential.mockReset().mockResolvedValue(undefined);
  credMock.deleteCredential.mockReset().mockResolvedValue(undefined);
  confirmMock.showConfirm.mockReset().mockResolvedValue(true);
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
