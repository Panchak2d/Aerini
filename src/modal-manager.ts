import type { NodeDescriptor } from "./ipc/workflow";
import { NODE_IDS, TRIGGER_NODE_IDS } from "./node-ids";
import { THEMES, getStoredTheme, applyTheme } from "./theme";

type ImportCallback = (obj: Record<string, unknown>) => void;

let _allNodes: NodeDescriptor[] = [];
let _pendingImport: Record<string, unknown> | null = null;
let _onConfirmImport: ImportCallback | null = null;

export function initModals(
  allNodes: NodeDescriptor[],
  onConfirmImport: ImportCallback
): void {
  _allNodes = allNodes;
  _onConfirmImport = onConfirmImport;

  const hide = (id: string) => document.getElementById(id)!.classList.add("hidden");

  document.getElementById("import-cancel")!.addEventListener("click", () => {
    hide("import-preview-modal"); _pendingImport = null;
  });
  document.getElementById("import-confirm")!.addEventListener("click", () => {
    if (_pendingImport && _onConfirmImport) {
      _onConfirmImport(_pendingImport); hide("import-preview-modal"); _pendingImport = null;
    }
  });
  document.getElementById("import-preview-modal")!.addEventListener("click", e => {
    if (e.target === document.getElementById("import-preview-modal")) {
      hide("import-preview-modal"); _pendingImport = null;
    }
  });
  document.getElementById("btn-close-settings")!.addEventListener("click", () => hide("settings-modal"));
  document.getElementById("settings-modal")!.addEventListener("click", e => {
    if (e.target === document.getElementById("settings-modal")) hide("settings-modal");
  });
  document.getElementById("btn-close-shortcuts")!.addEventListener("click", () => hide("shortcuts-modal"));
  document.getElementById("shortcuts-modal")!.addEventListener("click", e => {
    if (e.target === document.getElementById("shortcuts-modal")) hide("shortcuts-modal");
  });

  // Settings persistence
  const snapCb = document.getElementById("setting-grid-snap") as HTMLInputElement;
  const fitCb  = document.getElementById("setting-autofit")   as HTMLInputElement;
  snapCb.checked = localStorage.getItem("aerini_grid_snap") === "true";
  fitCb.checked  = localStorage.getItem("aerini_autofit") !== "false";
  snapCb.addEventListener("change", () => localStorage.setItem("aerini_grid_snap", String(snapCb.checked)));
  fitCb.addEventListener("change",  () => localStorage.setItem("aerini_autofit",  String(fitCb.checked)));

  // theme select — options built from THEMES so a future theme
  // needs no HTML edit here, only a new registry entry +
  // CSS block. initTheme() (theme.ts, called once at app startup, before
  // this modal is ever opened) already applied the persisted choice to
  // <html>; this only needs to reflect that choice in the dropdown and
  // apply future changes the user makes from here.
  const themeSelect = document.getElementById("setting-theme") as HTMLSelectElement;
  for (const t of THEMES) {
    const opt = document.createElement("option");
    opt.value = t.id;
    opt.textContent = t.label;
    themeSelect.appendChild(opt);
  }
  themeSelect.value = getStoredTheme();
  themeSelect.addEventListener("change", () => applyTheme(themeSelect.value));
}

export function showImportPreview(obj: Record<string, unknown>): void {
  const converted = isN8nWorkflow(obj) ? convertN8nWorkflow(obj) : obj;
  // Ephemeral field set by convertN8nWorkflow (see its own doc comment) —
  // read here, then deleted so it never reaches _onConfirmImport/gets saved
  // as part of the actual workflow.
  const importWarnings = (converted._n8n_import_warnings as string[] | undefined) ?? [];
  delete converted._n8n_import_warnings;
  _pendingImport = converted;
  document.getElementById("import-name")!.textContent        = String(obj.name ?? "Untitled");
  document.getElementById("import-desc")!.textContent        = String(obj.description ?? "");
  document.getElementById("import-node-count")!.textContent  = String((obj.nodes as unknown[])?.length ?? 0);
  document.getElementById("import-edge-count")!.textContent  = String((obj.edges as unknown[])?.length ?? 0);
  document.getElementById("import-version")!.textContent     = String(obj.aerini_version ?? obj.schema_version ?? "?");

  const reqList = document.getElementById("import-req-list")!;
  reqList.innerHTML = "";
  const typeIds = [...new Set(
    (obj.nodes as Array<{ node_type_id: string }>)?.map(n => n.node_type_id) ?? []
  )];
  for (const tid of typeIds) {
    const known = _allNodes.find(n => n.type_id === tid);
    const item = document.createElement("div");
    item.className = "import-req-item";
    const dot = document.createElement("span");
    dot.className = "import-req-dot";
    dot.style.background = known ? "var(--green)" : "var(--amber)";
    item.appendChild(dot);
    item.appendChild(document.createTextNode(known ? tid : `${tid} (unknown)`));
    reqList.appendChild(item);
  }
  // surface If/Switch condition-translation
  // limitations here, before the user confirms import, rather than only at
  // run time (the node types themselves are already listed above via the
  // green/amber dots — this is specifically about their config content).
  for (const warning of importWarnings) {
    const item = document.createElement("div");
    item.className = "import-req-item";
    const dot = document.createElement("span");
    dot.className = "import-req-dot";
    dot.style.background = "var(--amber)";
    item.appendChild(dot);
    item.appendChild(document.createTextNode(warning));
    reqList.appendChild(item);
  }
  document.getElementById("import-preview-modal")!.classList.remove("hidden");
}

export function isGridSnapEnabled(): boolean {
  return localStorage.getItem("aerini_grid_snap") === "true";
}

function isN8nWorkflow(obj: Record<string, unknown>): boolean {
  // n8n workflows have a "nodes" array where each node has a "type" field
  // starting with "n8n-nodes-base." and a "parameters" object
  const nodes = obj.nodes as Array<Record<string, unknown>> | undefined;
  return Array.isArray(nodes) && nodes.length > 0 &&
    typeof nodes[0]?.type === "string" && nodes[0].type.startsWith("n8n-nodes-base.");
}

const N8N_TYPE_MAP: Record<string, string> = {
  // Already mapped — do not remove
  "n8n-nodes-base.manualTrigger":      NODE_IDS.MANUAL_TRIGGER,
  "n8n-nodes-base.webhook":            NODE_IDS.WEBHOOK,
  "n8n-nodes-base.scheduleTrigger":    NODE_IDS.SCHEDULE,
  "n8n-nodes-base.httpRequest":        NODE_IDS.HTTP_REQUEST,
  "n8n-nodes-base.code":               NODE_IDS.CODE,
  "n8n-nodes-base.if":                 NODE_IDS.IF_CONDITION,
  "n8n-nodes-base.switch":             NODE_IDS.SWITCH,
  "n8n-nodes-base.emailSend":          NODE_IDS.EMAIL_SEND,
  "n8n-nodes-base.readWriteFile":      NODE_IDS.FILE,
  "n8n-nodes-base.set":                NODE_IDS.SET_VARIABLE,
  "n8n-nodes-base.wait":               NODE_IDS.WAIT,
  "n8n-nodes-base.noOp":               NODE_IDS.OUTPUT,
  "n8n-nodes-base.stickyNote":         NODE_IDS.NOTE,
  "n8n-nodes-base.merge":              NODE_IDS.MERGE,
  "n8n-nodes-base.slack":              "slack",
  "n8n-nodes-base.gmail":              "email",
  "n8n-nodes-base.googleSheets":       "google_sheets",
  "n8n-nodes-base.postgres":           "database",
  "n8n-nodes-base.mySql":              "database",
  "n8n-nodes-base.redis":              "unsupported",
  "n8n-nodes-base.github":             "github",
  "n8n-nodes-base.notion":             "notion",
  "n8n-nodes-base.airtable":           "unsupported",
  "n8n-nodes-base.discord":            "discord",
  "n8n-nodes-base.telegram":           "telegram",
  "n8n-nodes-base.function":           NODE_IDS.CODE,
};

export function convertN8nWorkflow(n8n: Record<string, unknown>): Record<string, unknown> {
  const MAX_IMPORT_NODES     = 200;
  const MAX_IMPORT_FILE_BYTES = 5 * 1024 * 1024; // 5 MB
  const approxBytes = JSON.stringify(n8n).length;
  if (approxBytes > MAX_IMPORT_FILE_BYTES) {
    throw new Error(
      `n8n workflow JSON is too large (${(approxBytes / 1024 / 1024).toFixed(1)} MB) — import limit is 5 MB.`
    );
  }
  const n8nNodes = (n8n.nodes as Array<Record<string, unknown>>) ?? [];
  if (n8nNodes.length > MAX_IMPORT_NODES) {
    throw new Error(
      `n8n workflow has ${n8nNodes.length} nodes — import limit is ${MAX_IMPORT_NODES}. Split the workflow and import each part separately.`
    );
  }
  const n8nConns = (n8n.connections as Record<string, unknown>) ?? {};
  // Built before node conversion (not after, unlike the edge-conversion loop
  // below) because If/Switch condition translation needs each node's
  // predecessor name while still inside the aeriniNodes.map() callback.
  const predecessors = buildPredecessorMap(n8nConns);
  const conditionWarnings: string[] = [];

  const nodeIdMap   = new Map<string, string>(); // n8n name → aerini id
  const nodeTypeMap = new Map<string, string>(); // aerini id → aerini node_type_id (for edge port resolution below)

  const aeriniNodes = n8nNodes.map((n: Record<string, unknown>, idx: number) => {
    const aeriniId = `node_imported_${idx}`;
    const n8nName = String(n.name ?? `Node ${idx}`);
    nodeIdMap.set(n8nName, aeriniId);

    const typeId = N8N_TYPE_MAP[String(n.type ?? "")] ?? "unsupported";
    nodeTypeMap.set(aeriniId, typeId);
    const pos = n.position as number[] | undefined;
    const rawParams = (n.parameters as Record<string, unknown>) ?? {};

    // translate n8n's decision-logic config
    // into Aerini's, not just the output port each branch lands on (that
    // part was Batch G). Best-effort only — see translateIfCondition/
    // translateSwitchConfig's own doc comments for exactly what shape is
    // (and isn't) translated. Anything not confidently translatable is left
    // unconfigured rather than guessed at, and surfaced as an import-time
    // warning instead of failing silently at run time.
    let config: Record<string, unknown> = rawParams;
    if (typeId === NODE_IDS.IF_CONDITION || typeId === NODE_IDS.SWITCH) {
      const preds = predecessors.get(n8nName);
      const sourceName = preds && preds.size === 1 ? [...preds][0] : null;
      if (typeId === NODE_IDS.IF_CONDITION) {
        const result = translateIfCondition(rawParams, sourceName, n8nName);
        if ("condition" in result) config = { condition: result.condition };
        else conditionWarnings.push(result.warning);
      } else {
        const result = translateSwitchConfig(rawParams, sourceName, n8nName);
        if ("warning" in result) conditionWarnings.push(result.warning);
        else config = result;
      }
    }

    return {
      id:           aeriniId,
      node_type_id: typeId,
      node_type:    getNodeCategory(typeId),
      name:         n8nName,
      config,
      credentials:  {},
      ports:        resolvePorts(typeId),
      input_schema:  { type: "object", properties: {} },
      output_schema: { type: "object" },
      retry:        { max_attempts: 1, backoff_ms: 500 },
      fallback_node: null,
      position:     { x: Array.isArray(pos) ? (pos[0] ?? 0) + 400 : 100, y: Array.isArray(pos) ? (pos[1] ?? 0) + 200 : 100 },
    };
  });

  // Convert n8n connection format to Aerini edges
  const aeriniEdges: unknown[] = [];
  let edgeIdx = 0;
  for (const [fromName, conns] of Object.entries(n8nConns)) {
    const fromId = nodeIdMap.get(fromName);
    if (!fromId) continue;
    const outputs = (conns as Record<string, unknown>).main as Array<Array<Record<string, unknown>>> | undefined;
    if (!Array.isArray(outputs)) continue;
    const fromTypeId = nodeTypeMap.get(fromId) ?? "unsupported";
    outputs.forEach((portConns, portIdx) => {
      if (!Array.isArray(portConns)) return;
      portConns.forEach((c: Record<string, unknown>) => {
        const toId = nodeIdMap.get(String(c.node ?? ""));
        if (!toId) return;
        aeriniEdges.push({
          id:        `edge_imported_${edgeIdx++}`,
          from_node: fromId,
          from_port: n8nOutputPortId(fromTypeId, portIdx),
          to_node:   toId,
          to_port:   "input",
          condition:  null, on_success: null, on_failure: null,
        });
      });
    });
  }

  const result: Record<string, unknown> = {
    schema_version: "1.0",
    id:   `wf_imported_${Date.now()}`,
    name: String(n8n.name ?? "Imported from n8n"),
    description: "Imported from n8n workflow",
    nodes: aeriniNodes,
    edges: aeriniEdges,
    metadata: { author: "imported", created_at: new Date().toISOString(), updated_at: new Date().toISOString(), version: "1.0.0", tags: ["n8n-import"] },
  };
  // Ephemeral, single-underscore-prefixed field (mirrors this codebase's own
  // __-prefixed executor-metadata convention for "not part of the persisted
  // workflow shape") — read and deleted by showImportPreview before
  // _pendingImport is ever set, so it never reaches _onConfirmImport or gets
  // saved. Not added to the exported Workflow type below: this key is only
  // ever present on the raw object convertN8nWorkflow returns, not on the
  // real Aerini workflow shape imported workflows share with every other
  // workflow once import-confirm has cleared it.
  if (conditionWarnings.length > 0) {
    result._n8n_import_warnings = conditionWarnings;
  }
  return result;
}

function getNodeCategory(typeId: string): "action" | "logic" | "utility" | "ai" {
  const logic:   string[] = [NODE_IDS.IF_CONDITION, NODE_IDS.SWITCH, NODE_IDS.LOOP, NODE_IDS.STOP];
  const utility: string[] = [NODE_IDS.DELAY, NODE_IDS.WAIT, NODE_IDS.TRANSFORM, NODE_IDS.JSON_NODE,
                              NODE_IDS.SET_VARIABLE, NODE_IDS.GET_VARIABLE, NODE_IDS.OUTPUT, NODE_IDS.NOTE];
  const ai:      string[] = [NODE_IDS.AI_PROMPT, NODE_IDS.AI_AGENT, NODE_IDS.AI_MEMORY, NODE_IDS.TEXT_SPLITTER];
  if (logic.includes(typeId))   return "logic";
  if (utility.includes(typeId)) return "utility";
  if (ai.includes(typeId))      return "ai";
  return "action";
}

/** Fallback only — used when no real descriptor for typeId exists in _allNodes (e.g. "unsupported"). */
function getDefaultPorts(typeId: string): { inputs: Array<{id:string;label:string;position:string}>; outputs: Array<{id:string;label:string;position:string}> } {
  const inputs = TRIGGER_NODE_IDS.has(typeId) ? [] : [{ id: "input", label: "In", position: "left" }];
  const outputs = typeId === NODE_IDS.STOP ? [] : [{ id: "output", label: "Out", position: "right" }];
  return { inputs, outputs };
}

/**
 * Real per-type ports, sourced from the live backend node registry (_allNodes,
 * populated from get_node_types()) when a matching descriptor exists — this is
 * what every branching/multi-port node's *actual* port ids come from (e.g.
 * if_condition's on_true/on_false, switch's case_1..case_8/default). Falls back
 * to the generic single input/output guess only for a type with no registered
 * descriptor (e.g. the "unsupported" placeholder for n8n node types with no
 * Aerini equivalent).
 */
function resolvePorts(typeId: string): { inputs: Array<{id:string;label:string;position:string}>; outputs: Array<{id:string;label:string;position:string}> } {
  const known = _allNodes.find(n => n.type_id === typeId);
  if (known) return { inputs: known.ports.inputs, outputs: known.ports.outputs };
  return getDefaultPorts(typeId);
}

/**
 * Maps an n8n output-connection array index to the real Aerini output port id
 * for the given (already-mapped) Aerini node type.
 *
 * If/Switch get explicit index→port mapping because n8n's connection format
 * only carries a bare array index, not a port name — verified against
 * if_condition.rs (ports() defines on_true before on_false, matching n8n's own
 * documented "two outputs labeled True and False", index 0 = True) and
 * switch.rs (case_1..case_8 + default). n8n's Switch node assigns sequential
 * rule outputs to sequential indices (rule 1 = output 0, rule 2 = output 1,
 * ...) and places its optional "Extra Output" fallback one index past the
 * last rule (docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.switch)
 * — so any index at or beyond Aerini's 8 case ports (including that fallback
 * slot) is mapped to "default", the only remaining valid port. This is a
 * best-effort index mapping only: it does not translate n8n's actual
 * match/condition config into Aerini's `cases`/`default_port` config field
 * (a separate, larger fix — n8n's rule shape varies per node version and
 * isn't parsed here; tracked in the backlog, not fixed by this batch).
 *
 * Every other node type uses its real output port id at that array index
 * (falling back to the first known output port, or "output" if the type has
 * no registered descriptor at all) instead of the previous "output"/"out_N"
 * guess.
 */
function n8nOutputPortId(typeId: string, portIdx: number): string {
  if (typeId === NODE_IDS.IF_CONDITION) {
    return portIdx === 0 ? "on_true" : "on_false";
  }
  if (typeId === NODE_IDS.SWITCH) {
    return portIdx < 8 ? `case_${portIdx + 1}` : "default";
  }
  const outputs = resolvePorts(typeId).outputs;
  return outputs[portIdx]?.id ?? outputs[0]?.id ?? "output";
}

// ── If/Switch decision-logic translation ───


type IfTranslation     = { condition: string } | { warning: string };
type SwitchTranslation = { field: string; cases: string; source_node: string } | { warning: string };

/**
 * Reverse-maps each n8n node name to the set of n8n node names that connect
 * INTO it via a "main" output — i.e. its immediate predecessor(s). A node
 * with exactly one predecessor gets a confident source_node for condition
 * translation below; zero or more than one is ambiguous (n8n's `$json`
 * means "this node's own input data," which has no single answer when more
 * than one upstream node feeds in) and is left untranslated by the caller.
 * Walks the identical `connections[name].main[port][]` shape the real
 * edge-conversion loop in convertN8nWorkflow already parses, rather than a
 * second, potentially-divergent reading of the same input.
 */
function buildPredecessorMap(n8nConns: Record<string, unknown>): Map<string, Set<string>> {
  const preds = new Map<string, Set<string>>();
  for (const [fromName, conns] of Object.entries(n8nConns)) {
    const outputs = (conns as Record<string, unknown>).main as Array<Array<Record<string, unknown>>> | undefined;
    if (!Array.isArray(outputs)) continue;
    for (const portConns of outputs) {
      if (!Array.isArray(portConns)) continue;
      for (const c of portConns) {
        const toName = String((c as Record<string, unknown>).node ?? "");
        if (!toName) continue;
        const set = preds.get(toName) ?? new Set<string>();
        set.add(fromName);
        preds.set(toName, set);
      }
    }
  }
  return preds;
}

/**
 * Extracts the dot-path after `$json.` from a bare n8n expression string of
 * the exact shape `={{ $json.a.b.c }}` (leading `=` optional — n8n prefixes
 * expression-mode field values with it but not always, depending on UI
 * version). This is intentionally the ONLY expression shape recognized —
 * not a named-node reference (`$('Other Node').item.json...`), not a
 * function call, not string concatenation, not multiple `{{ }}` blocks.
 * Every real n8n-exported workflow JSON checked this fix used exactly
 * this bare-`$json` shorthand for If/Switch field references; anything else
 * falls through to the "not translated" warning path rather than risking a
 * wrong guess at n8n's full expression grammar. Matches Aerini's own
 * dot-path semantics (nodes/util.rs::traverse_dotpath, expression/
 * resolver.rs::traverse_path — plain `.`-split object-key traversal, no
 * bracket/array-index support), so array-index paths are also deliberately
 * rejected rather than emitted as a condition Aerini can't resolve anyway.
 */
function extractJsonFieldPath(raw: unknown): string | null {
  if (typeof raw !== "string") return null;
  let s = raw.trim();
  if (s.startsWith("=")) s = s.slice(1).trim();
  const m = /^\{\{\s*\$json((?:\.[A-Za-z_][A-Za-z0-9_]*)+)\s*\}\}$/.exec(s);
  return m ? m[1].slice(1) : null;
}

/**
 * Renders a literal n8n rightValue/value2 as an Aerini If-condition
 * right-hand-side token. Numbers/booleans are emitted bare; strings are
 * single-quoted (if_condition.rs's evaluate() strips one layer of
 * surrounding quotes from the right-hand side before comparing either way —
 * VERIFIED by direct read of aerini-engine/src/nodes/if_condition.rs — so
 * quoting a numeric-looking string here is harmless, not a type mismatch).
 * Returns null for anything that isn't a plain literal (a second
 * `{{ $json... }}` reference on the right-hand side, an object, an array) —
 * out of scope, see extractJsonFieldPath's own doc comment.
 */
function renderConditionOperand(raw: unknown): string | null {
  if (typeof raw === "number" || typeof raw === "boolean") return String(raw);
  if (typeof raw === "string") {
    if (/\{\{.*\}\}/.test(raw)) return null;
    return `'${raw}'`;
  }
  return null;
}

/** Same literal-only restriction as renderConditionOperand, but returns the
 * raw JS value (not an Aerini condition-string token) for use as a Switch
 * case's `match` value, which is compared for exact equality against the
 * field's resolved value rather than spliced into a condition string. */
function extractCaseMatchValue(raw: unknown): string | number | boolean | null {
  if (typeof raw === "number" || typeof raw === "boolean") return raw;
  if (typeof raw === "string") return /\{\{.*\}\}/.test(raw) ? null : raw;
  return null;
}

const V2_CONDITION_OP_MAP: Record<string, string> = {
  equals: "==", notEquals: "!=", gt: ">", lt: "<", gte: ">=", lte: "<=", contains: "contains",
};
const V1_CONDITION_OP_MAP: Record<string, string> = {
  equal: "==", notEqual: "!=", contains: "contains",
  larger: ">", largerEqual: ">=", smaller: "<", smallerEqual: "<=",
};

/** Translates a single n8n v2+/v3 filter condition object
 * (`{leftValue, rightValue, operator: {type, operation, singleValue?}}`)
 * into an Aerini If-condition string, or a warning if it can't be. */
function translateV2Condition(cond: Record<string, unknown>, sourceName: string, n8nNodeName: string): IfTranslation {
  const fieldPath = extractJsonFieldPath(cond.leftValue);
  if (!fieldPath) {
    return { warning: `If node '${n8nNodeName}': left-hand side is not a plain '{{ $json.field }}' reference — condition left unconfigured (defaults to the False branch).` };
  }
  const operator = (cond.operator as Record<string, unknown>) ?? {};
  const opName = typeof operator.operation === "string" ? operator.operation : "";

  if (operator.singleValue === true && (opName === "true" || opName === "false")) {
    return { condition: `{{${sourceName}.output.${fieldPath}}} == ${opName}` };
  }
  const aeriniOp = V2_CONDITION_OP_MAP[opName];
  if (!aeriniOp) {
    return { warning: `If node '${n8nNodeName}': operator '${opName || "(unset)"}' isn't supported by Aerini's If node (only equals/not-equals/greater/less/contains) — condition left unconfigured (defaults to the False branch).` };
  }
  const rhs = renderConditionOperand(cond.rightValue);
  if (rhs === null) {
    return { warning: `If node '${n8nNodeName}': right-hand side is not a plain literal value — condition left unconfigured (defaults to the False branch).` };
  }
  return { condition: `{{${sourceName}.output.${fieldPath}}} ${aeriniOp} ${rhs}` };
}

/** Translates a single n8n v1 legacy condition entry (from
 * `conditions.boolean[]`/`.string[]`/`.number[]`, shape
 * `{value1, operation, value2?}`) into an Aerini If-condition string. */
function translateV1Condition(category: string, cond: Record<string, unknown>, sourceName: string, n8nNodeName: string): IfTranslation {
  const fieldPath = extractJsonFieldPath(cond.value1);
  if (!fieldPath) {
    return { warning: `If node '${n8nNodeName}': left-hand side (value1) is not a plain '{{ $json.field }}' reference — condition left unconfigured (defaults to the False branch).` };
  }
  const opName = typeof cond.operation === "string" ? cond.operation : "";
  if (category === "boolean" && (opName === "true" || opName === "false")) {
    return { condition: `{{${sourceName}.output.${fieldPath}}} == ${opName}` };
  }
  const aeriniOp = V1_CONDITION_OP_MAP[opName];
  if (!aeriniOp) {
    return { warning: `If node '${n8nNodeName}': legacy operator '${opName || "(unset)"}' isn't supported — condition left unconfigured (defaults to the False branch).` };
  }
  const rhs = renderConditionOperand(cond.value2);
  if (rhs === null) {
    return { warning: `If node '${n8nNodeName}': right-hand side (value2) is not a plain literal value — condition left unconfigured (defaults to the False branch).` };
  }
  return { condition: `{{${sourceName}.output.${fieldPath}}} ${aeriniOp} ${rhs}` };
}

/**
 * Translates an n8n If node's `parameters.conditions` into Aerini's single
 * `condition` string, handling both the typeVersion 1 legacy shape
 * (`conditions.{boolean,string,number}[]`, one array entry per condition,
 * implicitly AND-ed) and the typeVersion 2+ shape (`conditions.conditions[]`
 * + `combinator`). Aerini's If node supports exactly one comparison — a
 * chained AND/OR condition list of length other than 1 is left
 * untranslated (a specific warning, not a partial/wrong guess) rather than
 * collapsing multiple conditions into one arbitrarily.
 */
function translateIfCondition(params: Record<string, unknown>, sourceName: string | null, n8nNodeName: string): IfTranslation {
  const conditions = params.conditions as Record<string, unknown> | undefined;
  if (!conditions || typeof conditions !== "object") {
    return { warning: `If node '${n8nNodeName}': no recognizable condition config found — condition left unconfigured (defaults to the False branch).` };
  }
  if (!sourceName) {
    return { warning: `If node '${n8nNodeName}': could not identify exactly one upstream node to read data from — condition left unconfigured (defaults to the False branch).` };
  }

  if (Array.isArray(conditions.conditions)) {
    const list = conditions.conditions as Array<Record<string, unknown>>;
    if (list.length !== 1) {
      return { warning: `If node '${n8nNodeName}': ${list.length} chained conditions (AND/OR) — Aerini's If node only supports a single comparison. Condition left unconfigured (defaults to the False branch); combine upstream or split into nested If nodes manually.` };
    }
    return translateV2Condition(list[0], sourceName, n8nNodeName);
  }

  const categories = ["boolean", "string", "number", "dateTime"] as const;
  const entries: Array<{ cat: string; cond: Record<string, unknown> }> = [];
  for (const cat of categories) {
    const arr = conditions[cat];
    if (Array.isArray(arr)) for (const c of arr) entries.push({ cat, cond: c as Record<string, unknown> });
  }
  if (entries.length !== 1) {
    return { warning: `If node '${n8nNodeName}': ${entries.length} legacy condition(s) found — Aerini's If node only supports exactly one comparison. Condition left unconfigured (defaults to the False branch).` };
  }
  if (entries[0].cat === "dateTime") {
    return { warning: `If node '${n8nNodeName}': date/time conditions aren't supported by Aerini's If node — condition left unconfigured (defaults to the False branch).` };
  }
  return translateV1Condition(entries[0].cat, entries[0].cond, sourceName, n8nNodeName);
}

/**
 * Translates an n8n Switch node's `parameters` (rules mode only — n8n's
 * "expression" mode runs arbitrary JavaScript to pick an output index,
 * which Aerini's Switch node has no equivalent for) into Aerini's
 * `field`+`cases`+`source_node` config. Rule `i` maps to Aerini's
 * `case_{i+1}` — the same index-to-port mapping n8nOutputPortId already
 * uses for edge routing, kept identical on purpose so a rule's
 * translated `match` value actually lands on the port its own n8n-wired
 * edge was translated onto. Every rule must compare the SAME field with the
 * SAME (equals-only) operator: Aerini's Switch is fundamentally a single
 * field dispatched to N literal matches, not per-rule independent
 * conditions the way n8n's rules mode allows, so anything wider (a
 * different field per rule, a non-equality operator, more than 8 rules,
 * n8n's legacy typeVersion-1 value1/rules.rules shape — not independently
 * verified this batch, deliberately not guessed at) is left untranslated.
 */
function translateSwitchConfig(params: Record<string, unknown>, sourceName: string | null, n8nNodeName: string): SwitchTranslation {
  const mode = typeof params.mode === "string" ? params.mode : "rules";
  if (mode !== "rules") {
    return { warning: `Switch node '${n8nNodeName}': uses '${mode}' mode (a JavaScript expression picks the output) — Aerini's Switch node can't run n8n expressions. Configure 'field'/'cases'/'source_node' manually.` };
  }
  const rulesObj = params.rules as Record<string, unknown> | undefined;
  const values = rulesObj?.values;
  if (!Array.isArray(values) || values.length === 0) {
    return { warning: `Switch node '${n8nNodeName}': no translatable rules found (legacy typeVersion-1 Switch configs aren't supported) — configure 'field'/'cases'/'source_node' manually.` };
  }
  if (values.length > 8) {
    return { warning: `Switch node '${n8nNodeName}': ${values.length} rules — Aerini's Switch node supports at most 8 cases. Configure manually.` };
  }
  if (!sourceName) {
    return { warning: `Switch node '${n8nNodeName}': could not identify exactly one upstream node to read data from — configure 'field'/'cases'/'source_node' manually.` };
  }

  let sharedField: string | null = null;
  const cases: Array<{ match: string | number | boolean; port: string }> = [];

  for (let i = 0; i < values.length; i++) {
    const rule = values[i] as Record<string, unknown>;
    const condObj = rule.conditions as Record<string, unknown> | undefined;
    const list = condObj?.conditions;
    if (!Array.isArray(list) || list.length !== 1) {
      return { warning: `Switch node '${n8nNodeName}': rule ${i + 1} has ${Array.isArray(list) ? list.length : 0} chained conditions — Aerini's Switch node only matches one field against one value per case. Configure manually.` };
    }
    const cond = list[0] as Record<string, unknown>;
    const fieldPath = extractJsonFieldPath(cond.leftValue);
    if (!fieldPath) {
      return { warning: `Switch node '${n8nNodeName}': rule ${i + 1}'s left-hand side is not a plain '{{ $json.field }}' reference — configure manually.` };
    }
    if (sharedField === null) sharedField = fieldPath;
    else if (sharedField !== fieldPath) {
      return { warning: `Switch node '${n8nNodeName}': rules compare different fields ('${sharedField}' vs '${fieldPath}') — Aerini's Switch node can only switch on one field for the whole node. Configure manually.` };
    }

    const operator = (cond.operator as Record<string, unknown>) ?? {};
    const opName = typeof operator.operation === "string" ? operator.operation : "";
    let matchValue: string | number | boolean | null;
    if (operator.singleValue === true && (opName === "true" || opName === "false")) {
      matchValue = opName === "true";
    } else if (opName === "equals") {
      matchValue = extractCaseMatchValue(cond.rightValue);
    } else {
      return { warning: `Switch node '${n8nNodeName}': rule ${i + 1} uses operator '${opName || "(unset)"}' — Aerini's Switch node only matches exact equality. Configure manually.` };
    }
    if (matchValue === null) {
      return { warning: `Switch node '${n8nNodeName}': rule ${i + 1}'s right-hand side is not a plain literal value — configure manually.` };
    }
    cases.push({ match: matchValue, port: `case_${i + 1}` });
  }

  return { field: sharedField as string, cases: JSON.stringify(cases), source_node: sourceName };
}
