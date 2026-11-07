import { storageGet } from "../safe-storage";

export const GRID_SIZE = 20;

export function gridSnapEnabled(): boolean {
  return storageGet("aerini_grid_snap") === "true";
}

export function snapToGrid(v: number): number {
  return Math.round(v / GRID_SIZE) * GRID_SIZE;
}
