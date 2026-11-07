import type { CanvasNode, CanvasNodeData } from "./Node";
import type { ConnectorData } from "./Connector";
import { renameReferences, uniqueNodeName } from "../node-names";
import { NODE_IDS } from "../node-ids";
import { gridSnapEnabled, snapToGrid } from "./grid";

export interface ClipboardBounds { x: number; y: number; width: number; height: number }

export interface ClipboardPayload {
  nodes: CanvasNodeData[];
  edges: ConnectorData[];
  bounds: ClipboardBounds;
}

export interface PastePlan {
  nodes: CanvasNodeData[];
  edges: ConnectorData[];
}

/** Offset in world units between repeated pastes that have no pointer position. */
export const PASTE_OFFSET = 30;

// The clipboard lives in memory only and is never written to the system clipboard,
// so node configuration cannot leak into other applications.
let held: ClipboardPayload | null = null;
let pasteCount = 0;

const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

export function hasClipboard(): boolean {
  return held !== null;
}

export function heldClipboard(): ClipboardPayload | null {
  return held;
}

export function holdClipboard(payload: ClipboardPayload | null): void {
  held = payload;
  pasteCount = 0;
}

/**
 * Snapshots `nodes` and the wires with both ends inside the set. Wires to nodes
 * outside the set are dropped, and credential bindings are stripped so a paste
 * never silently reuses a secret binding.
 */
export function snapshotSelection(nodes: CanvasNode[], connectors: Iterable<{ data: ConnectorData }>): ClipboardPayload | null {
  if (!nodes.length) return null;
  const ids = new Set(nodes.map(n => n.data.id));
  const data = nodes.map(n => ({ ...clone(n.data), credentials: {} }));
  const edges: ConnectorData[] = [];
  for (const c of connectors) {
    if (ids.has(c.data.from_node) && ids.has(c.data.to_node)) edges.push(clone(c.data));
  }
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const n of nodes) {
    const p = n.data.position;
    x0 = Math.min(x0, p.x); y0 = Math.min(y0, p.y);
    x1 = Math.max(x1, p.x + n.width); y1 = Math.max(y1, p.y + n.height);
  }
  return { nodes: data, edges, bounds: { x: x0, y: y0, width: x1 - x0, height: y1 - y0 } };
}

/**
 * Top-left world position for a paste. With a pointer position the pasted block is
 * centred on it (and aligned to the grid when snapping is on); without one it lands
 * offset from where it was copied, a little further for each repeat.
 */
export function pasteOrigin(bounds: ClipboardBounds, at: { x: number; y: number } | null): { x: number; y: number } {
  if (!at) {
    const step = PASTE_OFFSET * (pasteCount + 1);
    return { x: bounds.x + step, y: bounds.y + step };
  }
  const x = at.x - bounds.width / 2;
  const y = at.y - bounds.height / 2;
  return gridSnapEnabled() ? { x: snapToGrid(x), y: snapToGrid(y) } : { x, y };
}

/**
 * Builds the data for a paste: fresh ids, names unique among `takenNames`, positions
 * shifted to `origin`, `{{Name.…}}` references between pasted nodes pointed at the
 * copies, fallback links kept only between pasted nodes, and wires remapped.
 */
export function planPaste(
  payload: ClipboardPayload,
  origin: { x: number; y: number },
  takenNames: Iterable<string>,
  newNodeId: (isNote: boolean) => string,
  newEdgeId: () => string,
): PastePlan {
  const dx = origin.x - payload.bounds.x;
  const dy = origin.y - payload.bounds.y;
  const names = new Set(takenNames);
  const idMap = new Map<string, string>();
  const renames: Array<[string, string]> = [];

  const nodes = payload.nodes.map(src => {
    const copy = clone(src);
    copy.id = newNodeId(src.node_type_id === NODE_IDS.NOTE);
    copy.name = uniqueNodeName(src.name, names);
    names.add(copy.name);
    copy.position = { x: src.position.x + dx, y: src.position.y + dy };
    idMap.set(src.id, copy.id);
    if (copy.name !== src.name) renames.push([src.name, copy.name]);
    return copy;
  });

  for (const copy of nodes) {
    copy.fallback_node = copy.fallback_node ? idMap.get(copy.fallback_node) ?? null : null;
    // Applied last to first so a new name equal to a later node's old name is not rewritten twice.
    for (let i = renames.length - 1; i >= 0; i--) {
      const [from, to] = renames[i];
      for (const [key, value] of Object.entries(copy.config)) {
        const next = renameReferences(value, from, to);
        if (next !== value) copy.config[key] = next;
      }
    }
  }

  const edges = payload.edges.flatMap((e): ConnectorData[] => {
    const from = idMap.get(e.from_node);
    const to = idMap.get(e.to_node);
    return from && to ? [{ ...clone(e), id: newEdgeId(), from_node: from, to_node: to }] : [];
  });

  pasteCount++;
  return { nodes, edges };
}
