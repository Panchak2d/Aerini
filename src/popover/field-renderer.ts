import { invoke } from "@tauri-apps/api/core";
import { NODE_IDS } from "../node-ids";
import type { CanvasNode } from "../canvas/Node";
import { showExpressionPicker } from "../expression-picker";
import { describeModelFetchError } from "../ipc/providers";
import {
  mk, mkSection, mkField, mkCustomSelect, CRON_PRESETS,
  type ExtensionContext, type NodeConfigExtension,
} from "../node-configs/popover-utils";

export interface PropSchema {
  type?: string;
  description?: string;
  enum?: string[];
  minimum?: number;
  maximum?: number;
  maxLength?: number;
  /** Opts a field into the credential picker. `true` for no type constraint,
   *  or an object naming the cred_type ("api_key" | "bearer" | "basic" |
   *  "oauth" | "other") to filter the saved-credential picker to that type. */
  "x-aerini-credential"?: true | { cred_type?: string };
  /** Opts a field into the model-discovery picker: a "Fetch Models" button
   *  that calls `ExtensionContext.fetchModels` and offers the result as a
   *  dropdown alongside the normal free-text input. Same shape as
   *  `x-aerini-credential` above — a schema flag, not a bespoke field type. */
  "x-aerini-model-picker"?: true;
  /** Item shape for `type: "array"` fields — JSON Schema's own `items`
   *  keyword. Only `type`/`enum` are read; array-of-object fields (e.g.
   *  Collect Files' `sources`) aren't rendered generically and must stay
   *  in `CUSTOM_UI_KEYS` (lifecycle.ts) with a bespoke node-configs
   *  extension instead. */
  items?: { type?: string; enum?: string[] };
  /** Opts a string field into a native file/folder picker button alongside
   *  the normal free-text input — same "schema flag, not a bespoke field
   *  type" shape as the two above. Any plugin or built-in node can set
   *  this on its own schema; nothing node-id-specific is needed on the
   *  host side to support a new one. */
  "x-aerini-path-picker"?: "file" | "directory" | "file-or-directory";
  /** Renders a string field as a multi-line text box. Use it for a field
   *  holding JSON or long text whose name is not in `MULTILINE_KEYS` — the
   *  flag is per schema, so another node's field of the same name is not
   *  affected. */
  "x-aerini-multiline"?: true;
}

const CREDENTIAL_KEYS = new Set(["api_key", "password"]);
export const AI_NODE_IDS: Set<string> = new Set([NODE_IDS.AI_PROMPT, NODE_IDS.AI_AGENT, NODE_IDS.IMAGE_GEN]);

interface CredentialFieldInfo { key: string; credType?: string; }

function credentialFieldInfo(key: string, prop: PropSchema): CredentialFieldInfo | null {
  const ann = prop["x-aerini-credential"];
  if (ann === true) return { key };
  if (ann && typeof ann === "object") return { key, credType: ann.cred_type };
  if (CREDENTIAL_KEYS.has(key)) return { key };
  return null;
}

/** All keys in a node's schema that should be treated as credential fields —
 *  the legacy bare `api_key`/`password` names plus anything opted in via
 *  `x-aerini-credential`. Used both to exclude these from the generic config
 *  field loop and to drive the Connection section's picker(s). */
export function getCredentialFieldKeys(props: Record<string, PropSchema>): Set<string> {
  const keys = new Set<string>();
  for (const [key, prop] of Object.entries(props)) {
    if (credentialFieldInfo(key, prop)) keys.add(key);
  }
  return keys;
}



// ── Label formatter ───────────────────────────────────────────────────────────

function formatLabel(key: string): string {
  const OVERRIDES: Record<string, string> = {
    api_key: "API Key", base_url: "Base URL", cron_expr: "Cron Expression",
    rate_limit_rpm: "Rate Limit (RPM)", smtp_host: "SMTP Host", smtp_port: "SMTP Port",
    db_path: "Database Path", max_tokens: "Max Tokens", mock_payload: "Mock Payload",
    timeout_secs: "Timeout (seconds)", duration_secs: "Duration (seconds)",
    poll_interval_secs: "Poll Interval (seconds)", backoff_ms: "Backoff (ms)",
    max_attempts: "Max Attempts", source_node: "Source Node", array_field: "Array Field",
    item_var: "Item Variable", index_var: "Index Variable", chunk_size: "Chunk Size",
    input_text: "Input Text", run_at: "Run At (ISO timestamp)", interval_secs: "Interval (seconds)",
    max_messages: "Max Messages", max_iterations: "Max Iterations", session_id: "Session ID",
    lhs: "Left Value", op: "Operator", rhs: "Right Value",
  };
  if (OVERRIDES[key]) return OVERRIDES[key];
  return key.replace(/_/g, " ").replace(/\b\w/g, c => c.toUpperCase())
    .replace(/\bUrl\b/g, "URL").replace(/\bId\b/g, "ID").replace(/\bJson\b/g, "JSON")
    .replace(/\bSql\b/g, "SQL").replace(/\bHtml\b/g, "HTML").replace(/\bRpm\b/g, "RPM")
    .replace(/\bSmtp\b/g, "SMTP").replace(/\bApi\b/g, "API");
}

/** Capitalizes a free-text Advanced Provider value for display (e.g.
 *  "anthropic" -> "Anthropic"). Provider metadata is a plain text field
 *  (CredentialPanel), not a fixed enum, so this can't map through a lookup
 *  table — don't replace it with one, it needs to work for any string a
 *  user typed. */
function formatProviderLabel(provider: string): string {
  return provider.charAt(0).toUpperCase() + provider.slice(1);
}

// ── Per-field sub-renderers ───────────────────────────────────────────────────
// These are the named functions for each field type. Each is also the target
// of the FIELD_RENDERERS dispatch table below (where applicable).

function renderTextField(
  key: string, prop: PropSchema, cur: string,
  node: CanvasNode, canvasEl: HTMLCanvasElement,
  onChange: () => void, syncRequired: () => void,
): HTMLElement {
  const inp = mk<HTMLInputElement>("input");
  inp.type = "text"; inp.value = cur;
  inp.placeholder = prop.description ? prop.description + " · {{ for data" : "Value or {{ for data";
  inp.autocomplete = "off"; inp.spellcheck = false;
  // Only constrains a literal value typed directly — an {{expression}} in this
  // same input isn't evaluated here, so it can still resolve past this cap at
  // execution time. Server-side enforcement is the real backstop.
  if (prop.maxLength !== undefined) inp.maxLength = prop.maxLength;
  inp.addEventListener("input", () => { node.data.config[key] = inp.value; onChange(); syncRequired(); });

  // Only add the expression button for string-typed fields.
  // Number and enum fields don't accept {{}} expressions.
  if (!prop.enum && prop.type !== "number") {
    const wrap = document.createElement("div");
    wrap.className = "field-string-wrap";
    const exprBtn = document.createElement("button");
    exprBtn.type = "button";
    exprBtn.className = "field-interp-hint field-interp-hint--inline";
    exprBtn.title = "Insert expression from a previous node\nExample: {{HTTP Request.output.body.name}}";
    exprBtn.setAttribute("data-tooltip", "Insert expression from a previous node");
    exprBtn.textContent = "{{ }}";
    exprBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      showExpressionPicker(exprBtn, inp, node.data.id, canvasEl);
    });
    wrap.appendChild(inp);
    wrap.appendChild(exprBtn);
    return wrap;
  }
  return inp;
}

/** Model field with discovery. Always renders the normal free-text input
 *  (identical to `renderTextField`, `{{ }}` expression button included) —
 *  that control is what actually gets saved and is never disabled or
 *  hidden. "Fetch Models" is a convenience layer on top: on success it
 *  offers a dropdown of discovered ids that writes into the same field; on
 *  any failure it leaves the text field exactly as it was, with only the
 *  button's own label giving transient feedback. Requires `ctx.fetchModels`
 *  (see `ExtensionContext`) — if a node schema sets `x-aerini-model-picker`
 *  without wiring that hook, this silently degrades to a plain text field,
 *  same fallback logic as a fetch failure. */
function renderModelPickerField(
  key: string, prop: PropSchema, cur: string,
  node: CanvasNode, canvasEl: HTMLCanvasElement,
  onChange: () => void, syncRequired: () => void,
  fetchModels: (() => Promise<string[]>) | undefined,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "field-multiline-wrap"; // vertical stack, same gap as other composite fields

  wrap.appendChild(renderTextField(key, prop, cur, node, canvasEl, onChange, syncRequired));

  if (!fetchModels) return wrap;

  const FETCH_LABEL = "Fetch Models";
  const fetchBtn = document.createElement("button");
  fetchBtn.type = "button";
  fetchBtn.className = "code-load-btn";
  fetchBtn.textContent = FETCH_LABEL;
  wrap.appendChild(fetchBtn);

  const listSlot = document.createElement("div");
  wrap.appendChild(listSlot);

  fetchBtn.addEventListener("click", async () => {
    fetchBtn.disabled = true;
    fetchBtn.textContent = "Fetching…";
    try {
      const models = await fetchModels();
      if (!models.length) throw new Error("no models returned");

      listSlot.innerHTML = "";
      const hint = document.createElement("div");
      hint.className = "config-hint";
      hint.textContent = `${models.length} model${models.length === 1 ? "" : "s"} found — select one, or keep typing above.`;
      listSlot.appendChild(hint);

      // Read live, not the `cur` this closure was built with — the user
      // may have edited the text field since this control was rendered.
      const liveValue = String((node.data.config as Record<string, unknown>)[key] ?? "");
      listSlot.appendChild(mkCustomSelect(models, liveValue, (v) => {
        (node.data.config as Record<string, unknown>)[key] = v;
        const input = wrap.querySelector<HTMLInputElement>("input");
        if (input) input.value = v;
        onChange(); syncRequired();
      }));

      fetchBtn.textContent = FETCH_LABEL;
    } catch (err) {
      // The free-text field above is untouched and fully usable either way.
      // The button's own label still gives brief feedback, but the reason
      // (missing/bad key, SSRF block, network error, ...) is now shown
      // underneath instead of discarded — "couldn't fetch" alone gave no
      // way to tell a missing API key apart from an unreachable server.
      fetchBtn.textContent = "Couldn't fetch — try again";
      listSlot.innerHTML = "";
      const errHint = document.createElement("div");
      errHint.className = "config-hint config-hint-warn";
      errHint.textContent = describeModelFetchError(err);
      listSlot.appendChild(errHint);
      setTimeout(() => { fetchBtn.textContent = FETCH_LABEL; }, 2500);
    } finally {
      fetchBtn.disabled = false;
    }
  });

  return wrap;
}

/** Ollama's default port — the one local backend with its own sidebar
 *  preset template, so it's the sane one-click default. Any other local
 *  server (LM Studio, vLLM, llama.cpp, ...) still just needs its own URL
 *  typed in; this never overwrites a value the user already entered. */
const OLLAMA_DEFAULT_BASE_URL = "http://localhost:11434/v1";

/** Base URL field for AI nodes only. `local` has no cloud default by
 *  design (records.rs: "user-supplied only; no cloud default" — there's no
 *  single sensible port across Ollama/LM Studio/vLLM/etc.), so a blank
 *  Base URL under Provider "local" resolves to an empty string and both
 *  Fetch Models and the node itself fail immediately. That's correct
 *  behavior, but the generic field's placeholder ("Leave blank for
 *  provider default") actively told users to do the one thing guaranteed
 *  to break it. This swaps in a concrete example and a one-click fill for
 *  the common case once Provider is actually "local" — free-text entry
 *  underneath is untouched, so any other local server still works exactly
 *  as before. */
function renderBaseUrlField(
  key: string, prop: PropSchema, cur: string,
  node: CanvasNode, canvasEl: HTMLCanvasElement,
  onChange: () => void, syncRequired: () => void,
): HTMLElement {
  const field = renderTextField(key, prop, cur, node, canvasEl, onChange, syncRequired);
  const provider = String((node.data.config as Record<string, unknown>)["provider"] ?? "");
  if (provider !== "local") return field;

  // base_url is never enum/number-typed, so renderTextField above always
  // wraps it in a container (see the branch at the bottom of that
  // function) — a bare `<input>` return isn't reachable here.
  const inp = field.querySelector<HTMLInputElement>("input");
  if (inp) inp.placeholder = `e.g. ${OLLAMA_DEFAULT_BASE_URL} (Ollama)`;

  const fillBtn = document.createElement("button");
  fillBtn.type = "button";
  fillBtn.className = "code-load-btn";
  fillBtn.textContent = "Use Ollama defaults";
  fillBtn.addEventListener("click", () => {
    if (inp) inp.value = OLLAMA_DEFAULT_BASE_URL;
    node.data.config[key] = OLLAMA_DEFAULT_BASE_URL;
    onChange(); syncRequired();
  });

  const wrap = document.createElement("div");
  wrap.className = "field-multiline-wrap";
  wrap.appendChild(field);
  wrap.appendChild(fillBtn);
  return wrap;
}

function renderEnumField(
  key: string, prop: PropSchema, cur: string,
  node: CanvasNode, onChange: () => void, syncRequired: () => void,
): HTMLElement {
  const effective = cur || prop.enum![0] || "";
  if (!cur && effective) node.data.config[key] = effective;
  return mkCustomSelect(prop.enum!, effective, (v) => { node.data.config[key] = v; onChange(); syncRequired(); });
}

function renderNumberField(
  key: string, prop: PropSchema,
  node: CanvasNode, onChange: () => void, syncRequired: () => void,
): HTMLElement {
  const inp = mk<HTMLInputElement>("input");
  inp.type = "number"; inp.autocomplete = "off";
  inp.value = String(node.data.config[key] ?? "");
  inp.placeholder = prop.description ?? "";
  if (prop.minimum !== undefined) inp.min = String(prop.minimum);
  if (prop.maximum !== undefined) inp.max = String(prop.maximum);
  inp.addEventListener("input", () => {
    const v = parseFloat(inp.value);
    if (Number.isNaN(v)) {
      // Field cleared — remove key rather than storing "" which would
      // be passed as a string to Rust and fail deserialization.
      delete node.data.config[key];
    } else {
      node.data.config[key] = v;
    }
    onChange(); syncRequired();
  });
  return inp;
}

function renderBoolField(
  key: string, _prop: PropSchema,
  node: CanvasNode, onChange: () => void, syncRequired: () => void,
): HTMLElement {
  const wrap = document.createElement("label");
  wrap.className = "field-bool-wrap";

  const cb = mk<HTMLInputElement>("input");
  cb.type = "checkbox";
  cb.className = "field-bool-checkbox";
  // Treat any truthy stored value as checked. Rust serializes booleans as
  // true/false; stored strings "true"/"1" are also accepted for robustness.
  const stored = node.data.config[key];
  cb.checked = stored === true || stored === "true" || stored === 1 || stored === "1";

  cb.addEventListener("change", () => {
    node.data.config[key] = cb.checked;
    onChange(); syncRequired();
  });

  wrap.appendChild(cb);
  return wrap;
}

/** Capitalizes one array-enum option for display (e.g. "created" ->
 *  "Created"). Options are short fixed identifiers from the schema, not
 *  free text, so a simple capitalize is enough — no lookup table needed. */
function formatEnumOption(value: string): string {
  return value.replace(/_/g, " ").replace(/\b\w/g, c => c.toUpperCase());
}

/** Reads a config value as a string array, dropping anything that isn't a
 *  string. Used by both array renderers below instead of trusting
 *  whatever shape happens to already be stored — a value saved before
 *  this renderer existed (or edited by hand in an exported workflow file)
 *  might not be an array at all. */
function currentStringArray(node: CanvasNode, key: string): string[] {
  const v = node.data.config[key];
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
}

/** Fixed set of options (`prop.items.enum`) as a checkbox group — one
 *  checkbox per option, config value is the list of checked ones in the
 *  schema's own order. A `<fieldset>`/`<legend>` pair gives the group a
 *  name for assistive tech without repeating the visible label `mkField`
 *  already renders above it (the legend is screen-reader-only). */
function renderArrayEnumField(
  key: string, options: string[],
  node: CanvasNode, onChange: () => void, syncRequired: () => void,
): HTMLElement {
  const fieldset = document.createElement("fieldset");
  fieldset.className = "field-checkbox-group";

  const legend = document.createElement("legend");
  legend.className = "sr-only";
  legend.textContent = formatLabel(key);
  fieldset.appendChild(legend);

  for (const opt of options) {
    const optLabel = document.createElement("label");
    optLabel.className = "field-checkbox-option";

    const cb = mk<HTMLInputElement>("input");
    cb.type = "checkbox";
    cb.value = opt;
    cb.checked = currentStringArray(node, key).includes(opt);
    cb.addEventListener("change", () => {
      const next = new Set(currentStringArray(node, key));
      if (cb.checked) next.add(opt); else next.delete(opt);
      // Filter the schema's own option list rather than building from
      // `next` directly, so stored order always matches the schema's
      // order regardless of which boxes were (un)checked in what sequence.
      node.data.config[key] = options.filter(o => next.has(o));
      onChange(); syncRequired();
    });

    const text = document.createElement("span");
    text.textContent = formatEnumOption(opt);

    optLabel.appendChild(cb);
    optLabel.appendChild(text);
    fieldset.appendChild(optLabel);
  }

  return fieldset;
}

/** Free-form string array (no fixed `items.enum`) as a chip list: existing
 *  values shown as removable chips, a text input + Add button (or Enter)
 *  appends a new one. Mirrors save-to-folder-config.ts's subfolder list —
 *  re-render the whole chip row on every add/remove rather than patching
 *  the DOM in place, same as that list does. */
function renderArrayStringListField(
  key: string,
  node: CanvasNode, onChange: () => void, syncRequired: () => void,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "field-chiplist-wrap";

  const chipsEl = document.createElement("div");
  chipsEl.className = "field-chiplist";

  const renderChips = () => {
    chipsEl.innerHTML = "";
    currentStringArray(node, key).forEach((val, idx) => {
      const chip = document.createElement("span");
      chip.className = "field-chip";

      const text = document.createElement("span");
      text.className = "field-chip-text";
      text.textContent = val;
      text.title = val;

      const del = document.createElement("button");
      del.type = "button";
      del.className = "field-chip-remove";
      del.setAttribute("aria-label", `Remove ${val}`);
      del.textContent = "×";
      del.addEventListener("click", () => {
        const next = currentStringArray(node, key);
        next.splice(idx, 1);
        node.data.config[key] = next;
        renderChips();
        onChange(); syncRequired();
      });

      chip.appendChild(text);
      chip.appendChild(del);
      chipsEl.appendChild(chip);
    });
  };
  renderChips();

  const addRow = document.createElement("div");
  addRow.className = "field-chiplist-add-row";

  const addInp = mk<HTMLInputElement>("input");
  addInp.type = "text";
  addInp.autocomplete = "off";
  addInp.spellcheck = false;
  addInp.placeholder = "Add and press Enter";
  addInp.setAttribute("aria-label", `Add to ${formatLabel(key)}`);

  const commit = () => {
    const val = addInp.value.trim();
    if (!val) return;
    const next = currentStringArray(node, key);
    if (!next.includes(val)) {
      next.push(val);
      node.data.config[key] = next;
      renderChips();
      onChange(); syncRequired();
    }
    addInp.value = "";
  };
  addInp.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); commit(); }
  });

  const addBtn = document.createElement("button");
  addBtn.type = "button";
  addBtn.className = "field-chiplist-add-btn";
  addBtn.textContent = "Add";
  addBtn.addEventListener("click", commit);

  addRow.appendChild(addInp);
  addRow.appendChild(addBtn);
  wrap.appendChild(chipsEl);
  wrap.appendChild(addRow);
  return wrap;
}

/** Dispatches a `type: "array"` field to whichever of the two renderers
 *  above fits its `items` shape. Array-of-object fields (`items.type ===
 *  "object"`, e.g. Collect Files' `sources`) are deliberately not handled
 *  here — those already get a bespoke node-configs extension via
 *  CUSTOM_UI_KEYS and must stay there; this function is never reached for
 *  them (see renderField's dispatch order) and doesn't try to guess a
 *  generic UI for an arbitrary object shape. */
function renderArrayField(
  key: string, prop: PropSchema,
  node: CanvasNode, onChange: () => void, syncRequired: () => void,
): HTMLElement {
  return prop.items?.enum
    ? renderArrayEnumField(key, prop.items.enum, node, onChange, syncRequired)
    : renderArrayStringListField(key, node, onChange, syncRequired);
}

/** Augments a normal text field with one or two native picker buttons
 *  (Tauri's file-dialog plugin — same `dialog:allow-open` capability
 *  save-to-folder-config.ts's folder picker already uses). The text field
 *  underneath is still what's actually saved and still fully editable by
 *  hand; a picked path just fills it in, the same "convenience layer over
 *  the real control" shape renderModelPickerField uses. Native pickers
 *  can't offer "file or folder" as a single dialog, so `"file-or-directory"`
 *  renders both buttons rather than guessing which one the author wants. */
function renderPathPickerField(
  key: string, prop: PropSchema, cur: string,
  node: CanvasNode, canvasEl: HTMLCanvasElement,
  onChange: () => void, syncRequired: () => void,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "field-multiline-wrap"; // vertical stack, same gap as other composite fields

  wrap.appendChild(renderTextField(key, prop, cur, node, canvasEl, onChange, syncRequired));

  const row = document.createElement("div");
  row.className = "path-picker-row";

  const addPickBtn = (label: string, command: "pick_file_dialog" | "pick_folder_dialog") => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "folder-pick-btn";
    btn.textContent = label;
    btn.addEventListener("click", async () => {
      btn.disabled = true;
      const picked = await invoke<string | null>(command).catch(() => null);
      btn.disabled = false;
      if (picked) {
        node.data.config[key] = picked;
        const input = wrap.querySelector<HTMLInputElement>("input[type='text']");
        if (input) input.value = picked;
        onChange(); syncRequired();
      }
    });
    row.appendChild(btn);
  };

  const mode = prop["x-aerini-path-picker"];
  if (mode === "file" || mode === "file-or-directory") addPickBtn("Choose File…", "pick_file_dialog");
  if (mode === "directory" || mode === "file-or-directory") addPickBtn("Choose Folder…", "pick_folder_dialog");

  wrap.appendChild(row);
  return wrap;
}

function renderCronField(
  key: string, cur: string, node: CanvasNode,
  onChange: () => void, syncRequired: () => void,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "cron-wrap";

  const presetsRow = document.createElement("div");
  presetsRow.className = "cron-presets";

  const inp = mk<HTMLInputElement>("input");

  CRON_PRESETS.forEach(p => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "cron-preset-btn";
    btn.textContent = p.label;
    btn.title = p.value;
    btn.setAttribute("data-tooltip", p.value);
    btn.addEventListener("click", () => {
      inp.value = p.value;
      node.data.config[key] = p.value;
      onChange(); syncRequired();
    });
    presetsRow.appendChild(btn);
  });

  inp.value = cur;
  inp.placeholder = "e.g. 0 9 * * 1-5";
  inp.addEventListener("input", () => { node.data.config[key] = inp.value; onChange(); syncRequired(); });

  const hint = document.createElement("div");
  hint.className = "cron-hint";
  hint.textContent = "min hour day month weekday (0=Sun)";

  wrap.appendChild(presetsRow);
  wrap.appendChild(inp);
  wrap.appendChild(hint);
  return wrap;
}

const MULTILINE_KEYS = ["body", "command", "prompt", "system", "condition",
  "mock_payload", "cases", "mappings", "content", "code", "goal", "tools", "context"];

function renderMultilineField(
  key: string, prop: PropSchema, cur: string,
  node: CanvasNode, canvasEl: HTMLCanvasElement,
  onChange: () => void, syncRequired: () => void,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "field-multiline-wrap";

  const ta = mk<HTMLTextAreaElement>("textarea");
  ta.value = cur;
  ta.rows = key === "code" ? 12 : 3;
  ta.placeholder = prop.description ?? "";
  if (key === "code") ta.className = "code-editor";
  ta.addEventListener("input", () => { node.data.config[key] = ta.value; onChange(); syncRequired(); });
  ta.addEventListener("keydown", (e) => {
    if (e.key === "Tab") {
      e.preventDefault();
      const s = ta.selectionStart, en = ta.selectionEnd;
      ta.value = ta.value.slice(0, s) + "  " + ta.value.slice(en);
      ta.selectionStart = ta.selectionEnd = s + 2;
      node.data.config[key] = ta.value; onChange(); syncRequired();
    }
  });
  wrap.appendChild(ta);

  // Skip expression button on code fields — code uses raw JS, not {{}} syntax.
  if (key !== "code") {
    const exprBtn = document.createElement("button");
    exprBtn.type = "button";
    exprBtn.className = "field-interp-hint";
    exprBtn.title = "Insert expression from a previous node\nExample: {{HTTP Request.output.body.name}}";
    exprBtn.setAttribute("data-tooltip", "Insert expression from a previous node");
    exprBtn.textContent = "{{ }}";
    exprBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      showExpressionPicker(exprBtn, ta, node.data.id, canvasEl);
    });
    wrap.appendChild(exprBtn);
  } else {
    const hint = document.createElement("div");
    hint.className = "field-interp-hint";
    hint.title = "Type {{ to insert data from another node\nExample: {{HTTP Request.output.body}}";
    hint.setAttribute("data-tooltip", "Insert data from another node");
    hint.textContent = "{{ }}";
    wrap.appendChild(hint);
  }

  if (key === "code") {
    const loadBtn = document.createElement("button");
    loadBtn.type = "button";
    loadBtn.className = "code-load-btn";
    loadBtn.textContent = "Load .js file";
    loadBtn.addEventListener("click", () => {
      const inp = document.createElement("input");
      inp.type = "file"; inp.accept = ".js,.ts,.txt";
      inp.addEventListener("change", () => {
        const file = inp.files?.[0]; if (!file) return;
        const reader = new FileReader();
        reader.onload = () => {
          ta.value = String(reader.result ?? "");
          node.data.config[key] = ta.value; onChange(); syncRequired();
        };
        reader.readAsText(file);
      });
      inp.click();
    });
    wrap.appendChild(loadBtn);
  }

  return wrap;
}

// ── Dispatch table ────────────────────────────────────────────────────────────
// Keyed by prop.type. Only covers the type-keyed subset of the dispatch chain:
// the cron / model-picker / base_url / path-picker / enum / multiline / array
// guards run before this table is consulted. renderTextField is the fallback
// and is not in the table because it requires extra parameters (cur, canvasEl)
// not available in this narrower signature.
//
// "array" has no entry here on purpose: an array field's own items shape
// (plain string vs. enum vs. object) decides which of renderArrayField's two
// renderers applies, which this table's single-function-per-type shape can't
// express — see the dedicated array check in renderField below instead.
type FieldTypeRenderer = (
  key: string, prop: PropSchema,
  node: CanvasNode, onChange: () => void, syncRequired: () => void,
) => HTMLElement;

const FIELD_RENDERERS: Partial<Record<string, FieldTypeRenderer>> = {
  number:  renderNumberField,
  boolean: renderBoolField,
};

/** False when `renderField` below has no dedicated control for this schema
 *  property: an object, or an array whose items aren't plain strings. Those
 *  would fall through to the text input, which shows "[object Object]" and
 *  replaces the stored value with a string on the first keystroke. Keep in
 *  step with the array and type-dispatch checks in `renderField`. */
export function isGenericallyEditable(prop: PropSchema): boolean {
  if (prop.enum) return true;
  if (prop.type === "object") return false;
  if (prop.type === "array") return !prop.items?.type || prop.items.type === "string";
  return true;
}

/** False when the stored config value is an object or array that this
 *  property's generic control would corrupt: only a list of strings under a
 *  string-list schema (array with plain-string or no `items`) survives, since
 *  every other control reads the value as text and overwrites it with a
 *  string on the first edit, and the string-list control silently drops
 *  non-string entries. Absent, null, and primitive values always fit — a
 *  number or boolean shown in a text field is coerced, not lost. Keep in step
 *  with `isGenericallyEditable` and `renderField`. */
export function storedValueFitsField(prop: PropSchema, value: unknown): boolean {
  if (value === null || typeof value !== "object") return true;
  const isStringList = prop.type === "array" && !prop.enum && (!prop.items?.type || prop.items.type === "string");
  return isStringList && Array.isArray(value) && value.every(x => typeof x === "string");
}

/** Render one config field. Checks: cron key → model-picker flag → AI-node
 *  base_url → path-picker flag → enum → multiline flag or key → array (string/enum
 *  items) → type dispatch table → text fallback. Order is load-bearing — do
 *  not reorder. */
function renderField(
  key: string, prop: PropSchema, cur: string,
  node: CanvasNode, canvasEl: HTMLCanvasElement,
  onChange: () => void, syncRequired: () => void,
  fetchModels: (() => Promise<string[]>) | undefined,
): HTMLElement {
  if (key === "cron_expr") return renderCronField(key, cur, node, onChange, syncRequired);
  if (prop["x-aerini-model-picker"]) return renderModelPickerField(key, prop, cur, node, canvasEl, onChange, syncRequired, fetchModels);
  if (key === "base_url" && AI_NODE_IDS.has(node.data.node_type_id)) return renderBaseUrlField(key, prop, cur, node, canvasEl, onChange, syncRequired);
  if (prop["x-aerini-path-picker"]) return renderPathPickerField(key, prop, cur, node, canvasEl, onChange, syncRequired);
  if (prop.enum)            return renderEnumField(key, prop, cur, node, onChange, syncRequired);
  if (prop["x-aerini-multiline"] || MULTILINE_KEYS.includes(key)) return renderMultilineField(key, prop, cur, node, canvasEl, onChange, syncRequired);
  // Array-of-object fields (items.type === "object", e.g. Collect Files'
  // "sources") are excluded here on purpose — those get a bespoke
  // node-configs extension via CUSTOM_UI_KEYS instead, and there's no
  // generic UI this function could build for an arbitrary object shape.
  if (prop.type === "array" && (!prop.items?.type || prop.items.type === "string")) {
    return renderArrayField(key, prop, node, onChange, syncRequired);
  }
  const typeRenderer = FIELD_RENDERERS[prop.type ?? ""];
  if (typeRenderer) return typeRenderer(key, prop, node, onChange, syncRequired);
  return renderTextField(key, prop, cur, node, canvasEl, onChange, syncRequired);
}

/** Renders the generic "Configuration" field loop for a node's config schema.
 *  Appends one field-group element per key, in order, to ctx.body. */
export function renderConfigFieldsLoop(
  ctx: ExtensionContext,
  cfgKeys: Array<[string, PropSchema]>,
  requiredKeys: string[],
): void {
  const { node, body, canvasEl, onChange, fetchModels, rerender } = ctx;

  for (const [key, prop] of cfgKeys) {
    const cur = String(node.data.config[key] ?? "");
    const isRequired = requiredKeys.includes(key);
    let fieldEl: HTMLElement;
    const syncRequired = () => {
      if (!isRequired) return;
      const v = node.data.config[key];
      fieldEl.classList.toggle("field-required-empty", v === undefined || String(v).trim() === "");
    };

    const hint = prop.maxLength !== undefined
      ? `${prop.description ? prop.description + " · " : ""}Max ${prop.maxLength} characters`
      : prop.description;

    // The Connection section's saved-credential pool for AI nodes is
    // filtered by this node's current Provider (renderCredentialSection) —
    // changing it has to rebuild the popover so that filter re-runs, same
    // as any other field-visibility-affecting change.
    const fieldOnChange = (key === "provider" && AI_NODE_IDS.has(node.data.node_type_id))
      ? () => { onChange(); rerender(); }
      : onChange;

    fieldEl = mkField(
      formatLabel(key),
      () => renderField(key, prop, cur, node, canvasEl, fieldOnChange, syncRequired, fetchModels),
      hint,
      isRequired,
    );

    if (isRequired && (!cur || cur.trim() === "")) fieldEl.classList.add("field-required-empty");
    body.appendChild(fieldEl);
  }
}

/** Renders the "Connection" section: saved-credential picker, plus the
 *  AI-node inline one-off API key fallback. No-ops if this node has no
 *  credential surface. */
export function renderCredentialSection(
  ctx: ExtensionContext,
  props: Record<string, PropSchema>,
  ext: NodeConfigExtension | undefined,
  autoFillFromCredentialMetadata: (credentialId: string) => void,
  credentialProviderMap: Map<string, string | undefined> = new Map(),
): void {
  const { node, body, creds, onChange } = ctx;

  const credFields: CredentialFieldInfo[] = [];
  for (const [key, prop] of Object.entries(props)) {
    const info = credentialFieldInfo(key, prop);
    if (info) credFields.push(info);
  }

  const hasCredField =
    credFields.length > 0 ||
    Object.keys(node.data.credentials).length > 0 ||
    (ext?.forceCredSection ?? false);

  if (!hasCredField) return;

  // No schema field matched (forceCredSection, or a saved workflow with a
  // credential entry whose key no longer appears in the current schema) —
  // fall back to the same single-field default the pre-multi-field code used.
  if (credFields.length === 0) {
    credFields.push({ key: props["api_key"] !== undefined ? "api_key" : props["password"] !== undefined ? "password" : "api_key" });
  }

  body.appendChild(mkSection("Connection"));
  ext?.credentialsHeader?.(ctx);

  const hint = document.createElement("div");
  hint.className = "config-hint";
  hint.textContent = credFields.length > 1
    ? "Select a saved credential to attach to each field below."
    : "Select a saved credential to attach to this node.";
  body.appendChild(hint);

  for (const { key: credKey, credType } of credFields) {
    let pool = credType ? creds.filter(c => c.cred_type === credType) : creds;

    // AI nodes: hard-filter the api_key picker to credentials whose Advanced
    // Provider matches this node's currently selected Provider, plus any
    // credential with no Provider set at all — those stay visible
    // regardless, since Advanced metadata is optional and hiding them would
    // silently break existing workflows. A blank or "auto" Provider means
    // the provider is detected later (from the Base URL), so nothing can
    // mismatch yet and the pool is left unfiltered.
    const isAiProviderField = credKey === "api_key" && AI_NODE_IDS.has(node.data.node_type_id);
    const curProvider = isAiProviderField
      ? String((node.data.config as Record<string, unknown>)["provider"] ?? "")
      : "";
    const filterByProvider = isAiProviderField && curProvider !== "" && curProvider !== "auto";
    if (filterByProvider) {
      pool = pool.filter(c => {
        const p = credentialProviderMap.get(c.id);
        return !p || p === curProvider;
      });
    }

    const label = credFields.length > 1 ? `Use Saved Credential — ${formatLabel(credKey)}` : "Use Saved Credential";

    body.appendChild(mkField(label, () => {
      const options = [{ value: "", label: "— none —" }, ...pool.map(c => {
        const p = credentialProviderMap.get(c.id);
        const optLabel = isAiProviderField && p ? `${c.name} — ${formatProviderLabel(p)}` : c.name;
        return { value: c.id, label: optLabel };
      })];
      const cur = node.data.credentials[credKey] ?? "";
      return mkCustomSelect(options.map(o => o.label), options.find(o => o.value === cur)?.label ?? "— none —", (selLabel) => {
        const opt = options.find(o => o.label === selLabel);
        if (opt?.value) {
          node.data.credentials[credKey] = opt.value;
          void autoFillFromCredentialMetadata(opt.value);
        } else {
          delete node.data.credentials[credKey];
        }
        onChange();
      });
    }));

    if (!pool.length) {
      const warn = document.createElement("div");
      warn.className = "config-hint config-hint-warn";
      warn.textContent = !creds.length
        ? "No credentials saved. Click Credentials in the toolbar."
        : filterByProvider
          ? `No saved credentials for provider ${formatProviderLabel(curProvider)}. Add one in Credentials in the toolbar.`
          : `No saved credentials of type "${credType}". Add one in Credentials in the toolbar.`;
      body.appendChild(warn);
    }

    // One-off inline key: AI nodes read api_key straight from config if no
    // credential is selected, so this needs no backend support — see
    // executor build_input(), which only overwrites resolved_input["api_key"]
    // when node.credentials actually has an entry for credKey.
    if (credKey === "api_key" && AI_NODE_IDS.has(node.data.node_type_id)) {
      const details = document.createElement("details");
      details.className = "cred-advanced";
      const summary = document.createElement("summary");
      summary.className = "cred-advanced-summary";
      summary.textContent = "Or enter a key directly";
      details.appendChild(summary);
      details.appendChild(mkField("API Key (one-off)", () => {
        const inp = mk<HTMLInputElement>("input");
        inp.type = "password"; inp.autocomplete = "off";
        inp.value = String((node.data.config as Record<string, unknown>)["api_key"] ?? "");
        inp.placeholder = "sk-…";
        inp.addEventListener("input", () => {
          (node.data.config as Record<string, unknown>)["api_key"] = inp.value;
          onChange();
        });
        return inp;
      }, "Saved in this workflow's file, unencrypted. Ignored if a saved credential is selected above."));
      body.appendChild(details);
    }
  }
}
