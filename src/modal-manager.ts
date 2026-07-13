import type { NodeDescriptor } from "./ipc/workflow";
import { NODE_IDS, TRIGGER_NODE_IDS } from "./node-ids";

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
}

export function showImportPreview(obj: Record<string, unknown>): void {
  const converted = isN8nWorkflow(obj) ? convertN8nWorkflow(obj) : obj;
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

  const nodeIdMap   = new Map<string, string>(); // n8n name → aerini id
  const nodeTypeMap = new Map<string, string>(); // aerini id → aerini node_type_id (for edge port resolution below)

  const aeriniNodes = n8nNodes.map((n: Record<string, unknown>, idx: number) => {
    const aeriniId = `node_imported_${idx}`;
    const n8nName = String(n.name ?? `Node ${idx}`);
    nodeIdMap.set(n8nName, aeriniId);

    const typeId = N8N_TYPE_MAP[String(n.type ?? "")] ?? "unsupported";
    nodeTypeMap.set(aeriniId, typeId);
    const pos = n.position as number[] | undefined;

    return {
      id:           aeriniId,
      node_type_id: typeId,
      node_type:    getNodeCategory(typeId),
      name:         n8nName,
      config:       n.parameters ?? {},
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

  return {
    schema_version: "1.0",
    id:   `wf_imported_${Date.now()}`,
    name: String(n8n.name ?? "Imported from n8n"),
    description: "Imported from n8n workflow",
    nodes: aeriniNodes,
    edges: aeriniEdges,
    metadata: { author: "imported", created_at: new Date().toISOString(), updated_at: new Date().toISOString(), version: "1.0.0", tags: ["n8n-import"] },
  };
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
