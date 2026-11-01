/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { CanvasNode, NODE_WIDTH } from "../canvas/Node";
import { Connector } from "../canvas/Connector";
import { SnapEngine } from "../canvas/SnapEngine";
import { CAMERA_EASE_MS } from "../canvas/camera";

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

function node(id: string, x: number, y = 0): CanvasNode {
  return new CanvasNode({
    id, node_type_id: "webhook", node_type: "action", name: id, config: {}, credentials: {},
    position: { x, y },
    ports: { inputs: [{ id: "input", label: "in", position: "left" as const }], outputs: [{ id: "output", label: "out", position: "right" as const }] },
    input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: false,
  });
}

const desc = {
  type_id: "webhook", node_type: "action", display_name: "Placed",
  ports: { inputs: [{ id: "input", label: "in", position: "left" as const }], outputs: [{ id: "output", label: "out", position: "right" as const }] },
  input_schema: {}, output_schema: {},
} as never;

function tick(canvas: Canvas, n = 1, step = 16): number {
  const before = canvas.renderCount;
  for (let i = 0; i < n; i++) {
    const queued = frames; frames = [];
    now += step;
    for (const f of queued) f(now);
  }
  return canvas.renderCount - before;
}

function build(extra: Array<[string, number, number?]> = [["a", 0], ["b", 400]]): Canvas {
  document.body.innerHTML = `<canvas id="canvas"></canvas>`;
  const el = document.getElementById("canvas") as HTMLCanvasElement;
  el.getContext = (() => noopCtx) as unknown as HTMLCanvasElement["getContext"];
  el.getBoundingClientRect = () => ({ left: 0, top: 0, right: 800, bottom: 600, width: 800, height: 600, x: 0, y: 0, toJSON() {} }) as DOMRect;
  const canvas = new Canvas(el);
  for (const [id, x, y] of extra) canvas.nodes.set(id, node(id, x, y ?? 0));
  canvas.panX = 0.5; canvas.panY = 0.5; canvas.zoom = 1;
  canvas.panX = 0; canvas.panY = 0;
  tick(canvas, 3);
  return canvas;
}

const at = (x: number, y: number, o: Partial<MouseEvent> = {}) =>
  ({ clientX: x, clientY: y, button: 0, altKey: false, shiftKey: false, ctrlKey: false, preventDefault() {}, ...o }) as unknown as MouseEvent;

beforeEach(() => {
  frames = []; now = 0;
  localStorage.clear();
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => { frames.push(cb); return frames.length; });
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
});
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

describe("node drag", () => {
  it("reports a change once when the drag starts moving and once on drop, never per move", () => {
    const canvas = build();
    const changed = vi.fn(); canvas.onCanvasChanged = changed;
    const ih = canvas.input;
    ih.onDown(at(20, 10));
    for (let i = 1; i <= 60; i++) ih.onMove(at(20 + i, 10 + i));
    expect(changed).toHaveBeenCalledTimes(1);
    ih.onUp(at(80, 70));
    expect(changed).toHaveBeenCalledTimes(2);
  });

  it("a press and release without movement reports no change", () => {
    const canvas = build();
    const changed = vi.fn(); canvas.onCanvasChanged = changed;
    canvas.input.onDown(at(20, 10));
    canvas.input.onUp(at(20, 10));
    expect(changed).not.toHaveBeenCalled();
  });

  it("reads the grid-snap setting once at the start of the drag, not on every move", () => {
    localStorage.setItem("aerini_grid_snap", "true");
    const canvas = build();
    const get = vi.spyOn(Storage.prototype, "getItem");
    const ih = canvas.input;
    ih.onDown(at(20, 10));
    const reads = get.mock.calls.length;
    localStorage.setItem("aerini_grid_snap", "false");
    for (let i = 1; i <= 40; i++) ih.onMove(at(20 + i, 10 + i));
    expect(get.mock.calls.length).toBe(reads);
    const p = canvas.nodes.get("a")!.data.position;
    expect(p.x % 20).toBe(0);
    expect(p.y % 20).toBe(0);
  });
});

describe("hover", () => {
  it("runs hit-testing once per frame however many pointer events arrive", () => {
    const canvas = build();
    const spy = vi.spyOn(canvas, "deleteHandleAt");
    for (let i = 0; i < 50; i++) canvas.input.onMove(at(300 + i, 300));
    expect(spy).not.toHaveBeenCalled();
    tick(canvas, 1);
    const afterOneFrame = spy.mock.calls.length;
    expect(afterOneFrame).toBeGreaterThan(0);
    tick(canvas, 5);
    expect(spy.mock.calls.length).toBe(afterOneFrame);
  });

  it("hovers the node under the last pointer position", () => {
    const canvas = build();
    canvas.input.onMove(at(10, 5));
    canvas.input.onMove(at(30, 15));
    tick(canvas, 1);
    expect(canvas.nodes.get("a")!.hovered).toBe(true);
    canvas.input.onMove(at(700, 500));
    tick(canvas, 1);
    expect(canvas.nodes.get("a")!.hovered).toBe(false);
  });

  it("reads the canvas rect from layout once per frame", () => {
    const canvas = build();
    const rect = vi.spyOn(canvas.el, "getBoundingClientRect");
    for (let i = 0; i < 30; i++) canvas.input.onMove(at(300 + i, 300));
    tick(canvas, 1);
    expect(rect.mock.calls.length).toBeLessThanOrEqual(2);
  });

  it("a pointer that left the canvas before the frame does not hover anything", () => {
    const canvas = build();
    canvas.input.onMove(at(30, 15));
    canvas.input.onLeave();
    tick(canvas, 1);
    expect(canvas.nodes.get("a")!.hovered).toBe(false);
  });
});

describe("wire hit-test", () => {
  it("finds a point on the curve and rejects points outside its bounds", () => {
    const canvas = build();
    const w = new Connector({ id: "w", from_node: "a", from_port: "output", to_node: "b", to_port: "input", condition: null, on_success: null, on_failure: null });
    const mid = w.deleteButtonAnchor(canvas.nodes)!;
    expect(w.containsPoint(mid.x, mid.y, canvas.nodes, 8)).toBe(true);
    expect(w.containsPoint(mid.x, mid.y + 300, canvas.nodes, 8)).toBe(false);
    expect(w.containsPoint(-500, mid.y, canvas.nodes, 8)).toBe(false);
  });
});

describe("snap guides", () => {
  it("draws one guide per aligned edge, not a duplicate for the same line", () => {
    const canvas = build([["m", 100, 0], ["o", 100, 300]]);
    const snap = new SnapEngine(canvas);
    snap.computeSnap(canvas.nodes.get("m")!);
    const vertical = snap.snapGuides.filter(g => g.x1 === g.x2);
    expect(vertical.filter(g => g.x1 === 100)).toHaveLength(1);
    expect(new Set(snap.snapGuides.map(g => JSON.stringify(g))).size).toBe(snap.snapGuides.length);
  });
});

describe("box select", () => {
  it("selects a node the box only partly covers", () => {
    const canvas = build();
    const ih = canvas.input;
    ih.onDown(at(230, 150, { shiftKey: true }));
    ih.onMove(at(150, 40, { shiftKey: true }));
    expect(canvas.nodes.get("a")!.selected).toBe(true);
    expect(canvas.selectedNodes.has("a")).toBe(true);
    expect(canvas.nodes.get("b")!.selected).toBe(false);
    ih.onMove(at(240, 160, { shiftKey: true }));
    expect(canvas.nodes.get("a")!.selected).toBe(false);
    expect(canvas.selectedNodes.has("a")).toBe(false);
  });

  it("does not select a node the box does not touch", () => {
    const canvas = build();
    canvas.input.onDown(at(300, 200, { shiftKey: true }));
    canvas.input.onMove(at(350, 250, { shiftKey: true }));
    expect(canvas.selectedNodes.size).toBe(0);
  });
});

describe("gesture cleanup", () => {
  it("a node drag goes back to its start when the window loses focus", () => {
    const canvas = build();
    const ih = canvas.input;
    ih.onDown(at(20, 10));
    ih.onMove(at(120, 90));
    expect(canvas.nodes.get("a")!.data.position.x).not.toBe(0);
    window.dispatchEvent(new Event("blur"));
    const p = canvas.nodes.get("a")!.data.position;
    expect([p.x, p.y]).toEqual([0, 0]);
    expect(ih.isGestureActive()).toBe(false);
    expect(ih.draggingNode).toBeNull();
    expect(tick(canvas, 1)).toBeGreaterThanOrEqual(1);
    expect(tick(canvas, 20)).toBe(0);
  });

  it("a multi-node drag restores every node, and a pan, wire pull and box-select end", () => {
    const canvas = build();
    const ih = canvas.input;
    canvas.selectNode(canvas.nodes.get("a")!); canvas.selectNode(canvas.nodes.get("b")!);
    ih.onDown(at(20, 10));
    ih.onMove(at(70, 60));
    ih.cancelGestures();
    expect(canvas.nodes.get("a")!.data.position).toEqual({ x: 0, y: 0 });
    expect(canvas.nodes.get("b")!.data.position).toEqual({ x: 400, y: 0 });

    ih.isPanning = true; ih.isBoxSel = true; ih.isCutting = true; ih.shiftHeld = true;
    ih.cancelGestures();
    expect(ih.isGestureActive()).toBe(false);
    expect(ih.shiftHeld).toBe(false);
  });

  it("ends gestures when the page becomes hidden", () => {
    const canvas = build();
    canvas.input.onDown(at(20, 10));
    canvas.input.onMove(at(120, 90));
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    document.dispatchEvent(new Event("visibilitychange"));
    expect(canvas.input.isGestureActive()).toBe(false);
    expect(canvas.nodes.get("a")!.data.position.x).toBe(0);
  });
});

describe("placement", () => {
  it("keeps a clicked node where it was clicked and does not move the view", () => {
    const canvas = build([["a", 0], ["b", 4000]]);
    const placed = canvas.placeNode(desc, 400, 300);
    expect(placed.data.position).toEqual({ x: 400 - NODE_WIDTH / 2, y: 280 });
    expect([canvas.panX, canvas.panY, canvas.zoom]).toEqual([0, 0, 1]);
  });

  it("a click near the middle of the view is placed there, not beside the rightmost node", () => {
    const canvas = build([["a", 0], ["b", 4000]]);
    const placed = canvas.placeNode(desc, 410, 310);
    expect(placed.data.position.x).toBeCloseTo(410 - NODE_WIDTH / 2, 5);
  });

  it("palette placement leaves the view alone when the new node is already fully visible", () => {
    const canvas = build([["a", 0, 200]]);
    canvas.placeNode(desc);
    expect([canvas.panX, canvas.panY]).toEqual([0, 0]);
  });

  it("palette placement brings the node into view when it lands off screen", () => {
    const canvas = build([["a", 0], ["b", 4000]]);
    const placed = canvas.placeNode(desc);
    const sx = placed.data.position.x * canvas.zoom + canvas.panX;
    expect(sx).toBeGreaterThanOrEqual(0);
    expect(sx + NODE_WIDTH * canvas.zoom).toBeLessThanOrEqual(800);
  });

  it("selects and reveals a node that was pushed aside to avoid overlap", () => {
    const canvas = build([["a", 0], ["b", 400]]);
    const placed = canvas.placeNode(desc, 110, 40);
    expect(placed.data.position.y).toBeGreaterThan(20);
    expect(canvas.selectedNode).toBe(placed);
    expect(placed.selected).toBe(true);
  });
});

describe("scroll wheel", () => {
  const wheel = (dy: number, o: Partial<WheelEventInit> = {}) => new WheelEvent("wheel", { deltaY: dy, clientX: 400, clientY: 300, cancelable: true, ...o });

  it("gives the same zoom for one big scroll as for the same distance in many small events", () => {
    const one = build();
    one.input.onWheel(wheel(-100));
    const many = build();
    for (let i = 0; i < 10; i++) many.input.onWheel(wheel(-10));
    expect(many.zoom).toBeCloseTo(one.zoom, 8);
    expect(one.zoom).toBeCloseTo(1 / 0.9, 8);
  });

  it("keeps the point under the cursor fixed while zooming", () => {
    const canvas = build();
    const before = canvas.s2w(250, 120);
    canvas.input.onWheel(wheel(-100, { clientX: 250, clientY: 120 }));
    const after = canvas.s2w(250, 120);
    expect(after.x).toBeCloseTo(before.x, 8);
    expect(after.y).toBeCloseTo(before.y, 8);
  });

  it("pans instead of zooming on a trackpad-style scroll and on Shift+scroll", () => {
    const canvas = build();
    canvas.input.onWheel(wheel(12.5, { deltaX: 3 }));
    expect(canvas.zoom).toBe(1);
    expect([canvas.panX, canvas.panY]).toEqual([-3, -12.5]);
    canvas.input.onWheel(wheel(40, { shiftKey: true }));
    expect(canvas.panX).toBe(-43);
    expect(canvas.panY).toBe(-12.5);
  });

  it("follows the scroll-wheel setting and does not touch storage per event", () => {
    const canvas = build();
    const get = vi.spyOn(Storage.prototype, "getItem");
    const set = vi.spyOn(Storage.prototype, "setItem");
    canvas.input.setScrollMode("pan");
    for (let i = 0; i < 20; i++) canvas.input.onWheel(wheel(10));
    expect(canvas.zoom).toBe(1);
    expect(canvas.panY).toBe(-200);
    expect(get).not.toHaveBeenCalled();
    expect(set).not.toHaveBeenCalled();
  });

  it("repaints within one frame after a wheel pan", () => {
    const canvas = build();
    canvas.input.onWheel(wheel(30, { deltaX: 2 }));
    expect(tick(canvas, 1)).toBeGreaterThanOrEqual(1);
    expect(tick(canvas, 20)).toBe(0);
  });
});

describe("camera animation", () => {
  const allowMotion = () => { (window as unknown as { matchMedia: unknown }).matchMedia = () => ({ matches: false }); };
  afterEach(() => { delete (window as unknown as { matchMedia?: unknown }).matchMedia; });

  it("fit to screen eases, keeps painting while it runs, and settles on the fitted view", () => {
    allowMotion();
    const canvas = build();
    canvas.mapsReplacedAt = performance.now() - 5000;
    canvas.fitToScreen();
    const startZoom = canvas.zoom;
    expect(canvas.isAnimating()).toBe(true);
    expect(tick(canvas, 3)).toBe(3);
    tick(canvas, Math.ceil(CAMERA_EASE_MS / 16) + 2);
    expect(canvas.isAnimating()).toBe(false);
    expect(canvas.zoom).not.toBe(startZoom);
    tick(canvas, 1);
    expect(tick(canvas, 20)).toBe(0);
  });

  it("any wheel input cancels the animation in place", () => {
    allowMotion();
    const canvas = build();
    canvas.mapsReplacedAt = performance.now() - 5000;
    canvas.centerOn(2000, 2000);
    tick(canvas, 2);
    canvas.input.onWheel(new WheelEvent("wheel", { deltaY: 0, deltaX: 1, clientX: 1, clientY: 1, cancelable: true }));
    expect(canvas.camAnim).toBeNull();
  });

  it("frames a freshly loaded workflow at once instead of sweeping across the old one", () => {
    allowMotion();
    const canvas = build();
    canvas.nodes = new Map([["z", node("z", 3000, 3000)]]);
    canvas.fitToScreen();
    expect(canvas.camAnim).toBeFalsy();
    expect(canvas.panX).not.toBe(0);
  });

  it("notifies the persistence hook when nodes are replaced, before the swap", () => {
    const canvas = build();
    const seen: number[] = [];
    canvas.onNodesReplaced = () => seen.push(canvas.nodes.size);
    canvas.nodes = new Map();
    expect(seen).toEqual([2]);
  });
});
