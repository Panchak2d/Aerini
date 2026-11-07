// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";
import { ContextMenu } from "../canvas/ContextMenu";
import type { Canvas } from "../canvas/Canvas";
import type { CanvasNode } from "../canvas/Node";
import type { Connector } from "../canvas/Connector";
import { holdClipboard } from "../canvas/node-clipboard";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function fakeNode(over: { isNote: boolean; style?: string }): CanvasNode {
  return {
    isNote: over.isNote,
    noteStyle: over.style ?? "note",
    disabled: false,
    height: 60,
    width: 220,
    data: { id: "n1", name: "Thing", config: {} as Record<string, unknown> },
  } as unknown as CanvasNode;
}

function fakeCanvas(extra: Record<string, unknown> = {}) {
  const el = document.createElement("div");
  el.tabIndex = 0;
  document.body.appendChild(el);
  const canvas = {
    el, zoom: 2, panX: 10, panY: 20,
    selectedNodes: new Set<string>(["n1"]),
    nodes: new Map(),
    notesVisible: true,
    hasShownNodes: () => true,
    onPaletteRequest: vi.fn(),
    onInsertOnWireRequest: vi.fn(),
    onCanvasChanged: vi.fn(),
    addNote: vi.fn(),
    selectAll: vi.fn(),
    fitToScreen: vi.fn(),
    deleteConnector: vi.fn(),
    containedIds: vi.fn(() => [] as string[]),
    fitFrameToContents: vi.fn(),
    editNode: vi.fn((_n: unknown, mutate: () => void) => mutate()),
    openNodeConfig: vi.fn(),
    dupSelected: vi.fn(),
    deleteSelected: vi.fn(),
    frameSelection: vi.fn(),
    ...extra,
  };
  return canvas;
}

const labels = (): string[] =>
  [...document.querySelectorAll("#canvas-ctx-menu [role=menuitem]")].map(e => e.textContent ?? "");

const press = (el: Element): void => {
  el.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
};

const keydown = (key: string): void => {
  (document.activeElement ?? document.body).dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
};

beforeEach(() => {
  document.body.innerHTML = "";
  holdClipboard(null);
});

describe("canvas menu", () => {
  it("offers Add node first and hands the palette the click point in canvas pixels", () => {
    const c = fakeCanvas();
    new ContextMenu(c as unknown as Canvas).showCanvas(100, 100, { x: 5, y: 7 });

    expect(labels()[0]).toBe("Add node here…");
    press(document.querySelector("#canvas-ctx-menu [role=menuitem]")!);
    expect(c.onPaletteRequest).toHaveBeenCalledWith({ sx: 20, sy: 34 });
    expect(document.getElementById("canvas-ctx-menu")).toBeNull();
  });

  it("leaves out Select all and Fit to screen on an empty workflow", () => {
    const c = fakeCanvas({ hasShownNodes: () => false });
    new ContextMenu(c as unknown as Canvas).showCanvas(0, 0, { x: 0, y: 0 });
    expect(labels()).not.toContain("Select all");
    expect(labels()).not.toContain("Fit to screen");
  });

  it("shows Paste only while the clipboard holds something and pastes at the click point", () => {
    const pasteClipboard = vi.fn();
    const c = fakeCanvas({ pasteClipboard });
    new ContextMenu(c as unknown as Canvas).showCanvas(0, 0, { x: 7, y: 9 });
    expect(labels()).not.toContain("Paste");

    holdClipboard({ nodes: [], edges: [], bounds: { x: 0, y: 0, width: 1, height: 1 } });
    new ContextMenu(c as unknown as Canvas).showCanvas(0, 0, { x: 7, y: 9 });
    const row = [...document.querySelectorAll("#canvas-ctx-menu [role=menuitem]")].find(e => e.textContent === "Paste")!;
    press(row);
    expect(pasteClipboard).toHaveBeenCalledWith({ x: 7, y: 9 });
  });

  it("adds a pre-titled section frame and a to-do note at the click point from the templates", () => {
    const c = fakeCanvas();
    new ContextMenu(c as unknown as Canvas).showCanvas(0, 0, { x: 3, y: 4 });
    const row = (label: string): Element => [...document.querySelectorAll("#canvas-ctx-menu [role=menuitem]")].find(e => e.textContent === label)!;

    press(row("Add section frame here"));
    expect(c.addNote).toHaveBeenLastCalledWith("frame", { x: 3, y: 4 }, expect.objectContaining({ id: "section", text: "Section" }));

    new ContextMenu(c as unknown as Canvas).showCanvas(0, 0, { x: 3, y: 4 });
    press(row("Add to-do note here"));
    expect(c.addNote).toHaveBeenLastCalledWith("note", { x: 3, y: 4 }, expect.objectContaining({ id: "todo" }));
  });

  it("replaces an open menu instead of stacking a second one", () => {
    const menu = new ContextMenu(fakeCanvas() as unknown as Canvas);
    menu.showCanvas(0, 0, { x: 0, y: 0 });
    menu.showCanvas(5, 5, { x: 0, y: 0 });
    expect(document.querySelectorAll("#canvas-ctx-menu")).toHaveLength(1);
  });
});

describe("menu keyboard", () => {
  it("moves with the arrow keys, activates with Enter and closes on Escape, returning focus to the canvas", () => {
    const c = fakeCanvas();
    const menu = new ContextMenu(c as unknown as Canvas);
    menu.showCanvas(0, 0, { x: 5, y: 7 }, true);

    expect((document.activeElement as HTMLElement).textContent).toBe("Add node here…");
    keydown("ArrowDown");
    expect((document.activeElement as HTMLElement).textContent).toBe("Add sticky note here");
    keydown("Enter");
    expect(c.addNote).toHaveBeenCalledWith("note", { x: 5, y: 7 });

    menu.showCanvas(0, 0, { x: 0, y: 0 }, true);
    keydown("Escape");
    expect(document.getElementById("canvas-ctx-menu")).toBeNull();
    expect(document.activeElement).toBe(c.el);
  });

  it("wraps from the first row to the last with ArrowUp", () => {
    const menu = new ContextMenu(fakeCanvas() as unknown as Canvas);
    menu.showCanvas(0, 0, { x: 0, y: 0 }, true);
    keydown("ArrowUp");
    expect((document.activeElement as HTMLElement).textContent).toBe("Hide notes");
  });
});

describe("wire menu", () => {
  it("deletes only when Delete connection is chosen and can start an insert", () => {
    const c = fakeCanvas();
    const conn = {} as Connector;
    const menu = new ContextMenu(c as unknown as Canvas);

    menu.showWire(0, 0, conn, { x: 1, y: 1 });
    expect(labels()).toEqual(["Insert node here…", "Delete connection"]);
    expect(c.deleteConnector).not.toHaveBeenCalled();
    press(document.querySelectorAll("#canvas-ctx-menu [role=menuitem]")[1]);
    expect(c.deleteConnector).toHaveBeenCalledWith(conn);

    menu.showWire(0, 0, conn, { x: 1, y: 1 });
    press(document.querySelector("#canvas-ctx-menu [role=menuitem]")!);
    expect(c.onInsertOnWireRequest).toHaveBeenCalledWith(conn, { sx: 12, sy: 22 });
  });
});

describe("selection menu", () => {
  it("gives a node Disable and Run from here, and a note neither", () => {
    const node = fakeNode({ isNote: false });
    new ContextMenu(fakeCanvas({ nodes: new Map([["n1", node]]) }) as unknown as Canvas).show(0, 0, node);
    expect(labels()).toEqual(expect.arrayContaining(["Rename", "Disable", "Run from here", "Group in frame"]));

    const note = fakeNode({ isNote: true });
    new ContextMenu(fakeCanvas({ nodes: new Map([["n1", note]]) }) as unknown as Canvas).show(0, 0, note);
    expect(labels().join("|")).not.toMatch(/Disable|Run from here|Group in frame/);
  });

  it("sets a note's colour from the swatch row", () => {
    const note = fakeNode({ isNote: true });
    const c = fakeCanvas({ nodes: new Map([["n1", note]]) });
    new ContextMenu(c as unknown as Canvas).show(0, 0, note);

    const blue = document.querySelector<HTMLElement>('#canvas-ctx-menu [role=menuitemradio][title="Blue"]')!;
    press(blue);
    expect(note.data.config["color"]).toBe("blue");
  });

  it("sets a note's text size and a frame's title size from the size row", () => {
    const note = fakeNode({ isNote: true });
    new ContextMenu(fakeCanvas({ nodes: new Map([["n1", note]]) }) as unknown as Canvas).show(0, 0, note);
    expect(document.querySelector('#canvas-ctx-menu [role=group][aria-label="Text size"]')).not.toBeNull();
    press(document.querySelector<HTMLElement>('#canvas-ctx-menu [role=menuitemradio][title="Extra large"]')!);
    expect(note.data.config["size"]).toBe("xl");

    const frame = fakeNode({ isNote: true, style: "frame" });
    new ContextMenu(fakeCanvas({ nodes: new Map([["n2", frame]]) }) as unknown as Canvas).show(0, 0, frame);
    expect(document.querySelector('#canvas-ctx-menu [role=group][aria-label="Title size"]')).not.toBeNull();
  });

  it("offers Fit to contents on a frame only when something is inside it", () => {
    const frame = fakeNode({ isNote: true, style: "frame" });
    const empty = fakeCanvas({ nodes: new Map([["n1", frame]]) });
    new ContextMenu(empty as unknown as Canvas).show(0, 0, frame);
    expect(labels()).toContain("Rename");
    expect(labels()).not.toContain("Fit to contents");

    const full = fakeCanvas({ nodes: new Map([["n1", frame]]), containedIds: vi.fn(() => ["a"]) });
    new ContextMenu(full as unknown as Canvas).show(0, 0, frame);
    expect(labels()).toContain("Fit to contents");
  });
});
