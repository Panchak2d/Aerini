/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { CanvasNode } from "../canvas/Node";
import { Connector } from "../canvas/Connector";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

const gradient = { addColorStop() {} };
const noopCtx = new Proxy({}, {
  get: (_t, k) => {
    if (k === "measureText") return () => ({ width: 10 });
    if (typeof k === "string" && k.startsWith("create")) return () => gradient;
    return () => undefined;
  },
  set: () => true,
});

let frames: FrameRequestCallback[] = [];
let now = 0;
let resizeCallbacks: Array<() => void> = [];
let size = { w: 800, h: 600 };

function node(id: string, x: number, y = 0): CanvasNode {
  return new CanvasNode({
    id, node_type_id: "webhook", node_type: "action", name: id, config: {}, credentials: {},
    position: { x, y },
    ports: { inputs: [{ id: "input", label: "in", position: "left" as const }], outputs: [{ id: "output", label: "out", position: "right" as const }] },
    input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: false,
  });
}

function wire(id: string, from: string, to: string): Connector {
  return new Connector({ id, from_node: from, from_port: "output", to_node: to, to_port: "input", condition: null, on_success: null, on_failure: null });
}

/** Runs queued animation frames `n` times and returns how many paints they caused. */
function tick(canvas: Canvas, n = 1): number {
  const before = canvas.renderCount;
  for (let i = 0; i < n; i++) {
    const queued = frames; frames = [];
    now += 16;
    for (const f of queued) f(now);
  }
  return canvas.renderCount - before;
}

function build(): Canvas {
  document.body.innerHTML = `<canvas id="canvas"></canvas>`;
  const el = document.getElementById("canvas") as HTMLCanvasElement;
  el.getContext = (() => noopCtx) as unknown as HTMLCanvasElement["getContext"];
  el.getBoundingClientRect = () => ({ left: 0, top: 0, right: size.w, bottom: size.h, width: size.w, height: size.h, x: 0, y: 0, toJSON() {} }) as DOMRect;
  const canvas = new Canvas(el);
  canvas.nodes.set("a", node("a", 0));
  canvas.nodes.set("b", node("b", 400));
  canvas.connectors.set("w", wire("w", "a", "b"));
  tick(canvas, 3);
  return canvas;
}

beforeEach(() => {
  frames = []; now = 0; size = { w: 800, h: 600 }; resizeCallbacks = [];
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => { frames.push(cb); return frames.length; });
  vi.stubGlobal("ResizeObserver", class { constructor(cb: () => void) { resizeCallbacks.push(cb); } observe() {} disconnect() {} });
});
afterEach(() => { vi.unstubAllGlobals(); });

describe("idle canvas", () => {
  it("paints nothing over two seconds when nothing is animating", () => {
    const canvas = build();
    expect(tick(canvas, 120)).toBe(0);
  });

  it("keeps painting every frame while a node is running and stops after it finishes", () => {
    const canvas = build();
    canvas.setNodeStatus("a", "running");
    expect(tick(canvas, 10)).toBe(10);
    canvas.setNodeStatus("a", "success");
    expect(tick(canvas, 1)).toBe(1);
    expect(tick(canvas, 30)).toBe(0);
  });

  it("keeps painting while a wire is flowing", () => {
    const canvas = build();
    canvas.connectors.get("w")!.active = true;
    expect(tick(canvas, 5)).toBe(5);
    canvas.clearConnectorActive();
    tick(canvas, 1);
    expect(tick(canvas, 20)).toBe(0);
  });
});

describe("a change repaints within one frame", () => {
  const cases: Array<[string, (c: Canvas) => void]> = [
    ["selecting a node", (c) => c.selectNode(c.nodes.get("a")!)],
    ["clearing the selection", (c) => { c.selectNode(c.nodes.get("a")!); tick(c, 1); c.clearSelection(); }],
    ["hovering a node", (c) => { c.nodes.get("a")!.hovered = true; }],
    ["hovering a wire", (c) => c.setHoveredConn(c.connectors.get("w")!)],
    ["setting a node status", (c) => c.setNodeStatus("a", "error")],
    ["setting a node output preview", (c) => c.setNodeOutput("a", "done")],
    ["resetting all statuses", (c) => { c.setNodeStatus("a", "success"); tick(c, 1); c.resetAllStatus(); }],
    ["disabling a node", (c) => { c.nodes.get("a")!.disabled = true; }],
    ["moving a node in place", (c) => { c.nodes.get("a")!.data.position.x += 50; }],
    ["renaming a node in place", (c) => { c.nodes.get("a")!.data.name = "a much longer name"; }],
    ["adding a node", (c) => { c.nodes.set("c", node("c", 800)); }],
    ["deleting a node", (c) => { c.nodes.delete("b"); }],
    ["adding a wire", (c) => { c.connectors.set("w2", wire("w2", "b", "a")); }],
    ["replacing the node map (workflow switch)", (c) => { c.nodes = new Map([["z", node("z", 10)]]); }],
    ["replacing the wire map (workflow switch)", (c) => { c.connectors = new Map(); }],
    ["panning", (c) => { c.panX += 10; }],
    ["zooming", (c) => { c.zoom = 2; }],
    ["fitting to screen", (c) => c.fitToScreen()],
    ["centring on a point", (c) => c.centerOn(500, 500)],
    ["undoing", (c) => { c.undo(); }],
    ["a keyboard shortcut", () => { window.dispatchEvent(new KeyboardEvent("keydown", { key: "x" })); }],
    ["a pointer press anywhere", () => { document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true })); }],
    ["a theme switch", async () => { document.documentElement.setAttribute("data-theme", "paper"); await Promise.resolve(); }],
  ];

  for (const [name, mutate] of cases) {
    it(`after ${name}`, async () => {
      const canvas = build();
      await mutate(canvas);
      await Promise.resolve();
      expect(tick(canvas, 1)).toBeGreaterThanOrEqual(1);
      expect(tick(canvas, 20)).toBe(0);
      document.documentElement.removeAttribute("data-theme");
    });
  }

  it("keeps painting while an insert is pending and repaints once when it is cancelled", () => {
    const canvas = build();
    canvas.pendingInsert = { id: "x" } as never;
    expect(tick(canvas, 4)).toBe(4);
    canvas.pendingInsert = null;
    expect(tick(canvas, 1)).toBe(1);
    expect(tick(canvas, 20)).toBe(0);
  });

  it("after a drag ends, one final frame clears the snap guides", () => {
    const canvas = build();
    const ih = canvas.input;
    const at = (x: number, y: number) => ({ clientX: x, clientY: y, button: 0, altKey: false, shiftKey: false, preventDefault() {} }) as unknown as MouseEvent;
    const n = canvas.nodes.get("a")!;
    canvas.panX = 0; canvas.panY = 0; canvas.zoom = 1; tick(canvas, 1);
    ih.onDown(at(n.data.position.x + 20, n.data.position.y + 10));
    ih.onMove(at(n.data.position.x + 60, n.data.position.y + 40));
    expect(tick(canvas, 3)).toBe(3);
    canvas.snap.snapGuides = [{ x1: 0, y1: 0, x2: 0, y2: 10 }];
    ih.onUp(at(n.data.position.x + 60, n.data.position.y + 40));
    expect(tick(canvas, 1)).toBe(1);
    expect(canvas.snap.snapGuides).toEqual([]);
    expect(tick(canvas, 20)).toBe(0);
  });
});

describe("resize", () => {
  it("does not touch the bitmap when the size is unchanged", () => {
    const canvas = build();
    let writes = 0;
    let w = canvas.el.width, h = canvas.el.height;
    Object.defineProperty(canvas.el, "width",  { get: () => w, set: (v: number) => { writes++; w = v; }, configurable: true });
    Object.defineProperty(canvas.el, "height", { get: () => h, set: (v: number) => { writes++; h = v; }, configurable: true });
    canvas.resize();
    expect(writes).toBe(0);
    expect(tick(canvas, 5)).toBe(0);
  });

  it("repaints synchronously when the size changes, before the next frame", () => {
    const canvas = build();
    const before = canvas.renderCount;
    size = { w: 640, h: 480 };
    resizeCallbacks.forEach((cb) => cb());
    expect(canvas.el.width).toBe(640);
    expect(canvas.renderCount).toBe(before + 1);
    expect(tick(canvas, 10)).toBe(0);
  });
});

describe("per-frame work", () => {
  it("skips nodes that lie fully outside the view and draws the ones inside", () => {
    const canvas = build();
    canvas.nodes.set("far", node("far", 50000, 50000));
    const far = canvas.nodes.get("far")!, near = canvas.nodes.get("a")!;
    const farDraw = vi.spyOn(far, "draw"), nearDraw = vi.spyOn(near, "draw");
    tick(canvas, 1);
    expect(farDraw).not.toHaveBeenCalled();
    expect(nearDraw).toHaveBeenCalled();
  });

  it("draws a node again once panning brings it into view", () => {
    const canvas = build();
    canvas.nodes.set("far", node("far", 5000, 0));
    const far = canvas.nodes.get("far")!;
    const farDraw = vi.spyOn(far, "draw");
    tick(canvas, 1);
    expect(farDraw).not.toHaveBeenCalled();
    canvas.panX = -4800;
    tick(canvas, 1);
    expect(farDraw).toHaveBeenCalled();
  });

  it("builds the insert ghost once per pending node type, not once per frame", () => {
    const canvas = build();
    const desc = {
      type_id: "webhook", node_type: "action", display_name: "Ghost",
      ports: { inputs: [], outputs: [{ id: "output", label: "out", position: "right" as const }] },
      input_schema: {}, output_schema: {},
    } as never;
    canvas.pendingInsert = desc;
    canvas.insertGhost = { x: 100, y: 100 };
    tick(canvas, 1);
    const ghost = (canvas as unknown as { ghostNode: CanvasNode }).ghostNode;
    expect(ghost).toBeTruthy();
    canvas.insertGhost = { x: 200, y: 150 };
    tick(canvas, 3);
    expect((canvas as unknown as { ghostNode: CanvasNode }).ghostNode).toBe(ghost);
    expect(ghost.data.position.x).toBe(200 - 110);
  });
});
