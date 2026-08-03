import { CanvasNode }  from "./Node";
import { Connector }  from "./Connector";
import type { CanvasNodeData } from "./Node";
import type { NodeDescriptor } from "../ipc/workflow";
import { NODE_IDS } from "../node-ids";

// Populated by app.ts after ALL_NODES is defined
let _nodeRegistry: Map<string, NodeDescriptor> = new Map();

export function registerNodeDescriptors(descriptors: NodeDescriptor[]): void {
  _nodeRegistry = new Map(descriptors.map(d => [d.type_id, d]));
}

/**
 * Per-workflow Chat Panel feature toggles. Field names and defaults
 * mirror aerini-engine::model::ChatSettings exactly — this is the wire
 * contract for the "settings.chat" key in workflow JSON.
 */
export interface ChatSettings {
  allow_attachments: boolean;
  allow_image_responses: boolean;
  max_message_length: number;
  session_persistence: boolean;
  show_branding: boolean;
}

export const DEFAULT_CHAT_SETTINGS: ChatSettings = {
  allow_attachments: false,
  allow_image_responses: true,
  max_message_length: 2000,
  session_persistence: true,
  show_branding: true,
};

function chatSettingsEqualDefault(s: ChatSettings): boolean {
  return s.allow_attachments === DEFAULT_CHAT_SETTINGS.allow_attachments
      && s.allow_image_responses === DEFAULT_CHAT_SETTINGS.allow_image_responses
      && s.max_message_length === DEFAULT_CHAT_SETTINGS.max_message_length
      && s.session_persistence === DEFAULT_CHAT_SETTINGS.session_persistence
      && s.show_branding === DEFAULT_CHAT_SETTINGS.show_branding;
}

export interface WorkflowDocument {
  schema_version: string;
  id: string;
  name: string;
  description: string;
  nodes: object[];
  edges: object[];
  metadata: {
    author: string;
    created_at: string;
    updated_at: string;
    version: string;
    tags: string[];
  };
  /** When true, independent branches run concurrently. Default: false (sequential). */
  parallel_execution?: boolean;
  /** Max concurrent node tasks when parallel_execution is true. Default: 8. */
  max_concurrent_nodes?: number;
  /** Workflow-scoped feature settings — currently only the Chat panel toggles. */
  settings?: { chat?: Partial<ChatSettings> };
}

export function serialize(
  id: string,
  name: string,
  nodes: Map<string, CanvasNode>,
  connectors: Map<string, Connector>,
  parallelExecution?: boolean,
  maxConcurrentNodes?: number,
  chatSettings?: ChatSettings,
): string {
  const doc: WorkflowDocument = {
    schema_version: "1.0",
    id,
    name,
    description: "",
    // Note nodes are canvas annotations — they have no Rust implementation
    // and must not appear in the JSON sent to run_workflow. Any connector
    // attached to a note is also excluded.
    nodes: Array.from(nodes.values())
      .filter(n => n.data.node_type_id !== NODE_IDS.NOTE)
      .map(n => n.toWorkflowNode()),
    edges: Array.from(connectors.values())
      .filter(c => {
        const from = nodes.get(c.data.from_node);
        const to   = nodes.get(c.data.to_node);
        // A missing endpoint means the edge is dangling — never ship it to
        // the engine, whatever produced it. This is a hard filter,
        // independent of the NOTE-exclusion check below.
        if (!from || !to) return false;
        return from.data.node_type_id !== NODE_IDS.NOTE && to.data.node_type_id !== NODE_IDS.NOTE;
      })
      .map(c => c.toWorkflowEdge()),
    metadata: {
      author: "user",
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      version: "1.0.0",
      tags: [],
    },
  };
  if (parallelExecution) {
    doc.parallel_execution = true;
    if (maxConcurrentNodes !== undefined && maxConcurrentNodes !== 8) {
      doc.max_concurrent_nodes = maxConcurrentNodes;
    }
  }
  if (chatSettings && !chatSettingsEqualDefault(chatSettings)) {
    doc.settings = { chat: chatSettings };
  }
  return JSON.stringify(doc, null, 2);
}

export function deserializeWorkflowName(json: string): string {
  try { return JSON.parse(json).name ?? "Untitled"; }
  catch { return "Untitled"; }
}

export function deserialize(json: string): {
  id: string;
  name: string;
  nodes: Map<string, CanvasNode>;
  connectors: Map<string, Connector>;
  parallelExecution: boolean;
  maxConcurrentNodes: number;
  chatSettings: ChatSettings;
} {
  const doc = JSON.parse(json) as {
    id?: string; name?: string;
    nodes?: unknown[]; edges?: unknown[];
    parallel_execution?: boolean;
    max_concurrent_nodes?: number;
    settings?: { chat?: Partial<ChatSettings> };
  };

  const id   = doc.id   ?? `wf_${Date.now()}`;
  const name = doc.name ?? "Untitled";
  const nodes      = new Map<string, CanvasNode>();
  const connectors = new Map<string, Connector>();

  for (const raw of (doc.nodes ?? []) as Array<Record<string, unknown>>) {
    try {
      const data: CanvasNodeData = {
        id:            String(raw.id           ?? `node_${Date.now()}_${Math.random().toString(36).slice(2)}`),
        node_type_id:  String(raw.node_type_id ?? NODE_IDS.MANUAL_TRIGGER),
        node_type:     (raw.node_type as "action"|"ai"|"logic"|"utility") ?? "action",
        name:          String(raw.name         ?? "Node"),
        config:        (raw.config             as Record<string, unknown>) ?? {},
        credentials:   (raw.credentials        as Record<string, string>)  ?? {},
        position:      (raw.position           as { x: number; y: number }) ?? { x: 100, y: 100 },
        ports:         (raw.ports as { inputs: Array<{id:string;label:string;position:"left"|"right"|"top"|"bottom"}>; outputs: Array<{id:string;label:string;position:"left"|"right"|"top"|"bottom"}>}) ?? _nodeRegistry.get(String(raw.node_type_id ?? ""))?.ports ?? { inputs: [], outputs: [] },
        input_schema:  (raw.input_schema  as Record<string, unknown>) ?? {},
        output_schema: (raw.output_schema as Record<string, unknown>) ?? {},
        retry:         (raw.retry as { max_attempts: number; backoff_ms: number }) ?? { max_attempts: 1, backoff_ms: 500 },
        fallback_node: (raw.fallback_node as string | null) ?? null,
        // Old saved workflows predate this field — absent means enabled.
        disabled:      (raw.disabled as boolean | undefined) ?? false,
        // Prefer the live registry value (authoritative) over saved JSON, since
        // saved files may predate the dynamic_ports field.
        dynamic_ports: _nodeRegistry.get(String(raw.node_type_id ?? ""))?.dynamic_ports ?? (raw.dynamic_ports as boolean | undefined) ?? false,
      };
      const n = new CanvasNode(data);
      nodes.set(n.data.id, n);
    } catch { /* skip malformed node */ }
  }

  for (const raw of (doc.edges ?? []) as Array<Record<string, unknown>>) {
    try {
      const c = new Connector({
        id:         String(raw.id        ?? `e_${Date.now()}_${Math.random().toString(36).slice(2)}`),
        from_node:  String(raw.from_node ?? ""),
        from_port:  String(raw.from_port ?? "output"),
        to_node:    String(raw.to_node   ?? ""),
        to_port:    String(raw.to_port   ?? "input"),
        condition:  (raw.condition  as string | null) ?? null,
        on_success: (raw.on_success as string | null) ?? null,
        on_failure: (raw.on_failure as string | null) ?? null,
      });
      if (nodes.has(c.data.from_node) && nodes.has(c.data.to_node)) {
        connectors.set(c.data.id, c);
      }
    } catch { /* skip malformed edge */ }
  }

  return {
    id,
    name,
    nodes,
    connectors,
    parallelExecution: doc.parallel_execution ?? false,
    maxConcurrentNodes: doc.max_concurrent_nodes ?? 8,
    chatSettings: { ...DEFAULT_CHAT_SETTINGS, ...(doc.settings?.chat ?? {}) },
  };
}
