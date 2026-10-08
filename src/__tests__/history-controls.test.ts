/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { UndoManager } from "../canvas/UndoManager";
import type { HistoryState } from "../canvas/UndoManager";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), convertFileSrc: vi.fn((p: string) => p) }));

import { bindHistoryControls } from "../toolbar";

const state = (o: Partial<HistoryState>): HistoryState =>
  ({ kind: "push", canUndo: false, canRedo: false, undoLabel: null, redoLabel: null, ...o });

function mountDom() {
  document.body.innerHTML = `
    <div id="toolbar"><div class="toolbar-divider" id="toolbar-divider-new-workflow"></div>
    <div class="toolbar-zoom-group"></div></div>
    <span id="status-text">Ready</span><div id="a11y-announcer"></div>`;
}

function setup() {
  mountDom();
  const canvas = {
    onHistoryChange: null as ((s: HistoryState) => void) | null,
    historyState: () => state({}),
    canUndo: vi.fn(), undo: vi.fn(), redo: vi.fn(),
  } as unknown as Canvas & { onHistoryChange: (s: HistoryState) => void };
  const toast = vi.fn();
  bindHistoryControls(canvas, toast);
  return { canvas, toast };
}

beforeEach(() => { vi.useFakeTimers(); });

describe("bindHistoryControls", () => {
  it("buttons start disabled, enable with the step's name in the tooltip, and click through to the canvas", () => {
    const { canvas } = setup();
    const undo = document.getElementById("btn-undo") as HTMLButtonElement;
    const redo = document.getElementById("btn-redo") as HTMLButtonElement;
    expect(undo.disabled).toBe(true);
    expect(redo.disabled).toBe(true);

    canvas.onHistoryChange(state({ canUndo: true, undoLabel: 'Delete "Fetch"' }));
    expect(undo.disabled).toBe(false);
    expect(undo.title).toBe('Undo: Delete "Fetch" (Ctrl+Z)');
    expect(undo.getAttribute("aria-label")).toBe(undo.title);
    undo.click();
    expect(canvas.undo).toHaveBeenCalledTimes(1);

    canvas.onHistoryChange(state({ canRedo: true, redoLabel: "Move wire" }));
    redo.click();
    expect(canvas.redo).toHaveBeenCalledTimes(1);
    expect(undo.disabled).toBe(true);
  });

  it("announces what was undone, and tells the person when there is nothing to do or a step failed", () => {
    const { canvas, toast } = setup();
    canvas.onHistoryChange(state({ kind: "undo", label: "Move wire", canRedo: true, redoLabel: "Move wire" }));
    vi.advanceTimersByTime(50);
    expect(document.getElementById("status-text")!.textContent).toBe("Undid: Move wire");
    expect(document.getElementById("a11y-announcer")!.textContent).toBe("Undid: Move wire");

    canvas.onHistoryChange(state({ kind: "empty-undo" }));
    expect(toast).toHaveBeenCalledWith("Nothing to undo", "info");
    canvas.onHistoryChange(state({ kind: "failed", label: "Move wire" }));
    expect(toast).toHaveBeenLastCalledWith(expect.stringContaining("Move wire"), "error");
  });
});

describe("status bar and screen-reader feedback", () => {
  const status = () => document.getElementById("status-text")!.textContent;
  const announced = () => document.getElementById("a11y-announcer")!.textContent;

  it("the status message goes back to what was there once it has been shown", () => {
    const { canvas } = setup();
    canvas.onHistoryChange(state({ kind: "undo", label: "Move wire" }));
    expect(status()).toBe("Undid: Move wire");
    canvas.onHistoryChange(state({ kind: "redo", label: "Move wire" }));
    expect(status()).toBe("Redid: Move wire");

    vi.advanceTimersByTime(4000);
    expect(status()).toBe("Ready");
  });

  it("edge case: a status written by something else in the meantime is left alone", () => {
    const { canvas } = setup();
    canvas.onHistoryChange(state({ kind: "undo", label: "Move wire" }));
    document.getElementById("status-text")!.textContent = "Saved";

    vi.advanceTimersByTime(4000);
    expect(status()).toBe("Saved");
  });

  it("an identical message twice in a row is announced again", () => {
    const { canvas } = setup();
    const ev = state({ kind: "undo", label: "Move wire" });
    canvas.onHistoryChange(ev);
    vi.advanceTimersByTime(50);
    expect(announced()).toBe("Undid: Move wire");

    canvas.onHistoryChange(ev);
    expect(announced()).toBe("");
    vi.advanceTimersByTime(50);
    expect(announced()).toBe("Undid: Move wire");
  });
});

describe("workflow switch without polling", () => {
  it("replacing the canvas graph disables the buttons straight away, with no timer running", () => {
    mountDom();
    const canvas = Object.create(Canvas.prototype) as Canvas & Record<string, unknown>;
    canvas.nodes = new Map();
    canvas.connectors = new Map();
    canvas.selectedNodes = new Set();
    canvas.undoMgr = new UndoManager(canvas);
    const timers = vi.getTimerCount();
    bindHistoryControls(canvas, vi.fn());
    expect(vi.getTimerCount()).toBe(timers);

    canvas.pushUndo({ type: "move_node", nodeId: "a", from: { x: 0, y: 0 }, to: { x: 1, y: 1 } });
    const undo = document.getElementById("btn-undo") as HTMLButtonElement;
    expect(undo.disabled).toBe(false);

    canvas.nodes = new Map();
    expect(undo.disabled).toBe(true);
  });
});
