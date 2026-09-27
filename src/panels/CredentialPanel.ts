import {
  listCredentials,
  listCredentialUsage,
  getCredentialMetadata,
  getCredentialSecret,
  saveCredential,
  deleteCredential,
  exportEncryptionKey,
} from "../ipc/credentials";
import { listProviderModels, describeModelFetchError } from "../ipc/providers";
import { mkCustomSelect } from "../node-configs/popover-utils";
import { showConfirm } from "../confirm";
import { escapeHtml as escHtml } from "../utils";

interface Credential { id: string; name: string; cred_type: string; }

interface CredType { value: string; label: string; hint: string; placeholder: string; }

const CRED_TYPES: CredType[] = [
  { value: "api_key", label: "API Key",        hint: "A plain API key passed as a header or query param", placeholder: "sk-… or your API key"       },
  { value: "bearer",  label: "Bearer Token",   hint: "Will be sent as Authorization: Bearer <value>",     placeholder: "eyJ… or your token"          },
  { value: "basic",   label: "Basic Auth",     hint: "Enter as username:password",                        placeholder: "username:password"           },
  { value: "oauth",   label: "OAuth Token",    hint: "An OAuth access or refresh token",                  placeholder: "ya29.… or your OAuth token"  },
  { value: "other",   label: "Other / Custom", hint: "Any custom secret value",                           placeholder: "Your secret value"           },
];

// Prefill snapshot loaded into the form when re-opening a saved credential
// for editing. `value` is the decrypted secret — held in memory only for the
// life of this edit, same as anything else typed into the form.
interface EditingCredential {
  id: string;
  name: string;
  cred_type: string;
  value: string;
  provider: string;
  model: string;
  base_url: string;
}

export class CredentialPanel {
  private el: HTMLElement;
  private creds: Credential[] = [];
  private usage: Record<string, string[]> = {};
  private editing: EditingCredential | null = null;
  // Guards the async gap in startEdit/delete (before any confirm dialog is
  // up) — without it, clicking Edit on a second item while the first is
  // still loading can let whichever IPC call resolves last silently
  // clobber the edit session the user is actually looking at.
  private busy = false;

  constructor() {
    this.el = document.getElementById("cred-panel")!;
    if (!this.el) {
      this.el = document.createElement("div");
      this.el.id = "cred-panel";
      this.el.className = "cred-panel hidden";
      document.body.appendChild(this.el);
    }
  }

  async show(): Promise<void> {
    this.editing = null;
    this.el.classList.remove("hidden");
    await this.refresh();
  }

  hide(): void { this.el.classList.add("hidden"); }

  private async refresh(): Promise<void> {
    const [creds, usage] = await Promise.all([
      listCredentials().catch(() => []),
      listCredentialUsage().catch(() => ({})),
    ]);
    this.creds = creds;
    this.usage = usage;
    this.render();
  }

  private async backupEncryptionKey(): Promise<void> {
    if (this.busy) return;
    const ok = await showConfirm(
      "This reveals the master key that encrypts every credential saved here. " +
      "Anyone who gets it can decrypt all of them. Store it somewhere as secure " +
      "as the credentials themselves — a password manager, not a plain text file " +
      "on this machine. It will only be shown this once per click.",
      true,
      "Show Key",
    );
    if (!ok) return;

    this.busy = true;
    const btn = this.el.querySelector<HTMLButtonElement>("#cred-key-backup")!;
    btn.disabled = true;
    const originalLabel = btn.textContent;
    btn.textContent = "Loading…";
    try {
      const key = await exportEncryptionKey();
      this.renderKeyBanner(key);
    } catch (e) {
      await showConfirm(`Could not read the encryption key: ${e}`);
    } finally {
      btn.disabled = false;
      btn.textContent = originalLabel;
      this.busy = false;
    }
  }

  private renderKeyBanner(key: string): void {
    const slot = this.el.querySelector("#cred-key-banner-slot");
    if (!slot) return;
    slot.innerHTML = `
      <div class="esp-run-secret-banner">
        <div class="esp-run-secret-title">⚠ Your encryption key — save it now</div>
        <div class="esp-run-secret-desc">
          If the OS keychain entry holding this is ever lost, this is the <strong>only</strong> way
          to recover every credential saved above. To restore: stop the app, place this exact value
          in the key file the app expects (see <code>docs/credentials.md</code>), then restart —
          it's picked up automatically.
        </div>
        <div class="esp-run-secret-row">
          <code class="esp-run-secret-value">${escHtml(key)}</code>
          <button class="esp-run-secret-copy" type="button">Copy</button>
        </div>
      </div>`;

    const copyBtn = slot.querySelector<HTMLButtonElement>(".esp-run-secret-copy")!;
    copyBtn.addEventListener("click", () => {
      navigator.clipboard.writeText(key).then(() => {
        copyBtn.textContent = "Copied!";
        setTimeout(() => { copyBtn.textContent = "Copy"; }, 2000);
      }).catch(() => {
        copyBtn.textContent = "Copy failed";
        setTimeout(() => { copyBtn.textContent = "Copy"; }, 2000);
      });
    });
  }

  private render(): void {
    const isEmpty = this.creds.length === 0;
    this.el.innerHTML = `
      <div class="cred-panel-backdrop"></div>
      <div class="cred-panel-box">
        <div class="cred-panel-header">
          <div>
            <div class="cred-panel-title">Credentials</div>
            <div class="cred-panel-subtitle">API keys and service credentials — stored encrypted on your device. Never sent anywhere.</div>
          </div>
          <button class="cred-panel-close" id="cred-close" aria-label="Close">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
          </button>
        </div>

        <div class="cred-panel-body" id="cred-panel-body">
          <div class="cred-key-backup-row">
            <button class="cred-key-backup-btn" id="cred-key-backup" type="button">Backup Encryption Key</button>
            <span class="cred-key-backup-hint">Recover access if the OS keychain entry is ever lost — read this before you need it.</span>
          </div>
          <div id="cred-key-banner-slot"></div>
          ${isEmpty ? `
          <div class="cred-empty-state">
            <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" opacity="0.4"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg>
            <div class="cred-empty-state-title">No credentials yet</div>
            <div class="cred-empty-state-desc">Add your first API key or service credential below. Credentials are used by nodes like HTTP Request, AI Prompt, and Send Email.</div>
          </div>` : `
          <div class="cred-list" id="cred-list">
            ${this.renderList()}
          </div>`}

          <div class="cred-add-section${this.editing ? " is-editing" : ""}">
            <div class="cred-add-header">
              <div class="cred-add-title">${this.formTitle()}</div>
            </div>
            <div class="cred-form" id="cred-form">
              ${this.renderForm()}
            </div>
          </div>
        </div>

        <div class="cred-panel-footer">
          <div class="cred-save-error hidden" id="cred-save-error"></div>
          <div class="cred-footer-actions">
            ${this.editing ? `<button type="button" class="cred-cancel-btn" id="cred-cancel">Cancel</button>` : ""}
            <button class="btn-primary cred-save-btn" id="cred-save">${this.editing ? "Save Changes" : "Save Credential"}</button>
          </div>
        </div>
      </div>`;

    this.el.querySelector("#cred-close")!.addEventListener("click", () => this.hide());
    this.el.querySelector(".cred-panel-backdrop")!.addEventListener("click", () => this.hide());
    this.el.querySelector("#cred-key-backup")!.addEventListener("click", () => this.backupEncryptionKey());
    this.bindForm();
    this.bindList();
  }

  private formTitle(): string {
    if (this.editing) return `Editing — ${escHtml(this.editing.name)}`;
    return this.creds.length === 0 ? "Add your first credential" : "Add a credential";
  }

  private renderList(): string {
    if (!this.creds.length) {
      return `<div class="cred-empty">
        No credentials saved yet. Add your first API key below.
      </div>`;
    }
    return this.creds.map(c => {
      const names = this.usage[c.id] ?? [];
      const usageLabel = names.length
        ? `Used by ${names.length} workflow${names.length === 1 ? "" : "s"}`
        : "Not used by any workflow";
      const usageTitle = names.length ? names.join(", ") : usageLabel;
      return `
      <div class="cred-item${this.editing?.id === c.id ? " editing" : ""}" data-id="${escHtml(c.id)}">
        <div class="cred-item-icon"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="7.5" cy="15.5" r="3.5"/><path d="M21 2l-9.6 9.6"/><path d="M15.5 7.5l3 3L22 7l-3-3"/></svg></div>
        <div class="cred-item-info">
          <div class="cred-item-name" title="${escHtml(c.name)}">${escHtml(c.name)}</div>
          <div class="cred-item-id" title="${escHtml(c.id)}">${escHtml(c.id)}</div>
          <div class="cred-item-usage${names.length ? "" : " unused"}" title="${escHtml(usageTitle)}" data-tooltip="${escHtml(usageTitle)}">${escHtml(usageLabel)}</div>
        </div>
        <span class="cred-item-type">${escHtml(credTypeLabel(c.cred_type))}</span>
        <div class="cred-item-actions">
          <button class="cred-item-edit" data-id="${escHtml(c.id)}" title="View / edit this credential">Edit</button>
          <button class="cred-item-del" data-id="${escHtml(c.id)}" title="Delete this credential">Delete</button>
        </div>
      </div>`;
    }).join("");
  }

  private renderForm(): string {
    const e = this.editing;
    const activeType = CRED_TYPES.find(t => t.value === e?.cred_type) ?? CRED_TYPES[0];
    return `
      <div class="field-group">
        <label class="field-label">Type</label>
        <div id="cred-type-wrap"></div>
        <div class="field-hint" id="cred-type-hint">${escHtml(activeType.hint)}</div>
      </div>
      <div class="field-group">
        <label class="field-label">Name</label>
        <input id="cred-name" type="text" placeholder="e.g. OpenAI Production Key" autocomplete="off" value="${escHtml(e?.name ?? "")}" />
        <div class="field-hint">A label to identify this credential in the UI</div>
      </div>
      <div class="field-group">
        <label class="field-label">ID / Key</label>
        <input id="cred-id" type="text" placeholder="e.g. openai_prod (no spaces)" autocomplete="off" value="${escHtml(e?.id ?? "")}" ${e ? "readonly" : ""} />
        <div class="field-hint">${e ? "ID can't be changed after creation — delete and re-add to use a different one" : "Short identifier used in nodes — auto-filled from name"}</div>
      </div>
      <div class="field-group">
        <label class="field-label">Secret Value</label>
        <div class="cred-secret-wrap">
          <input id="cred-value" type="password" placeholder="${escHtml(activeType.placeholder)}" autocomplete="new-password" value="${escHtml(e?.value ?? "")}" />
          <button type="button" class="cred-show-btn" id="cred-show" title="Show / hide" data-tooltip="Show / hide" aria-label="Show or hide secret value">
            <svg id="cred-eye-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>
            </svg>
          </button>
        </div>
        <div class="field-hint">Stored encrypted on your device. Never sent to Aerini servers.</div>
      </div>
      <details class="cred-advanced"${e && (e.provider || e.model || e.base_url) ? " open" : ""}>
        <summary class="cred-advanced-summary">Advanced (optional) — provider, model, base URL</summary>
        <div class="field-group">
          <label class="field-label">Provider</label>
          <div class="field-multiline-wrap">
            <input id="cred-meta-provider" type="text" placeholder="e.g. openai, anthropic, gemini" autocomplete="off" value="${escHtml(e?.provider ?? "")}" />
            <div id="cred-provider-picker-slot"></div>
          </div>
          <div class="field-hint">Pick a known provider, or type any other value (a proxy or self-hosted alias) — the field always stays editable.</div>
        </div>
        <div class="field-group">
          <label class="field-label">Model</label>
          <div class="field-multiline-wrap">
            <input id="cred-meta-model" type="text" placeholder="e.g. gpt-4o, claude-sonnet-4-6" autocomplete="off" value="${escHtml(e?.model ?? "")}" />
            <button type="button" class="code-load-btn" id="cred-fetch-models">Fetch Models</button>
            <div id="cred-model-list-slot"></div>
          </div>
        </div>
        <div class="field-group">
          <label class="field-label">Base URL</label>
          <div class="field-multiline-wrap">
            <input id="cred-meta-base-url" type="text" placeholder="Leave blank for provider default" autocomplete="off" value="${escHtml(e?.base_url ?? "")}" />
            <div id="cred-base-url-help-slot"></div>
          </div>
        </div>
        <div class="field-hint">Not secret — used to auto-fill matching fields on AI nodes when this credential is selected.</div>
      </details>`;
  }

  private bindForm(): void {
    const saveBtn  = this.el.querySelector("#cred-save")     as HTMLButtonElement;
    const cancelBtn = this.el.querySelector("#cred-cancel")  as HTMLButtonElement | null;
    const errDiv   = this.el.querySelector("#cred-save-error") as HTMLElement;
    const nameInp  = this.el.querySelector("#cred-name")     as HTMLInputElement;
    const idInp    = this.el.querySelector("#cred-id")       as HTMLInputElement;
    const valInp   = this.el.querySelector("#cred-value")    as HTMLInputElement;
    const hintEl   = this.el.querySelector("#cred-type-hint") as HTMLElement;
    const typeWrap = this.el.querySelector("#cred-type-wrap") as HTMLElement;
    const showBtn  = this.el.querySelector("#cred-show")     as HTMLButtonElement;
    const eyeIcon  = this.el.querySelector("#cred-eye-icon") as SVGElement;
    const providerInp = this.el.querySelector("#cred-meta-provider") as HTMLInputElement;
    const modelInp    = this.el.querySelector("#cred-meta-model")    as HTMLInputElement;
    const baseUrlInp  = this.el.querySelector("#cred-meta-base-url") as HTMLInputElement;
    const fetchModelsBtn = this.el.querySelector("#cred-fetch-models")   as HTMLButtonElement;
    const modelListSlot  = this.el.querySelector("#cred-model-list-slot") as HTMLElement;
    const providerPickerSlot = this.el.querySelector("#cred-provider-picker-slot") as HTMLElement;

    // Build custom select — avoids WebKitGTK Linux native <select> rendering bug
    let selectedCredType = this.editing?.cred_type ?? CRED_TYPES[0].value;
    typeWrap.appendChild(buildCredTypeSelect(CRED_TYPES, (t) => {
      hintEl.textContent = t.hint;
      valInp.placeholder = t.placeholder;
      selectedCredType   = t.value;
    }, selectedCredType));

    // Provider is deliberately free text, not a closed enum — this metadata
    // has to accept any string (a proxy alias, a self-hosted name), not just
    // the built-in ids (see formatProviderLabel in field-renderer.ts). This
    // picker is a convenience layer on top, same pattern as the Model field
    // below: click to fill the four built-in ids, or keep typing anything
    // else — the input stays the actual, authoritative value either way.
    const KNOWN_PROVIDERS = ["openai", "anthropic", "gemini", "local"];
    function refreshProviderPicker(): void {
      providerPickerSlot.innerHTML = "";
      providerPickerSlot.appendChild(mkCustomSelect(KNOWN_PROVIDERS, providerInp.value.trim(), (v) => {
        providerInp.value = v;
        refreshProviderPicker();
        refreshBaseUrlHelp();
      }));
    }
    refreshProviderPicker();

    // "local" has no cloud default by design (any OpenAI-compatible local
    // server, any port) — leaving Base URL blank under it resolves to an
    // empty string and Fetch Models (and the node itself) fails immediately.
    // That's correct, but the generic placeholder below actively told users
    // to do the one thing guaranteed to break it. Ollama is the common case
    // (the only local backend with its own sidebar preset), so swap in a
    // concrete example and a one-click fill once Provider is actually
    // "local"; any other local server still just needs its own URL typed in.
    const OLLAMA_DEFAULT_BASE_URL = "http://localhost:11434/v1";
    const GENERIC_BASE_URL_PLACEHOLDER = "Leave blank for provider default";
    const baseUrlHelpSlot = this.el.querySelector("#cred-base-url-help-slot") as HTMLElement;
    function refreshBaseUrlHelp(): void {
      const isLocal = providerInp.value.trim() === "local";
      baseUrlInp.placeholder = isLocal ? `e.g. ${OLLAMA_DEFAULT_BASE_URL} (Ollama)` : GENERIC_BASE_URL_PLACEHOLDER;
      baseUrlHelpSlot.innerHTML = "";
      if (!isLocal) return;
      const fillBtn = document.createElement("button");
      fillBtn.type = "button";
      fillBtn.className = "code-load-btn";
      fillBtn.textContent = "Use Ollama defaults";
      fillBtn.addEventListener("click", () => { baseUrlInp.value = OLLAMA_DEFAULT_BASE_URL; });
      baseUrlHelpSlot.appendChild(fillBtn);
    }
    refreshBaseUrlHelp();
    // Keeps both the picker's own label and the Base URL help in sync with
    // manual typing too, not just picker clicks — rebuilding a plain select
    // on every keystroke is cheap (DOM construction only, no network).
    providerInp.addEventListener("input", () => { refreshProviderPicker(); refreshBaseUrlHelp(); });

    // Auto-slug ID from name — skipped entirely while editing, since the ID
    // field is read-only and re-deriving it would fight the locked value.
    let idEdited = !!this.editing;
    idInp.addEventListener("input", () => { idEdited = true; });
    nameInp.addEventListener("input", () => {
      if (!idEdited) {
        idInp.value = nameInp.value.trim()
          .toLowerCase()
          .replace(/[^a-z0-9]+/g, "_")
          .replace(/^_+|_+$/g, "")
          .slice(0, 40);
      }
    });

    // Show/hide secret
    showBtn.addEventListener("click", () => {
      const shown = valInp.type === "text";
      valInp.type = shown ? "password" : "text";
      eyeIcon.innerHTML = shown
        ? `<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>`
        : `<path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"/>` +
          `<path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"/>` +
          `<line x1="1" y1="1" x2="23" y2="23"/>`;
    });

    // Mirrors the node popover's model picker: free-text Model is never
    // blocked, success offers a dropdown on top of it, any failure silently
    // leaves the text field alone. Reads Provider/Base URL/Secret Value off
    // this form directly — no saved credential to look up while adding one.
    const FETCH_MODELS_LABEL = "Fetch Models";
    fetchModelsBtn.addEventListener("click", async () => {
      fetchModelsBtn.disabled = true;
      fetchModelsBtn.textContent = "Fetching…";
      try {
        const provider = providerInp.value.trim() || "auto";
        const baseUrl  = baseUrlInp.value.trim();
        const models   = await listProviderModels(provider, baseUrl, valInp.value);
        if (!models.length) throw new Error("no models returned");

        modelListSlot.innerHTML = "";
        const hint = document.createElement("div");
        hint.className = "config-hint";
        hint.textContent = `${models.length} model${models.length === 1 ? "" : "s"} found — select one, or keep typing above.`;
        modelListSlot.appendChild(hint);

        // Live read, not a value captured when this handler was bound —
        // the user may have kept typing into Model since the panel opened.
        modelListSlot.appendChild(mkCustomSelect(models, modelInp.value, (v) => {
          modelInp.value = v;
        }));

        fetchModelsBtn.textContent = FETCH_MODELS_LABEL;
      } catch (err) {
        fetchModelsBtn.textContent = "Couldn't fetch — try again";
        modelListSlot.innerHTML = "";
        const errHint = document.createElement("div");
        errHint.className = "config-hint config-hint-warn";
        errHint.textContent = describeModelFetchError(err);
        modelListSlot.appendChild(errHint);
        setTimeout(() => { fetchModelsBtn.textContent = FETCH_MODELS_LABEL; }, 2500);
      } finally {
        fetchModelsBtn.disabled = false;
      }
    });

    cancelBtn?.addEventListener("click", () => this.cancelEdit());

    saveBtn.addEventListener("click", async () => {
      const name  = nameInp.value.trim();
      const id    = idInp.value.trim().replace(/\s+/g, "_");
      const value = valInp.value;

      errDiv.classList.add("hidden");
      if (!name)  { this.showFormError("Name is required"); return; }
      if (!id)    { this.showFormError("ID is required"); return; }
      if (!value && providerInp.value.trim() !== "local") { this.showFormError("Secret value is required"); return; }
      if (!/^[a-zA-Z0-9_-]+$/.test(id)) {
        this.showFormError("ID can only contain letters, numbers, underscores, and hyphens");
        return;
      }

      saveBtn.disabled = true;
      saveBtn.textContent = "Saving…";
      try {
        const provider = providerInp.value.trim();
        const model    = modelInp.value.trim();
        const baseUrl  = baseUrlInp.value.trim();
        await saveCredential({
          id, name, value, cred_type: selectedCredType,
          provider: provider || undefined,
          model:    model    || undefined,
          base_url: baseUrl  || undefined,
        });
        this.editing = null;
        await this.refresh();
      } catch (e) {
        this.showFormError(`Save failed: ${e}`);
        saveBtn.disabled = false;
        saveBtn.textContent = this.editing ? "Save Changes" : "Save Credential";
      }
    });
  }

  private showFormError(msg: string): void {
    const errDiv = this.el.querySelector("#cred-save-error") as HTMLElement;
    errDiv.textContent = msg;
    errDiv.classList.remove("hidden");
  }

  private bindList(): void {
    this.el.querySelectorAll<HTMLButtonElement>(".cred-item-edit").forEach(btn => {
      btn.addEventListener("click", () => this.startEdit(btn.dataset.id!, btn));
    });
    this.el.querySelectorAll<HTMLButtonElement>(".cred-item-del").forEach(btn => {
      btn.addEventListener("click", async () => {
        if (this.busy) return;
        this.busy = true;
        try {
          const id = btn.dataset.id!;
          const cred = this.creds.find(c => c.id === id);
          const ok = await showConfirm(`Delete credential "${cred?.name ?? id}"? Nodes using it will stop working.`, true, "Delete");
          if (!ok) return;
          btn.disabled = true; btn.textContent = "Deleting…";
          try {
            await deleteCredential(id);
            if (this.editing?.id === id) this.editing = null;
            await this.refresh();
          } catch (e) {
            btn.disabled = false; btn.textContent = "Delete";
            await showConfirm(`Delete failed: ${e}`);
          }
        } finally {
          this.busy = false;
        }
      });
    });
  }

  // Loads the decrypted secret + metadata for a saved credential into the
  // form so it can be viewed (masked, same as the password field) and
  // re-edited. The ID input stays locked: `saveCredential` upserts by ID, so
  // an edited ID would create a second, orphaned credential instead of
  // updating this one.
  private async startEdit(id: string, btn: HTMLButtonElement): Promise<void> {
    if (this.busy) return;
    const cred = this.creds.find(c => c.id === id);
    if (!cred) return;

    this.busy = true;
    btn.disabled = true;
    const originalLabel = btn.textContent;
    btn.textContent = "Loading…";
    try {
      const [value, meta] = await Promise.all([
        getCredentialSecret(id),
        getCredentialMetadata(id).catch(() => null),
      ]);
      if (value == null) {
        btn.disabled = false;
        btn.textContent = originalLabel;
        await showConfirm(`Could not load credential "${cred.name}" — it may have been deleted.`);
        await this.refresh();
        return;
      }
      this.editing = {
        id: cred.id, name: cred.name, cred_type: cred.cred_type, value,
        provider: meta?.provider ?? "", model: meta?.model ?? "", base_url: meta?.base_url ?? "",
      };
      this.render();
    } catch (e) {
      btn.disabled = false;
      btn.textContent = originalLabel;
      await showConfirm(`Failed to load credential: ${e}`);
    } finally {
      this.busy = false;
    }
  }

  private cancelEdit(): void {
    this.editing = null;
    this.render();
  }
}

function credTypeLabel(value: string): string {
  const labels: Record<string, string> = {
    api_key: "API Key",
    bearer:  "Bearer",
    basic:   "Basic Auth",
    oauth:   "OAuth",
    other:   "Custom",
  };
  return labels[value] ?? value;
}

// Cleanup for the currently-attached outside-click listener, if any.
// render() is called fresh on every panel open, save, and delete, and
// each call re-invokes this function — calling the stored cleanup before
// registering a new listener keeps at most one attached at a time, no
// matter how many times the panel re-renders.
let activeCredTypeSelectCleanup: (() => void) | null = null;

export function buildCredTypeSelect(types: CredType[], onChange: (t: CredType) => void, initialValue?: string): HTMLElement {
  activeCredTypeSelectCleanup?.();
  activeCredTypeSelectCleanup = null;

  let current = types.find(t => t.value === initialValue) ?? types[0];

  const wrap    = document.createElement("div");
  wrap.className = "csel-wrap";

  const trigger = document.createElement("button");
  trigger.type  = "button";
  trigger.className = "csel-trigger";

  const label = document.createElement("span");
  label.className = "csel-label";
  label.textContent = current.label;
  label.title = current.label;

  const arrow = document.createElement("span");
  arrow.className = "csel-arrow";
  arrow.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="6 9 12 15 18 9"/></svg>`;

  trigger.appendChild(label);
  trigger.appendChild(arrow);
  wrap.appendChild(trigger);

  const dropdown = document.createElement("div");
  dropdown.className = "csel-dropdown hidden";

  types.forEach(t => {
    const item = document.createElement("button");
    item.type  = "button";
    item.className = "csel-option" + (t.value === current.value ? " selected" : "");
    item.textContent = t.label;
    item.addEventListener("mousedown", (e) => {
      e.preventDefault();
      current = t;
      label.textContent = t.label;
      label.title = t.label;
      dropdown.querySelectorAll(".csel-option").forEach(el => el.classList.remove("selected"));
      item.classList.add("selected");
      dropdown.classList.add("hidden");
      trigger.setAttribute("aria-expanded", "false");
      onChange(t);
    });
    dropdown.appendChild(item);
  });

  wrap.appendChild(dropdown);

  trigger.addEventListener("click", () => {
    const open = !dropdown.classList.contains("hidden");
    dropdown.classList.toggle("hidden", open);
    trigger.setAttribute("aria-expanded", String(!open));
  });

  // Close on outside click
  const onOutsideClick = (e: MouseEvent) => {
    if (!wrap.contains(e.target as Node)) {
      dropdown.classList.add("hidden");
      trigger.setAttribute("aria-expanded", "false");
    }
  };
  document.addEventListener("mousedown", onOutsideClick);
  activeCredTypeSelectCleanup = () => document.removeEventListener("mousedown", onOutsideClick);

  return wrap;
}
