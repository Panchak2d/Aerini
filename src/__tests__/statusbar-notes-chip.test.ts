// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowManager } from "../workflow-manager";
import { initStatusBarFields } from "../statusbar-fields";

function fakeCanvas(noteCount: number, nodeCount: number, notesVisible: boolean) {
  const nodes = new Map<string, { isNote: boolean }>();
  for (let i = 0; i < noteCount; i++) nodes.set(`note${i}`, { isNote: true });
  for (let i = 0; i < nodeCount; i++) nodes.set(`node${i}`, { isNote: false });
  return { nodes, connectors: new Map(), selectedNodes: new Set(), zoom: 1, notesVisible } as unknown as Canvas;
}

describe("status bar notes chip", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = `
      <div id="canvas-stat-bar"></div>
      <div id="status-bar"><button class="notes-chip hidden" id="notes-chip"></button></div>`;
  });
  afterEach(() => { vi.useRealTimers(); });

  const chip = () => document.getElementById("notes-chip")!;

  it("shows only while notes are hidden and the workflow has at least one, and a click shows them again", () => {
    const canvas = fakeCanvas(2, 3, false);
    initStatusBarFields(canvas, {} as WorkflowManager);
    vi.advanceTimersByTime(200);
    expect(chip().classList.contains("hidden")).toBe(false);

    chip().click();
    expect(canvas.notesVisible).toBe(true);
    vi.advanceTimersByTime(200);
    expect(chip().classList.contains("hidden")).toBe(true);
  });

  it("stays hidden when nothing would be revealed, and the node count leaves notes out", () => {
    initStatusBarFields(fakeCanvas(0, 4, false), {} as WorkflowManager);
    vi.advanceTimersByTime(200);
    expect(chip().classList.contains("hidden")).toBe(true);
    expect(document.getElementById("stat-nodes")!.textContent).toBe("4");
  });
});
