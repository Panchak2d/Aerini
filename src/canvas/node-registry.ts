import type { NodeDescriptor } from "../ipc/workflow";
import { DANGEROUS_NODE_IDS } from "../node-ids";

// Leaf module: imports only a type. Node.ts and CanvasSerializer.ts both read
// the registry, and CanvasSerializer.ts imports Node.ts, so the registry can't
// live in either without creating an import cycle.
let _registry: Map<string, NodeDescriptor> = new Map();

export function registerNodeDescriptors(descriptors: NodeDescriptor[]): void {
  _registry = new Map(descriptors.map(d => [d.type_id, d]));
}

export function getNodeDescriptor(typeId: string): NodeDescriptor | undefined {
  return _registry.get(typeId);
}

/** Every currently-registered descriptor, in the order last passed to
 *  registerNodeDescriptors(). The shared source for callers that need the
 *  full list rather than a single type_id lookup (e.g. the command palette
 *  search). */
export function getAllNodeDescriptors(): NodeDescriptor[] {
  return [..._registry.values()];
}

/** True when `typeId` was loaded from a WASM plugin rather than built in. */
export function isPluginNodeType(typeId: string): boolean {
  return _registry.get(typeId)?.is_plugin === true;
}

/**
 * True when a registry that has actually loaded has no descriptor for `typeId`.
 * An empty registry means "not loaded" (the startup fetch failed, or there is
 * no backend to ask), not "nothing is installed" -- the backend always
 * registers the built-ins -- so it never reports a type as unregistered.
 */
export function isUnregisteredNodeType(typeId: string): boolean {
  return _registry.size > 0 && !_registry.has(typeId);
}

/**
 * True for nodes that run code on the user's machine: the built-in dangerous
 * set plus every plugin. Drives the run-time confirmation gate. The canvas
 * warning badge is deliberately narrower (built-in set only); plugin nodes get
 * the neutral "P" chip instead.
 */
export function isDangerousNodeType(typeId: string): boolean {
  return DANGEROUS_NODE_IDS.has(typeId) || isPluginNodeType(typeId);
}
