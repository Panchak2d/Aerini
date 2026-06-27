import type { Canvas } from "./Canvas";
import type { CanvasNode } from "./Node";
import type { Connector } from "./Connector";

export type MoveEntry = { nodeId: string; from: { x: number; y: number }; to: { x: number; y: number } };

export type UndoAction =
  | { type: "add_node";    node: CanvasNode }
  | { type: "delete_node"; node: CanvasNode; connectors: Connector[] }
  | { type: "add_edge";    connector: Connector }
  | { type: "delete_edge"; connector: Connector }
  | { type: "split_edge";  removed: Connector; added1: Connector; added2: Connector; node: CanvasNode }
  | { type: "move_node";   nodeId: string; from: { x: number; y: number }; to: { x: number; y: number } }
  | { type: "move_nodes";  moves: MoveEntry[] }
  | { type: "cut_edges";   connectors: Connector[] };

export class UndoManager {
  private canvas: Canvas;
  private stack: UndoAction[] = [];
  private redos: UndoAction[] = [];

  constructor(canvas: Canvas) {
    this.canvas = canvas;
  }

  push(a: UndoAction): void {
    this.stack.push(a);
    if (this.stack.length > 100) this.stack.shift();
    this.redos = [];
  }

  undo(): void {
    const a = this.stack.pop();
    if (!a) return;
    this.redos.push(a);
    const c = this.canvas;
    switch (a.type) {
      case "add_node":
        c.nodes.delete(a.node.data.id);
        break;
      case "delete_node":
        c.nodes.set(a.node.data.id, a.node);
        for (const conn of a.connectors) {
          c.connectors.set(conn.data.id, conn);
          c.injectDynamicPortExpr(conn);
        }
        break;
      case "add_edge":
        c.connectors.delete(a.connector.data.id);
        c.clearDynamicPortExpr(a.connector);
        break;
      case "delete_edge":
        c.connectors.set(a.connector.data.id, a.connector);
        c.injectDynamicPortExpr(a.connector);
        break;
      case "move_node": {
        const n = c.nodes.get(a.nodeId);
        if (n) { n.data.position = { ...a.from }; n.updatePortPositions(); }
        break;
      }
      case "move_nodes":
        for (const m of a.moves) {
          const n = c.nodes.get(m.nodeId);
          if (n) { n.data.position = { ...m.from }; n.updatePortPositions(); }
        }
        break;
      case "cut_edges":
        for (const conn of a.connectors) {
          c.connectors.set(conn.data.id, conn);
          c.injectDynamicPortExpr(conn);
        }
        break;
      case "split_edge":
        c.connectors.delete(a.added1.data.id);
        c.connectors.delete(a.added2.data.id);
        c.clearDynamicPortExpr(a.added2);
        c.nodes.delete(a.node.data.id);
        c.connectors.set(a.removed.data.id, a.removed);
        c.injectDynamicPortExpr(a.removed);
        break;
    }
    c.onCanvasChanged?.();
  }

  redo(): void {
    const a = this.redos.pop();
    if (!a) return;
    this.stack.push(a);
    const c = this.canvas;
    switch (a.type) {
      case "add_node":
        c.nodes.set(a.node.data.id, a.node);
        break;
      case "delete_node":
        c.nodes.delete(a.node.data.id);
        for (const conn of a.connectors) {
          c.clearDynamicPortExpr(conn);
          c.connectors.delete(conn.data.id);
        }
        break;
      case "add_edge":
        c.connectors.set(a.connector.data.id, a.connector);
        c.injectDynamicPortExpr(a.connector);
        break;
      case "delete_edge":
        c.connectors.delete(a.connector.data.id);
        c.clearDynamicPortExpr(a.connector);
        break;
      case "move_node": {
        const n = c.nodes.get(a.nodeId);
        if (n) { n.data.position = { ...a.to }; n.updatePortPositions(); }
        break;
      }
      case "move_nodes":
        for (const m of a.moves) {
          const n = c.nodes.get(m.nodeId);
          if (n) { n.data.position = { ...m.to }; n.updatePortPositions(); }
        }
        break;
      case "cut_edges":
        for (const conn of a.connectors) {
          c.clearDynamicPortExpr(conn);
          c.connectors.delete(conn.data.id);
        }
        break;
      case "split_edge":
        c.clearDynamicPortExpr(a.removed);
        c.connectors.delete(a.removed.data.id);
        c.nodes.set(a.node.data.id, a.node);
        c.connectors.set(a.added1.data.id, a.added1);
        c.connectors.set(a.added2.data.id, a.added2);
        c.injectDynamicPortExpr(a.added2);
        break;
    }
    c.onCanvasChanged?.();
  }
}
