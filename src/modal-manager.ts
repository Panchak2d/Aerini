import type { NodeDescriptor } from "./ipc/workflow";
import type { Canvas } from "./canvas/Canvas";

type ImportCallback = (obj: Record<string, unknown>) => void;
type ToastFn = (msg: string, type: "success" | "error" | "info") => void;

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
  snapCb.checked = localStorage.getItem("flowo_grid_snap") === "true";
  fitCb.checked  = localStorage.getItem("flowo_autofit") !== "false";
  snapCb.addEventListener("change", () => localStorage.setItem("flowo_grid_snap", String(snapCb.checked)));
  fitCb.addEventListener("change",  () => localStorage.setItem("flowo_autofit",  String(fitCb.checked)));
}

export function showImportPreview(obj: Record<string, unknown>): void {
  const converted = isN8nWorkflow(obj) ? convertN8nWorkflow(obj) : obj;
  _pendingImport = converted;
  document.getElementById("import-name")!.textContent        = String(obj.name ?? "Untitled");
  document.getElementById("import-desc")!.textContent        = String(obj.description ?? "");
  document.getElementById("import-node-count")!.textContent  = String((obj.nodes as unknown[])?.length ?? 0);
  document.getElementById("import-edge-count")!.textContent  = String((obj.edges as unknown[])?.length ?? 0);
  document.getElementById("import-version")!.textContent     = String(obj.flowo_version ?? obj.schema_version ?? "?");

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
  return localStorage.getItem("flowo_grid_snap") === "true";
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
  "n8n-nodes-base.manualTrigger":      "manual_trigger",
  "n8n-nodes-base.webhook":            "webhook",
  "n8n-nodes-base.scheduleTrigger":    "schedule",
  "n8n-nodes-base.httpRequest":        "http_request",
  "n8n-nodes-base.code":               "code",
  "n8n-nodes-base.if":                 "if_condition",
  "n8n-nodes-base.switch":             "switch",
  "n8n-nodes-base.emailSend":          "email_send",
  "n8n-nodes-base.readWriteFile":      "file",
  "n8n-nodes-base.set":                "set_variable",
  "n8n-nodes-base.wait":               "wait",
  "n8n-nodes-base.noOp":               "output",
  "n8n-nodes-base.stickyNote":         "note",
  "n8n-nodes-base.merge":              "merge",
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
  "n8n-nodes-base.function":           "code",
};

function convertN8nWorkflow(n8n: Record<string, unknown>): Record<string, unknown> {
  const n8nNodes = (n8n.nodes as Array<Record<string, unknown>>) ?? [];
  const n8nConns = (n8n.connections as Record<string, unknown>) ?? {};

  const nodeIdMap = new Map<string, string>(); // n8n name → flowo id

  const flowoNodes = n8nNodes.map((n: Record<string, unknown>, idx: number) => {
    const flowoId = `node_imported_${idx}`;
    const n8nName = String(n.name ?? `Node ${idx}`);
    nodeIdMap.set(n8nName, flowoId);

    const typeId = N8N_TYPE_MAP[String(n.type ?? "")] ?? "unsupported";
    const pos = n.position as number[] | undefined;

    return {
      id:           flowoId,
      node_type_id: typeId,
      node_type:    getNodeCategory(typeId),
      name:         n8nName,
      config:       n.parameters ?? {},
      credentials:  {},
      ports:        getDefaultPorts(typeId),
      input_schema:  { type: "object", properties: {} },
      output_schema: { type: "object" },
      retry:        { max_attempts: 1, backoff_ms: 500 },
      fallback_node: null,
      position:     { x: Array.isArray(pos) ? (pos[0] ?? 0) + 400 : 100, y: Array.isArray(pos) ? (pos[1] ?? 0) + 200 : 100 },
    };
  });

  // Convert n8n connection format to Flowo edges
  const flowoEdges: unknown[] = [];
  let edgeIdx = 0;
  for (const [fromName, conns] of Object.entries(n8nConns)) {
    const fromId = nodeIdMap.get(fromName);
    if (!fromId) continue;
    const outputs = (conns as Record<string, unknown>).main as Array<Array<Record<string, unknown>>> | undefined;
    if (!Array.isArray(outputs)) continue;
    outputs.forEach((portConns, portIdx) => {
      if (!Array.isArray(portConns)) return;
      portConns.forEach((c: Record<string, unknown>) => {
        const toId = nodeIdMap.get(String(c.node ?? ""));
        if (!toId) return;
        flowoEdges.push({
          id:        `edge_imported_${edgeIdx++}`,
          from_node: fromId,
          from_port: portIdx === 0 ? "output" : `out_${portIdx}`,
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
    nodes: flowoNodes,
    edges: flowoEdges,
    metadata: { author: "imported", created_at: new Date().toISOString(), updated_at: new Date().toISOString(), version: "1.0.0", tags: ["n8n-import"] },
  };
}

function getNodeCategory(typeId: string): "action" | "logic" | "utility" | "ai" {
  const logic   = ["if_condition","switch","loop","stop"];
  const utility = ["delay","wait","transform","json","set_variable","get_variable","output","note"];
  const ai      = ["ai_prompt","ai_agent","ai_memory","text_splitter"];
  if (logic.includes(typeId))   return "logic";
  if (utility.includes(typeId)) return "utility";
  if (ai.includes(typeId))      return "ai";
  return "action";
}

function getDefaultPorts(typeId: string): { inputs: Array<{id:string;label:string;position:string}>; outputs: Array<{id:string;label:string;position:string}> } {
  const noInput = ["manual_trigger","webhook","schedule"];
  const inputs = noInput.includes(typeId) ? [] : [{ id: "input", label: "In", position: "left" }];
  const outputs = typeId === "stop" ? [] : [{ id: "output", label: "Out", position: "right" }];
  return { inputs, outputs };
}
