import { NODE_IDS } from "../node-ids";
import { REQUIRED_FIELDS, checkDangerousNodes } from "../validation";
import { showConfirm } from "../confirm";
import type { CanvasNode } from "../canvas/Node";
import type { Canvas } from "../canvas/Canvas";
import { listCredentials, getCredentialMetadata, getCredentialSecret } from "../ipc/credentials";
import { listProviderModels } from "../ipc/providers";
import { runWorkflow } from "../ipc/workflow";
import type { WorkflowLogEntry } from "../ipc/workflow";
import { escapeHtml } from "../utils";
import { getNodeDescriptor, isUnregisteredNodeType } from "../canvas/node-registry";
import { closeExpressionPicker } from "../expression-picker";
import {
  mk, mkSection, mkField,
  type ExtensionContext, type NodeConfigExtension,
} from "../node-configs/popover-utils";
import { renderScheduleFields }      from "../node-configs/schedule-config";
import { renderSaveToFolderFields }  from "../node-configs/save-to-folder-config";
import { renderTextToFileFields }    from "../node-configs/text-to-file-config";
import { renderCollectFilesFields }  from "../node-configs/collect-files-config";
import { renderAiCostWarning, renderAiAttachments }   from "./extensions/ai-prompt";
import { renderHttpAuthMode, renderHttpSsrfWarning }  from "./extensions/http";
import { renderWebhookBanners }                       from "./extensions/webhook";
import { renderSocialUploadFields }                   from "./extensions/social-upload";
import { renderPluginRawConfigEditor }                 from "./extensions/plugin-config";
import { renderUnregisteredNodeNotice }                from "./extensions/unregistered-node";
import {
  getCredentialFieldKeys, renderConfigFieldsLoop, renderCredentialSection,
  isGenericallyEditable, storedValueFitsField, AI_NODE_IDS, type PropSchema,
} from "./field-renderer";

// ── Extension registry ────────────────────────────────────────────────────────

const NODE_CONFIG_EXTENSIONS: Partial<Record<string, NodeConfigExtension>> = {
  [NODE_IDS.AI_PROMPT]:      { beforeFields: renderAiCostWarning, afterFields: renderAiAttachments },
  [NODE_IDS.AI_AGENT]:       { beforeFields: renderAiCostWarning },
  [NODE_IDS.SCHEDULE]:       { beforeFields: renderScheduleFields, replaceGenericFields: true },
  [NODE_IDS.SAVE_TO_FOLDER]: { afterFields: renderSaveToFolderFields },
  [NODE_IDS.TEXT_TO_FILE]:   { afterFields: renderTextToFileFields },
  [NODE_IDS.COLLECT_FILES]:  { afterFields: renderCollectFilesFields },
  [NODE_IDS.SOCIAL_UPLOAD]:  { afterFields: renderSocialUploadFields },
  [NODE_IDS.HTTP_REQUEST]:   { forceCredSection: true, credentialsHeader: renderHttpAuthMode, afterFields: renderHttpSsrfWarning },
  [NODE_IDS.WEBHOOK]:        { afterReliability: renderWebhookBanners },
};

// Config keys a built-in node's bespoke UI or canvas ports already own, so the
// generic field loop must skip them. Keyed by node id on purpose: a plugin node
// may declare a property with one of these names and still needs a normal field
// for it. The plugin loader rejects a type_id that collides with a built-in, so
// a plugin can't opt into (or out of) these exclusions.
const CUSTOM_UI_KEYS: Partial<Record<string, ReadonlySet<string>>> = {
  [NODE_IDS.SAVE_TO_FOLDER]: new Set(["subfolders", "folder_path", "overwrite"]),
  [NODE_IDS.COLLECT_FILES]:  new Set(["sources"]),
  [NODE_IDS.SOCIAL_UPLOAD]:  new Set(["files"]),
  [NODE_IDS.AI_PROMPT]:      new Set(["attachments"]),
  [NODE_IDS.AI_AGENT]:       new Set(["attachments"]),
};

// ── Popover state ─────────────────────────────────────────────────────────────

// Session-scoped approvals for the Test button's dangerous-node confirmation,
// keyed by checkDangerousNodes' own workflow-id + node-id key.
const _testApproved = new Set<string>();

// The shared confirm dialog lives outside the popover; while it is showing,
// the popover's outside-click, Esc, and Tab handling must leave it alone.
function isConfirmOpen(): boolean {
  const modal = document.getElementById("confirm-modal");
  return !!modal && !modal.classList.contains("hidden");
}

let _activePopover: HTMLElement | null = null;
// Unique ID per popover instance — prevents old onOutside handlers from
// closing a newly-opened popover when rapidly switching between nodes.
let _activePopoverId = 0;
// Node the currently-open popover belongs to — used on close to flag
// missing required fields. Cleared whenever the popover closes.
let _activePopoverNode: CanvasNode | null = null;
// Element focused before popover opened — restored on close
let _previousFocus: HTMLElement | null = null;
// Removes the active popover's document-level listeners (focus trap, outside
// click, Esc). Set by showPopover(), run unconditionally by closePopover() —
// every close path goes through here so nothing is left registered.
let _activePopoverCleanup: (() => void) | null = null;

function _doValidateMissingFields(): void {
  if (!_activePopoverNode) return;
  const required = REQUIRED_FIELDS[_activePopoverNode.data.node_type_id] ?? [];
  _activePopoverNode.missingRequired = required.some((f) => {
    const v = _activePopoverNode!.data.config[f];
    return v === undefined || String(v).trim() === "";
  });
}

export function closePopover(animated = true): void {
  if (!_activePopover) return;
  _activePopoverId++;   // invalidate any pending onOutside timers
  _activePopoverCleanup?.();
  _activePopoverCleanup = null;
  _doValidateMissingFields();
  const prev = _previousFocus;
  _previousFocus = null;
  closeExpressionPicker();

  if (!animated) {
    _activePopover.remove();
    _activePopover = null;
    _activePopoverNode = null;
    prev?.focus({ preventScroll: true });
    return;
  }

  // Animate out, then remove
  const el = _activePopover;
  _activePopover = null;
  _activePopoverNode = null;
  prev?.focus({ preventScroll: true });
  el.classList.add("popover-out");
  setTimeout(() => el.remove(), 120);
}

export async function showPopover(
  node: CanvasNode,
  canvasEl: HTMLCanvasElement,
  onChangeFn: () => void,
  canvas: Canvas,
): Promise<void> {
  // Close any existing popover immediately (no animation — prevents race conditions)
  closePopover(false);
  _previousFocus = document.activeElement as HTMLElement | null;

  const myId = ++_activePopoverId; // snapshot this popover's ID

  // Wrap the caller's onChange so dynamic-port nodes re-derive their port list
  // whenever any config field changes, keeping the canvas port layout in sync.
  // A rebuild can shrink the port set (e.g. adding the first subfolder replaces
  // the flat-mode "input" port) — prune any connector left pointing at a port
  // that no longer exists.
  const onChange = () => {
    let pruned: ReturnType<Canvas["pruneOrphanedConnectors"]> = [];
    if (node.data.dynamic_ports) {
      node.derivePorts(node.data.config as Record<string, unknown>);
      node.rebuildPorts();
      pruned = canvas.pruneOrphanedConnectors(node.data.id);
    }
    canvas.commitNodeEdit(node, pruned);
    onChangeFn();
  };

  const creds = await listCredentials().catch(() => []);

  // If another popover opened while we were awaiting credentials, abort.
  if (myId !== _activePopoverId) return;

  // AI nodes' Connection section is filtered by Provider (field-renderer.ts's
  // renderCredentialSection) — prefetch each pool credential's Advanced
  // Provider up front so that filter has something to check. Gated to
  // AI_NODE_IDS so non-AI nodes never pay this extra round trip.
  let credentialProviderMap: Map<string, string | undefined> = new Map();
  if (AI_NODE_IDS.has(node.data.node_type_id)) {
    const entries = await Promise.all(creds.map(async (c) =>
      [c.id, (await getCredentialMetadata(c.id).catch(() => null))?.provider] as const
    ));
    credentialProviderMap = new Map(entries);
  }

  // Another popover may have opened while we were awaiting metadata.
  if (myId !== _activePopoverId) return;

  // Parse schema — fall back to the ALL_NODES descriptor registry if the saved
  // node has an empty input_schema (happens when loaded from a .aerini file that
  // was saved before the serializer included the full schema).
  const schema  = node.data.input_schema as Record<string, unknown>;
  let rawProps = (schema?.properties ?? {}) as Record<string, unknown>;
  if (!Object.keys(rawProps).length) {
    const desc = getNodeDescriptor(node.data.node_type_id);
    if (desc) {
      const ds = desc.input_schema as Record<string, unknown>;
      rawProps = (ds?.properties ?? {}) as Record<string, unknown>;
    }
  }
  const props   = rawProps as Record<string, PropSchema>;

  //  pre-fill model/base_url/provider from a saved credential's metadata,
  // but only into fields that exist on this node and only when currently
  // blank — never overwrite a value the user already set. For enum fields
  // (provider), only fill if the metadata value is an actual option for THIS
  // node's schema, since different node types use incompatible provider
  // enums (e.g. image_gen's ["dalle3","imagen4"] vs ai_prompt's ["auto","openai",...]).
  async function autoFillFromCredentialMetadata(credentialId: string): Promise<void> {
    const meta = await getCredentialMetadata(credentialId).catch(() => null);
    if (!meta) return;
    const config = node.data.config as Record<string, unknown>;
    let changed = false;

    if (meta.model && props["model"]) {
      if (!String(config["model"] ?? "").trim()) { config["model"] = meta.model; changed = true; }
    }
    if (meta.base_url && props["base_url"]) {
      if (!String(config["base_url"] ?? "").trim()) { config["base_url"] = meta.base_url; changed = true; }
    }
    const providerEnum = props["provider"]?.enum;
    if (meta.provider && providerEnum?.includes(meta.provider)) {
      const curProvider = String(config["provider"] ?? "");
      // providerEnum[0] is the value the field auto-defaults to on first
      // render (see the enum field branch below) — treat that as "untouched".
      if (!curProvider.trim() || curProvider === providerEnum[0]) {
        config["provider"] = meta.provider; changed = true;
      }
    }

    if (myId !== _activePopoverId) return; // popover closed/reopened while awaiting
    if (changed) { onChange(); showPopover(node, canvasEl, onChangeFn, canvas); }
  }

  // Powers `x-aerini-model-picker` fields' "Fetch Models" button. Reads live
  // config at call time (not captured up front), so it always reflects
  // whatever the user has typed into provider/base_url/the credential picker
  // so far. api_key precedence mirrors the executor's own build_input(): a
  // saved credential (node.data.credentials["api_key"]) wins over the
  // inline one-off key in config, never the other way round.
  async function fetchModelsForNode(): Promise<string[]> {
    const config = node.data.config as Record<string, unknown>;
    const provider = String(config["provider"] ?? "auto");
    const baseUrl  = String(config["base_url"] ?? "");
    const credId   = node.data.credentials["api_key"];
    const apiKey   = credId
      ? (await getCredentialSecret(credId).catch(() => null)) ?? ""
      : String(config["api_key"] ?? "");
    return listProviderModels(provider, baseUrl, apiKey);
  }
  const customUiKeys = CUSTOM_UI_KEYS[node.data.node_type_id];
  const credFieldKeys = getCredentialFieldKeys(props);
  const editableKeys = Object.entries(props).filter(([k]) => !credFieldKeys.has(k) && !customUiKeys?.has(k));
  // A schema can declare object/array-of-object properties the generic
  // renderer has no control for, or leave a property's type unspecified
  // (e.g. HTTP Request's `body`), and a stored value can be an object or
  // array its property's control would corrupt; those are edited as JSON
  // instead of a text input that would overwrite them. Applies uniformly to
  // every node — built-in schemas declare or hold non-string values too
  // (HTTP Request's `headers`, Shell's `env`, Transform's `mappings`), and a
  // plugin node whose plugin is no longer installed has no descriptor and
  // only its saved schema to go on.
  const rawEditKeys = editableKeys.filter(([k, p]) =>
    !isGenericallyEditable(p) || !storedValueFitsField(p, node.data.config[k]));
  const rawKeySet = new Set(rawEditKeys.map(([k]) => k));
  const cfgKeys = rawKeySet.size ? editableKeys.filter(([k]) => !rawKeySet.has(k)) : editableKeys;

  const pop = document.createElement("div");
  pop.className = "node-popover"; pop.id = "node-popover";
  pop.setAttribute("role", "dialog");
  pop.setAttribute("aria-modal", "true");
  pop.setAttribute("aria-labelledby", "popover-title-label");

  // Header — name + node type subtitle
  const header = document.createElement("div");
  header.className = "popover-header";
  const headerText = document.createElement("div");
  headerText.className = "popover-header-text";
  const titleEl = document.createElement("div");
  titleEl.id = "popover-title-label";
  titleEl.className = "popover-title"; titleEl.textContent = node.data.name; titleEl.title = node.data.name;
  const subtitleEl = document.createElement("div");
  const subtitleText = node.data.node_type_id.replace(/_/g, " ").replace(/\b\w/g, c => c.toUpperCase());
  subtitleEl.className = "popover-subtitle"; subtitleEl.textContent = subtitleText; subtitleEl.title = subtitleText;
  headerText.appendChild(titleEl); headerText.appendChild(subtitleEl);
  // Plugin identity — same registry the schema fallback above already reads,
  // keyed off is_plugin (never a hardcoded type_id list). pack_id/pack_display_name
  // are NOT on NodeDescriptor (only on the separate Plugins-tab PluginInfo
  // type, keyed by filename, not type_id) -- author is the richest provenance
  // field actually available here.
  const pluginMeta = getNodeDescriptor(node.data.node_type_id);
  if (pluginMeta?.is_plugin) {
    const metaRow = document.createElement("div");
    metaRow.className = "popover-plugin-meta";
    const tag = document.createElement("span");
    tag.className = "palette-plugin-tag"; tag.textContent = "Plugin";
    metaRow.appendChild(tag);
    if (pluginMeta.author) {
      const authorEl = document.createElement("span");
      authorEl.className = "popover-plugin-author";
      authorEl.textContent = `by ${pluginMeta.author}`;
      authorEl.title = authorEl.textContent;
      metaRow.appendChild(authorEl);
    }
    headerText.appendChild(metaRow);
  }
  const closeBtn = document.createElement("button");
  closeBtn.className = "popover-close";
  closeBtn.setAttribute("aria-label", "Close");
  closeBtn.innerHTML = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
  closeBtn.addEventListener("click", () => closePopover());

  const testBtn = document.createElement("button");
  testBtn.className = "popover-test-btn";
  testBtn.title = "Test this node in isolation";
  testBtn.setAttribute("data-tooltip", "Test this node in isolation");
  testBtn.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg> Test`;
  testBtn.addEventListener("click", async () => {
    testBtn.disabled = true;
    testBtn.textContent = "Running…";
    try {
      await testSingleNode(node, onChange);
    } finally {
      testBtn.disabled = false;
      testBtn.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg> Test`;
    }
  });

  header.appendChild(headerText);
  header.appendChild(testBtn);
  header.appendChild(closeBtn);
  pop.appendChild(header);

  // Body
  const body = document.createElement("div");
  body.className = "popover-body";

  if (isUnregisteredNodeType(node.data.node_type_id)) {
    renderUnregisteredNodeNotice(body, node.data.node_type_id);
  }

  // Search — only when ≥4 config fields
  if (cfgKeys.length >= 4) {
    const wrap = document.createElement("div");
    wrap.className = "popover-search-wrap";
    const si = document.createElement("input") as HTMLInputElement;
    si.type = "text"; si.placeholder = "Search fields…"; si.className = "popover-search";
    si.setAttribute("aria-label", "Search configuration fields");
    si.addEventListener("input", () => {
      const q = si.value.toLowerCase();
      body.querySelectorAll<HTMLElement>(".field-group").forEach(fg => {
        const lbl = fg.querySelector(".field-label")?.textContent?.toLowerCase() ?? "";
        fg.style.display = q && !lbl.includes(q) ? "none" : "";
      });
    });
    wrap.appendChild(si); body.appendChild(wrap);
  }

  // Node name
  body.appendChild(mkSection("Node"));
  body.appendChild(mkField("Name", () => {
    const inp = mk<HTMLInputElement>("input");
    inp.type = "text"; inp.value = node.data.name; inp.autocomplete = "off"; inp.spellcheck = false;
    inp.addEventListener("input", () => { node.data.name = inp.value; titleEl.textContent = inp.value; titleEl.title = inp.value; onChange(); });
    return inp;
  }));

  // Extension context — shared by all hooks for this popover instance.
  const ext = NODE_CONFIG_EXTENSIONS[node.data.node_type_id];
  const ctx: ExtensionContext = {
    node, body, canvasEl, onChange, creds,
    rerender: () => showPopover(node, canvasEl, onChangeFn, canvas),
    hasConfigSection: cfgKeys.length > 0,
    fetchModels: fetchModelsForNode,
  };

  // Config fields
  if (cfgKeys.length > 0) {
    body.appendChild(mkSection("Configuration"));
    ext?.beforeFields?.(ctx);

    if (!ext?.replaceGenericFields) {
      const requiredKeys = REQUIRED_FIELDS[node.data.node_type_id] ?? [];
      renderConfigFieldsLoop(ctx, cfgKeys, requiredKeys);
    }
  }

  ext?.afterFields?.(ctx);
  if (rawEditKeys.length) renderPluginRawConfigEditor(ctx, credFieldKeys);

  // Credentials
  renderCredentialSection(ctx, props, ext, autoFillFromCredentialMetadata, credentialProviderMap);

  // Reliability
  body.appendChild(mkSection("Reliability"));
  body.appendChild(mkField("Retry attempts", () => {
    const inp = mk<HTMLInputElement>("input");
    inp.type = "number"; inp.min = "1"; inp.max = "10"; inp.autocomplete = "off";
    inp.value = String(node.data.retry.max_attempts);
    inp.addEventListener("input", () => {
      const v = parseInt(inp.value);
      if (!isNaN(v) && v >= 1) { node.data.retry.max_attempts = v; onChange(); }
    });
    return inp;
  }, "1 = no retry"));

  ext?.afterReliability?.(ctx);

  pop.appendChild(body);
  document.body.appendChild(pop);
  _activePopover = pop;
  _activePopoverNode = node;

  positionPopover(pop, node, canvasEl);

  // Every edit made from here on is undoable relative to the fully rendered form,
  // so defaults the render itself fills in are not attributed to the first keystroke.
  canvas.beginNodeEdit(node);

  // Move focus to the first focusable field inside the popover body.
  // Scoped to `body`, not `pop` — `pop` includes the header, where testBtn
  // (a <button>) sits before closeBtn in document order and would win
  // querySelector's first match. Landing focus there auto-fired its
  // tooltip (focus listener in tooltip-manager.ts) on every popover open,
  // and left Enter wired to "run this node" instead of editing a field.
  const FOCUSABLE = 'input, select, textarea, button, [tabindex]:not([tabindex="-1"])';
  setTimeout(() => {
    if (myId !== _activePopoverId) return;
    const first = body.querySelector<HTMLElement>(FOCUSABLE);
    first?.focus({ preventScroll: true });
  }, 50);

  // Focus trap — keep Tab/Shift+Tab inside popover
  const onFocusTrap = (e: KeyboardEvent) => {
    if (e.key !== "Tab" || isConfirmOpen()) return;
    const focusable = Array.from(pop.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>(FOCUSABLE)).filter(el => !el.disabled);
    if (!focusable.length) return;
    const first = focusable[0];
    const last  = focusable[focusable.length - 1];
    if (e.shiftKey) {
      if (document.activeElement === first) { e.preventDefault(); last.focus(); }
    } else {
      if (document.activeElement === last) { e.preventDefault(); first.focus(); }
    }
  };
  document.addEventListener("keydown", onFocusTrap, true);

  // Close on outside click.
  const onOutside = (e: MouseEvent) => {
    if (!pop.contains(e.target as Node) && !isConfirmOpen()) closePopover();
  };

  // Close on Esc
  const onEsc = (e: KeyboardEvent) => {
    if (e.key === "Escape" && !isConfirmOpen()) closePopover();
  };
  document.addEventListener("keydown", onEsc, true);

  // closePopover() removes all three listeners unconditionally, so this
  // covers every close path (Esc, outside click, the × button, and a new
  // popover superseding this one) — not just the two that fire here.
  _activePopoverCleanup = () => {
    document.removeEventListener("keydown", onFocusTrap, true);
    document.removeEventListener("keydown", onEsc, true);
    document.removeEventListener("mousedown", onOutside, true);
  };

  // Delay slightly so the triggering dblclick doesn't immediately close the popover.
  setTimeout(() => {
    if (myId === _activePopoverId) {
      document.addEventListener("mousedown", onOutside, true);
    }
  }, 120);
}

// ── Positioning ───────────────────────────────────────────────────────────────

function positionPopover(pop: HTMLElement, node: CanvasNode, canvasEl: HTMLCanvasElement): void {
  const canvas = (canvasEl as unknown as { __canvas?: import("../canvas/Canvas").Canvas }).__canvas;
  if (!canvas) { pop.style.left = "50%"; pop.style.top = "50%"; pop.style.transform = "translate(-50%,-50%)"; return; }
  const r = canvasEl.getBoundingClientRect();
  const nodeRightSx = (node.data.position.x + 220) * canvas.zoom + canvas.panX + r.left;
  const nodeMidSy   = (node.data.position.y + node.height / 2) * canvas.zoom + canvas.panY + r.top;
  const POP_W = 360, POP_MAX_H = 680, MARGIN = 12;
  let left = nodeRightSx + MARGIN;
  let top  = nodeMidSy - 140;
  if (left + POP_W > window.innerWidth - MARGIN) left = nodeRightSx - POP_W - MARGIN * 2;
  if (left < MARGIN) left = MARGIN;
  if (top + POP_MAX_H > window.innerHeight - MARGIN) top = window.innerHeight - POP_MAX_H - MARGIN;
  if (top < MARGIN) top = MARGIN;
  pop.style.left = `${left}px`; pop.style.top = `${top}px`; pop.style.transform = "none";
}

// ── Single-node test ──────────────────────────────────────────────────────────

async function testSingleNode(node: CanvasNode, onChange: () => void): Promise<void> {
  // Same confirmation a canvas Run gives for nodes that execute code.
  if (!await checkDangerousNodes(`test_${node.data.id}`, [node], _testApproved, showConfirm)) return;

  // Wrap the node in a minimal workflow: trigger → node
  const triggerId = "test_trigger";
  const minimalWorkflow = {
    id: `test_${node.data.id}`,
    name: `Test: ${node.data.name}`,
    nodes: [
      {
        id: triggerId,
        node_type_id: NODE_IDS.MANUAL_TRIGGER,
        node_type: "action",
        name: "Test Trigger",
        config: {},
        credentials: {},
        position: { x: 0, y: 0 },
        ports: { inputs: [], outputs: [{ id: "output", label: "Start", position: "right" }] },
        input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 0 }, fallback_node: null,
      },
      {
        id: node.data.id,
        node_type_id: node.data.node_type_id,
        node_type: node.data.node_type ?? "action",
        name: node.data.name,
        config: node.data.config,
        credentials: node.data.credentials,
        position: { x: 300, y: 0 },
        ports: node.data.ports,
        input_schema: node.data.input_schema,
        output_schema: node.data.output_schema,
        retry: node.data.retry ?? { max_attempts: 1, backoff_ms: 0 },
        fallback_node: null,
      },
    ],
    edges: [
      { id: "e_test", from_node: triggerId, from_port: "output", to_node: node.data.id, to_port: "input" },
    ],
  };

  const workflowJson = JSON.stringify(minimalWorkflow);
  const pop = document.getElementById("node-popover");
  if (!pop) return;
  const body = pop.querySelector(".popover-body") as HTMLElement | null;
  const container = body ?? pop;

  // Remove any existing test result panel.
  container.querySelector(".popover-test-result")?.remove();

  try {
    const result = await runWorkflow(workflowJson, {});
    const nodeOutput = result.node_outputs?.[node.data.id];
    const success   = result.success && nodeOutput !== undefined;
    const errors    = (result.logs ?? []).filter((l: WorkflowLogEntry) => l.level === "error");

    const panel = document.createElement("div");
    panel.className = `popover-test-result ${success ? "test-ok" : "test-fail"}`;

    if (success) {
      panel.innerHTML = `
        <div class="test-result-header">Node ran successfully</div>
        <pre class="test-result-json">${escapeHtml(JSON.stringify(nodeOutput, null, 2))}</pre>`;
    } else {
      const errMsg = errors.map((l: WorkflowLogEntry) => l.message).join("\n") || result.error || "Unknown error";
      panel.innerHTML = `
        <div class="test-result-header">Node failed</div>
        <pre class="test-result-json test-result-err">${escapeHtml(errMsg)}</pre>`;
    }

    container.appendChild(panel);
    container.scrollTop = container.scrollHeight;
  } catch (e) {
    const panel = document.createElement("div");
    panel.className = "popover-test-result test-fail";
    panel.innerHTML = `<div class="test-result-header">Error</div><pre class="test-result-json test-result-err">${escapeHtml(String(e))}</pre>`;
    container.appendChild(panel);
    container.scrollTop = container.scrollHeight;
  }

  onChange();
}
