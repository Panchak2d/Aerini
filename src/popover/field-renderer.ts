import { NODE_IDS } from "../node-ids";
import type { CanvasNode } from "../canvas/Node";
import { showExpressionPicker } from "../expression-picker";
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
}

export const CREDENTIAL_KEYS = new Set(["api_key", "password"]);
const AI_NODE_IDS: Set<string> = new Set([NODE_IDS.AI_PROMPT, NODE_IDS.AI_AGENT, NODE_IDS.IMAGE_GEN]);



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
  };
  if (OVERRIDES[key]) return OVERRIDES[key];
  return key.replace(/_/g, " ").replace(/\b\w/g, c => c.toUpperCase())
    .replace(/\bUrl\b/g, "URL").replace(/\bId\b/g, "ID").replace(/\bJson\b/g, "JSON")
    .replace(/\bSql\b/g, "SQL").replace(/\bHtml\b/g, "HTML").replace(/\bRpm\b/g, "RPM")
    .replace(/\bSmtp\b/g, "SMTP").replace(/\bApi\b/g, "API");
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
// the cron / enum / multiline guards run before this table is consulted.
// renderTextField is the fallback and is not in the table because it requires
// extra parameters (cur, canvasEl) not available in this narrower signature.
//
// renderArrayField is intentionally absent: no array-type input fields reach
// renderConfigFieldsLoop — all array props in node schemas are in CUSTOM_UI_KEYS
// and filtered out in lifecycle.ts before the field loop runs. Adding it would
// be dead code.
type FieldTypeRenderer = (
  key: string, prop: PropSchema,
  node: CanvasNode, onChange: () => void, syncRequired: () => void,
) => HTMLElement;

const FIELD_RENDERERS: Partial<Record<string, FieldTypeRenderer>> = {
  number:  renderNumberField,
  boolean: renderBoolField,
};

/** Render one config field. Checks: cron key → enum → multiline key → type
 *  dispatch table → text fallback. Order is load-bearing — do not reorder. */
function renderField(
  key: string, prop: PropSchema, cur: string,
  node: CanvasNode, canvasEl: HTMLCanvasElement,
  onChange: () => void, syncRequired: () => void,
): HTMLElement {
  if (key === "cron_expr") return renderCronField(key, cur, node, onChange, syncRequired);
  if (prop.enum)            return renderEnumField(key, prop, cur, node, onChange, syncRequired);
  if (MULTILINE_KEYS.includes(key)) return renderMultilineField(key, prop, cur, node, canvasEl, onChange, syncRequired);
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
  const { node, body, canvasEl, onChange } = ctx;

  for (const [key, prop] of cfgKeys) {
    const cur = String(node.data.config[key] ?? "");
    const isRequired = requiredKeys.includes(key);
    let fieldEl: HTMLElement;
    const syncRequired = () => {
      if (!isRequired) return;
      const v = node.data.config[key];
      fieldEl.classList.toggle("field-required-empty", v === undefined || String(v).trim() === "");
    };

    fieldEl = mkField(
      formatLabel(key),
      () => renderField(key, prop, cur, node, canvasEl, onChange, syncRequired),
      prop.description,
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
): void {
  const { node, body, creds, onChange } = ctx;

  const hasCredField =
    Object.keys(props).some(k => CREDENTIAL_KEYS.has(k)) ||
    Object.keys(node.data.credentials).length > 0 ||
    (ext?.forceCredSection ?? false);

  if (!hasCredField) return;

  const credKey = props["api_key"] !== undefined ? "api_key"
    : props["password"] !== undefined ? "password"
    : "api_key"; // default for http_request
  body.appendChild(mkSection("Connection"));
  ext?.credentialsHeader?.(ctx);

  const hint = document.createElement("div");
  hint.className = "config-hint";
  hint.textContent = "Select a saved credential to attach to this node.";
  body.appendChild(hint);
  body.appendChild(mkField("Use Saved Credential", () => {
    const options = [{ value: "", label: "— none —" }, ...creds.map(c => ({ value: c.id, label: c.name }))];
    const cur = node.data.credentials[credKey] ?? "";
    return mkCustomSelect(options.map(o => o.label), options.find(o => o.value === cur)?.label ?? "— none —", (label) => {
      const opt = options.find(o => o.label === label);
      if (opt?.value) {
        node.data.credentials[credKey] = opt.value;
        void autoFillFromCredentialMetadata(opt.value);
      } else {
        delete node.data.credentials[credKey];
      }
      onChange();
    });
  }));
  if (!creds.length) {
    const warn = document.createElement("div");
    warn.className = "config-hint config-hint-warn";
    warn.textContent = "No credentials saved. Click Credentials in the toolbar.";
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
