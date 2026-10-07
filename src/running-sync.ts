import { NODE_IDS } from "./node-ids";
import { canonicalJson } from "./canonical-json";

interface GraphNode { id: string; node_type_id: string; config: unknown }
interface GraphEdge { from_node: string; to_node: string }

/**
 * Identity of the part of a workflow the scheduler freezes when it starts a
 * job: the entry node (first node with no incoming edge) and its config. The
 * rest of the graph is re-read from the saved workflow on every run, so it
 * deliberately plays no part here — node positions, names and wiring changes
 * never change this value.
 */
export function triggerFingerprint(nodes: Iterable<GraphNode>, edges: Iterable<GraphEdge>): string | null {
  const real = Array.from(nodes).filter(n => n.node_type_id !== NODE_IDS.NOTE);
  const ids = new Set(real.map(n => n.id));
  const hasIncoming = new Set<string>();
  for (const e of edges) if (ids.has(e.from_node) && ids.has(e.to_node)) hasIncoming.add(e.to_node);
  const entry = real.find(n => !hasIncoming.has(n.id));
  return entry ? `${entry.node_type_id}:${canonicalJson(entry.config ?? {})}` : null;
}

interface GraphSource {
  nodes: Map<string, { data: GraphNode }>;
  connectors: Map<string, { data: GraphEdge }>;
}

export function canvasTriggerFingerprint(canvas: GraphSource): string | null {
  return triggerFingerprint(
    Array.from(canvas.nodes.values(), n => n.data),
    Array.from(canvas.connectors.values(), c => c.data),
  );
}

export function fingerprintFromJson(json: string): string | null {
  try {
    const doc = JSON.parse(json) as { nodes?: GraphNode[]; edges?: GraphEdge[] };
    return triggerFingerprint(doc.nodes ?? [], doc.edges ?? []);
  } catch {
    return null;
  }
}

/**
 * Tracks, per background job, the trigger it started with and whether a run is
 * executing, so the UI can tell when the canvas's trigger no longer matches the
 * running job.
 */
export class RunningSync {
  private armed = new Map<string, string | null>();
  private loading = new Set<string>();
  private inFlight = new Set<string>();
  private listeners = new Set<() => void>();

  constructor(private loadStored: (id: string) => Promise<string | null>) {}

  onChange(cb: () => void): () => void {
    this.listeners.add(cb);
    return () => { this.listeners.delete(cb); };
  }

  notify(): void {
    for (const cb of this.listeners) cb();
  }

  observe(workflowId: string, status: string): void {
    if (status === "error") {
      // A failed run still leaves the job armed; only a stop or a one-shot finish ends it.
      this.inFlight.delete(workflowId);
      this.notify();
      return;
    }
    if (status !== "running" && status !== "waiting") {
      this.armed.delete(workflowId);
      this.inFlight.delete(workflowId);
      this.notify();
      return;
    }
    if (status === "running") this.inFlight.add(workflowId); else this.inFlight.delete(workflowId);
    if (!this.armed.has(workflowId) && !this.loading.has(workflowId)) {
      this.loading.add(workflowId);
      this.loadStored(workflowId)
        .then(json => { if (json !== null && !this.armed.has(workflowId)) this.armed.set(workflowId, fingerprintFromJson(json)); })
        .catch(() => {})
        .finally(() => { this.loading.delete(workflowId); this.notify(); });
    }
    this.notify();
  }

  isRunInFlight(workflowId: string): boolean {
    return this.inFlight.has(workflowId);
  }

  /** True when the job is running with a different trigger than `canvasFingerprint`. */
  triggerChanged(workflowId: string, canvasFingerprint: string | null): boolean {
    return this.armed.has(workflowId) && this.armed.get(workflowId) !== canvasFingerprint;
  }
}
