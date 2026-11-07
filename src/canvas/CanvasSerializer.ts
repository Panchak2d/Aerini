import { CanvasNode }  from "./Node";
import { Connector }  from "./Connector";
import type { CanvasNodeData } from "./Node";
import { NODE_IDS } from "../node-ids";
import { getNodeDescriptor } from "./node-registry";
import { normalizeTags } from "../workflow-tags";
import { MAX_NOTE_TEXT, MAX_FRAME_TITLE } from "./note-format";
import { NOTE_MAX_SIZE, noteSizeOf, type NoteStyle } from "./note-geometry";

// Populated by app.ts after ALL_NODES is defined
export { registerNodeDescriptors } from "./node-registry";

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

export interface AnnotationDocument {
  id: string;
  kind: NoteStyle;
  x: number;
  y: number;
  w?: number;
  h?: number;
  text?: string;
  title?: string;
  color?: string;
  size?: string;
}

const ANNOTATION_COLORS = new Set(["default", "yellow", "blue", "green", "red"]);
export const MAX_ANNOTATIONS = 500;

function finiteOr(v: unknown, fallback: number): number {
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}

function positiveSize(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.min(v, NOTE_MAX_SIZE) : null;
}

function annotationToDocument(n: CanvasNode): AnnotationDocument {
  const c = n.data.config;
  const kind = n.noteStyle;
  const text = String(c["text"] ?? "");
  const doc: AnnotationDocument = { id: n.data.id, kind, x: n.data.position.x, y: n.data.position.y };
  const w = positiveSize(c["width"]), h = positiveSize(c["height"]);
  if (w !== null) doc.w = w;
  if (h !== null) doc.h = h;
  if (kind === "frame") {
    if (text) doc.title = text.slice(0, MAX_FRAME_TITLE);
  } else if (text) {
    doc.text = text.slice(0, MAX_NOTE_TEXT);
  }
  const color = String(c["color"] ?? "default");
  if (color !== "default" && ANNOTATION_COLORS.has(color)) doc.color = color;
  const size = noteSizeOf(c);
  if (size !== "m") doc.size = size;
  return doc;
}

/** Returns null for an unknown `kind`, so a file from a newer build never turns an unrecognised annotation into a note. */
function annotationFromDocument(raw: Record<string, unknown>, used: Set<string>): { node: CanvasNode; replaced: boolean } | null {
  if (raw.kind !== undefined && raw.kind !== "note" && raw.kind !== "frame") return null;
  const style: NoteStyle = raw.kind === "frame" ? "frame" : "note";
  const id = resolveImportedId(raw.id, "note", used);
  const text = style === "frame"
    ? (typeof raw.title === "string" ? raw.title : typeof raw.text === "string" ? raw.text : "").slice(0, MAX_FRAME_TITLE)
    : (typeof raw.text === "string" ? raw.text : "").slice(0, MAX_NOTE_TEXT);
  const config: Record<string, unknown> = {
    style,
    text,
    color: typeof raw.color === "string" && ANNOTATION_COLORS.has(raw.color) ? raw.color : "default",
  };
  const size = noteSizeOf({ size: raw.size });
  if (size !== "m") config.size = size;
  const w = positiveSize(raw.w), h = positiveSize(raw.h);
  if (w !== null) config.width = w;
  if (h !== null) config.height = h;
  const node = new CanvasNode({
    id: id.id,
    node_type_id: NODE_IDS.NOTE,
    node_type: "utility",
    name: style === "frame" ? "Group" : "Note",
    config,
    credentials: {},
    position: { x: finiteOr(raw.x, 0), y: finiteOr(raw.y, 0) },
    ports: { inputs: [], outputs: [] },
    input_schema: {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  });
  return { node, replaced: id.replaced };
}

export interface WorkflowDocument {
  schema_version: string;
  id: string;
  name: string;
  description: string;
  nodes: object[];
  edges: object[];
  annotations?: AnnotationDocument[];
  metadata: {
    author: string;
    created_at: string;
    updated_at: string;
    version: string;
    tags: string[];
    /** Exclusive Workflows-sidebar collection membership. `null`/absent = Uncategorized. */
    collection_id?: string | null;
  };
  /** When true, independent branches run concurrently. Default: false (sequential). */
  parallel_execution?: boolean;
  /** Max concurrent node tasks when parallel_execution is true. Default: 8. */
  max_concurrent_nodes?: number;
  /** When true, a desktop manual run skips the 24h server ceiling. Default: false. */
  unlimited_duration?: boolean;
  /** Wall-clock limit for the whole run, in seconds. Absent = no limit. */
  max_duration_secs?: number;
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
  tags?: string[],
  unlimitedDuration?: boolean,
  collectionId?: string | null,
  maxDurationSecs?: number,
): string {
  const doc: WorkflowDocument = {
    schema_version: "1.0",
    id,
    name,
    description: "",
    // Notes and frames are canvas annotations with no Rust implementation, so
    // they are written to `annotations` and never to `nodes`. Any connector
    // attached to one is excluded too.
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
      tags: tags ?? [],
      collection_id: collectionId ?? null,
    },
  };
  const annotations = Array.from(nodes.values()).filter(n => n.data.node_type_id === NODE_IDS.NOTE);
  if (annotations.length) doc.annotations = annotations.map(annotationToDocument);
  if (parallelExecution) {
    doc.parallel_execution = true;
    if (maxConcurrentNodes !== undefined && maxConcurrentNodes !== 8) {
      doc.max_concurrent_nodes = maxConcurrentNodes;
    }
  }
  if (unlimitedDuration) {
    doc.unlimited_duration = true;
  }
  if (maxDurationSecs !== undefined) {
    doc.max_duration_secs = maxDurationSecs;
  }
  if (chatSettings && !chatSettingsEqualDefault(chatSettings)) {
    doc.settings = { chat: chatSettings };
  }
  return JSON.stringify(doc, null, 2);
}

// Matches every id already produced in this codebase (node_..., e_..., wf_...,
// hand-authored ones like "n1"/"wf-1" in docs and fixtures) while excluding
// HTML/JS metacharacters and unbounded length — defense in depth on top of
// the escaping already done at every render site, not a replacement for it.
const IMPORTED_ID_RE = /^[\w.-]{1,256}$/;

// Accepts `raw` as an id only if it's a non-empty string matching
// IMPORTED_ID_RE and not already used in this document; otherwise generates
// a fresh one with the existing `${prefix}_${Date.now()}_${random}` scheme.
// A genuinely absent id (undefined/null) isn't flagged as `replaced` — that's
// the normal case for a hand-built or freshly-created node, not bad input.
function resolveImportedId(raw: unknown, prefix: string, used: Set<string>): { id: string; replaced: boolean } {
  const present  = raw !== undefined && raw !== null;
  const candidate = present ? String(raw) : "";
  const valid = present && IMPORTED_ID_RE.test(candidate) && !used.has(candidate);
  const id = valid ? candidate : `${prefix}_${Date.now()}_${Math.random().toString(36).slice(2)}`;
  used.add(id);
  return { id, replaced: present && !valid };
}

export function deserialize(json: string): {
  id: string;
  name: string;
  nodes: Map<string, CanvasNode>;
  connectors: Map<string, Connector>;
  parallelExecution: boolean;
  maxConcurrentNodes: number;
  unlimitedDuration: boolean;
  chatSettings: ChatSettings;
  tags: string[];
  collectionId: string | null;
  maxDurationSecs: number | undefined;
  /** Human-readable notes on data this import had to correct (invalid/duplicate ids, dropped dangling edges). Empty when nothing needed fixing. */
  importWarnings: string[];
} {
  const doc = JSON.parse(json) as {
    id?: string; name?: string;
    nodes?: unknown[]; edges?: unknown[]; annotations?: unknown[];
    parallel_execution?: boolean;
    max_concurrent_nodes?: number;
    unlimited_duration?: boolean;
    max_duration_secs?: number | null;
    settings?: { chat?: Partial<ChatSettings> };
    metadata?: { tags?: string[]; collection_id?: string | null };
  };

  const id   = doc.id   ?? `wf_${Date.now()}`;
  const name = doc.name ?? "Untitled";
  const nodes      = new Map<string, CanvasNode>();
  const connectors = new Map<string, Connector>();
  const usedNodeIds = new Set<string>();
  const usedEdgeIds = new Set<string>();
  let fixedNodeIds = 0;
  let fixedEdgeIds = 0;
  let droppedEdges = 0;
  let skippedAnnotations = 0;
  let keptAnnotations = 0;
  const annotationIds = new Set<string>();

  const addAnnotation = (rawAnnotation: Record<string, unknown>): void => {
    if (keptAnnotations >= MAX_ANNOTATIONS) { skippedAnnotations++; return; }
    try {
      const made = annotationFromDocument(rawAnnotation, usedNodeIds);
      if (!made) { skippedAnnotations++; return; }
      if (made.replaced) fixedNodeIds++;
      nodes.set(made.node.data.id, made.node);
      annotationIds.add(made.node.data.id);
      keptAnnotations++;
    } catch { skippedAnnotations++; }
  };

  for (const raw of (doc.nodes ?? []) as Array<Record<string, unknown>>) {
    if (raw.node_type_id === NODE_IDS.NOTE) {
      const cfg = (raw.config as Record<string, unknown> | undefined) ?? {};
      const pos = (raw.position as { x?: unknown; y?: unknown } | undefined) ?? {};
      addAnnotation({ id: raw.id, kind: cfg["style"] === "frame" ? "frame" : "note", x: pos.x, y: pos.y, w: cfg["width"], h: cfg["height"], text: cfg["text"], color: cfg["color"] });
      continue;
    }
    try {
      const nodeId = resolveImportedId(raw.id, "node", usedNodeIds);
      if (nodeId.replaced) fixedNodeIds++;
      const data: CanvasNodeData = {
        id:            nodeId.id,
        node_type_id:  String(raw.node_type_id ?? NODE_IDS.MANUAL_TRIGGER),
        node_type:     (raw.node_type as "action"|"ai"|"logic"|"utility") ?? "action",
        name:          String(raw.name         ?? "Node"),
        config:        (raw.config             as Record<string, unknown>) ?? {},
        credentials:   (raw.credentials        as Record<string, string>)  ?? {},
        position:      (raw.position           as { x: number; y: number }) ?? { x: 100, y: 100 },
        ports:         (raw.ports as { inputs: Array<{id:string;label:string;position:"left"|"right"|"top"|"bottom"}>; outputs: Array<{id:string;label:string;position:"left"|"right"|"top"|"bottom"}>}) ?? getNodeDescriptor(String(raw.node_type_id ?? ""))?.ports ?? { inputs: [], outputs: [] },
        input_schema:  (raw.input_schema  as Record<string, unknown>) ?? {},
        output_schema: (raw.output_schema as Record<string, unknown>) ?? {},
        retry:         (raw.retry as { max_attempts: number; backoff_ms: number }) ?? { max_attempts: 1, backoff_ms: 500 },
        fallback_node: (raw.fallback_node as string | null) ?? null,
        // Old saved workflows predate this field — absent means enabled.
        disabled:      (raw.disabled as boolean | undefined) ?? false,
        // Prefer the live registry value (authoritative) over saved JSON, since
        // saved files may predate the dynamic_ports field.
        dynamic_ports: getNodeDescriptor(String(raw.node_type_id ?? ""))?.dynamic_ports ?? (raw.dynamic_ports as boolean | undefined) ?? false,
      };
      const n = new CanvasNode(data);
      nodes.set(n.data.id, n);
    } catch { /* skip malformed node */ }
  }

  if (Array.isArray(doc.annotations)) {
    for (const raw of doc.annotations as unknown[]) {
      if (typeof raw !== "object" || raw === null || Array.isArray(raw)) { skippedAnnotations++; continue; }
      addAnnotation(raw as Record<string, unknown>);
    }
  }

  for (const raw of (doc.edges ?? []) as Array<Record<string, unknown>>) {
    try {
      const edgeId = resolveImportedId(raw.id, "e", usedEdgeIds);
      if (edgeId.replaced) fixedEdgeIds++;
      const c = new Connector({
        id:         edgeId.id,
        from_node:  String(raw.from_node ?? ""),
        from_port:  String(raw.from_port ?? "output"),
        to_node:    String(raw.to_node   ?? ""),
        to_port:    String(raw.to_port   ?? "input"),
        condition:  (raw.condition  as string | null) ?? null,
        on_success: (raw.on_success as string | null) ?? null,
        on_failure: (raw.on_failure as string | null) ?? null,
      });
      const ends = [c.data.from_node, c.data.to_node];
      if (ends.every(id => nodes.has(id) && !annotationIds.has(id))) {
        connectors.set(c.data.id, c);
      } else {
        droppedEdges++;
      }
    } catch { /* skip malformed edge */ }
  }

  const importWarnings: string[] = [];
  if (fixedNodeIds) importWarnings.push(`${fixedNodeIds} node id${fixedNodeIds === 1 ? "" : "s"} had an invalid or duplicate format and ${fixedNodeIds === 1 ? "was" : "were"} replaced.`);
  if (fixedEdgeIds) importWarnings.push(`${fixedEdgeIds} edge id${fixedEdgeIds === 1 ? "" : "s"} had an invalid or duplicate format and ${fixedEdgeIds === 1 ? "was" : "were"} replaced.`);
  if (droppedEdges) importWarnings.push(`${droppedEdges} edge${droppedEdges === 1 ? "" : "s"} referencing a missing node ${droppedEdges === 1 ? "was" : "were"} dropped.`);

  if (skippedAnnotations) importWarnings.push(`${skippedAnnotations} note${skippedAnnotations === 1 ? "" : "s"} or frame${skippedAnnotations === 1 ? "" : "s"} could not be read and ${skippedAnnotations === 1 ? "was" : "were"} skipped.`);

  return {
    id,
    name,
    nodes,
    connectors,
    parallelExecution: doc.parallel_execution ?? false,
    maxConcurrentNodes: doc.max_concurrent_nodes ?? 8,
    unlimitedDuration: doc.unlimited_duration ?? false,
    chatSettings: { ...DEFAULT_CHAT_SETTINGS, ...(doc.settings?.chat ?? {}) },
    tags: normalizeTags(doc.metadata?.tags),
    collectionId: typeof doc.metadata?.collection_id === "string" ? doc.metadata.collection_id : null,
    maxDurationSecs: typeof doc.max_duration_secs === "number" ? doc.max_duration_secs : undefined,
    importWarnings,
  };
}
