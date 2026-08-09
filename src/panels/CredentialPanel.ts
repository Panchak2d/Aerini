import {
  listCredentials,
  getCredentialMetadata,
  getCredentialSecret,
  saveCredential,
  deleteCredential,
} from "../ipc/credentials";
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
    this.creds = await listCredentials().catch(() => []);
    this.render();
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
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
          </button>
        </div>

        <div class="cred-panel-body" id="cred-panel-body">
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
    return this.creds.map(c => `
      <div class="cred-item${this.editing?.id === c.id ? " editing" : ""}" data-id="${escHtml(c.id)}">
        <div class="cred-item-icon"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="7.5" cy="15.5" r="3.5"/><path d="M21 2l-9.6 9.6"/><path d="M15.5 7.5l3 3L22 7l-3-3"/></svg></div>
        <div class="cred-item-info">
          <div class="cred-item-name">${escHtml(c.name)}</div>
          <div class="cred-item-id">${escHtml(c.id)}</div>
        </div>
        <span class="cred-item-type">${escHtml(credTypeLabel(c.cred_type))}</span>
        <div class="cred-item-actions">
          <button class="cred-item-edit" data-id="${escHtml(c.id)}" title="View / edit this credential">Edit</button>
          <button class="cred-item-del" data-id="${escHtml(c.id)}" title="Delete this credential">Delete</button>
        </div>
      </div>`).join("");
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
          <button type="button" class="cred-show-btn" id="cred-show" title="Show / hide" aria-label="Show or hide secret value">
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
          <input id="cred-meta-provider" type="text" placeholder="e.g. openai, anthropic, gemini" autocomplete="off" value="${escHtml(e?.provider ?? "")}" />
        </div>
        <div class="field-group">
          <label class="field-label">Model</label>
          <input id="cred-meta-model" type="text" placeholder="e.g. gpt-4o, claude-sonnet-4-6" autocomplete="off" value="${escHtml(e?.model ?? "")}" />
        </div>
        <div class="field-group">
          <label class="field-label">Base URL</label>
          <input id="cred-meta-base-url" type="text" placeholder="Leave blank for provider default" autocomplete="off" value="${escHtml(e?.base_url ?? "")}" />
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

    // Build custom select — avoids WebKitGTK Linux native <select> rendering bug
    let selectedCredType = this.editing?.cred_type ?? CRED_TYPES[0].value;
    typeWrap.appendChild(buildCredTypeSelect(CRED_TYPES, (t) => {
      hintEl.textContent = t.hint;
      valInp.placeholder = t.placeholder;
      selectedCredType   = t.value;
    }, selectedCredType));

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

    cancelBtn?.addEventListener("click", () => this.cancelEdit());

    saveBtn.addEventListener("click", async () => {
      const name  = nameInp.value.trim();
      const id    = idInp.value.trim().replace(/\s+/g, "_");
      const value = valInp.value;

      errDiv.classList.add("hidden");
      if (!name)  { this.showFormError("Name is required"); return; }
      if (!id)    { this.showFormError("ID is required"); return; }
      if (!value) { this.showFormError("Secret value is required"); return; }
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
