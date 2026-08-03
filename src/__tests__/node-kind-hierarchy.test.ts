import { describe, it, expect, vi } from "vitest";
import { NODE_IDS } from "../node-ids";
import { CanvasNode, type CanvasNodeData } from "../canvas/Node";

// This whole suite's fallback-path guarantee depends on this. If it ever
// fails, everything below needs re-verifying against a real jsdom/browser
// environment instead — fail loudly, don't let this assumption drift silently.
it("sanity: this suite runs with no `document` (vitest environment: node)", () => {
  expect(typeof document).toBe("undefined");
});

interface Call { args: unknown[]; colorAtCall: unknown; }

function makeMockCtx() {
  const gradient = { addColorStop: vi.fn() };
  const fillRectCalls: Call[] = [];
  const fillTextCalls: Call[] = [];

  const ctx = {
    save: vi.fn(), restore: vi.fn(),
    beginPath: vi.fn(), closePath: vi.fn(), clip: vi.fn(),
    moveTo: vi.fn(), lineTo: vi.fn(), arcTo: vi.fn(), arc: vi.fn(), rect: vi.fn(),
    fill: vi.fn(), stroke: vi.fn(),
    fillRect: vi.fn((...args: unknown[]) => {
      fillRectCalls.push({ args, colorAtCall: ctx.fillStyle });
    }),
    fillText: vi.fn((...args: unknown[]) => {
      fillTextCalls.push({ args, colorAtCall: ctx.fillStyle });
    }),
    measureText: vi.fn(() => ({ width: 30 }) as TextMetrics),
    drawImage: vi.fn(),
    createLinearGradient: vi.fn(() => gradient),
    fillStyle: "", strokeStyle: "", font: "", lineWidth: 1,
    textAlign: "left", textBaseline: "alphabetic", globalAlpha: 1,
  } as unknown as CanvasRenderingContext2D;

  return { ctx, fillRectCalls, fillTextCalls };
}

function makeNodeData(node_type_id: string, node_type: CanvasNodeData["node_type"] = "action"): CanvasNodeData {
  return {
    id: "n1",
    node_type_id,
    node_type,
    name: "Test Node",
    config: {},
    credentials: {},
    position: { x: 0, y: 0 },
    ports: { inputs: [], outputs: [] },
    input_schema: {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  };
}

const CAT_ACTION  = "#4d9eff";
const CAT_AI      = "#a78bfa";
const CAT_LOGIC   = "#34d399";
const CAT_UTILITY = "#f59e0b";
const CAT_TRIGGER = "#8aa9c9";

describe("CanvasNode.draw — category accent (CSS-var wiring, node-env fallback)", () => {
  it.each([
    ["action",  NODE_IDS.HTTP_REQUEST,  "ACTION",  CAT_ACTION],
    ["ai",      NODE_IDS.AI_PROMPT,     "AI",      CAT_AI],
    ["logic",   NODE_IDS.IF_CONDITION,  "LOGIC",   CAT_LOGIC],
    ["utility", NODE_IDS.COLLECT_FILES, "UTILITY", CAT_UTILITY],
  ] as const)("%s category renders its label and stripe in the fallback hex (%s)", (nodeType, typeId, label, hex) => {
    const node = new CanvasNode(makeNodeData(typeId, nodeType as CanvasNodeData["node_type"]));
    const { ctx, fillRectCalls, fillTextCalls } = makeMockCtx();
    node.draw(ctx, 0);

    // .filter, not .find: TYPE_META_BASE.ai's icon glyph ("AI") is textually
    // identical to its label ("AI") — both produce a fillText("AI", ...)
    // call, distinguishable only by color (icon: bare accent, label: accent+"99").
    const matches = fillTextCalls.filter(c => c.args[0] === label);
    expect(matches.length, `expected at least one fillText("${label}", ...) call`).toBeGreaterThan(0);
    expect(matches.some(c => c.colorAtCall === hex + "99")).toBe(true);

    // The stripe is the only fillRect call reached for a non-note node.
    expect(fillRectCalls).toHaveLength(1);
    expect(fillRectCalls[0].colorAtCall).toBe(hex);
  });
});

describe("CanvasNode.draw — trigger identity overlay (independent of NodeType)", () => {
  it("swaps the category label to TRIGGER and the stripe to --cat-trigger, for a trigger node_type_id", () => {
    const node = new CanvasNode(makeNodeData(NODE_IDS.SCHEDULE, "action"));
    const { ctx, fillRectCalls, fillTextCalls } = makeMockCtx();
    node.draw(ctx, 0);

    expect(fillTextCalls.find(c => c.args[0] === "ACTION")).toBeUndefined();
    const labelCall = fillTextCalls.find(c => c.args[0] === "TRIGGER");
    expect(labelCall, 'expected fillText("TRIGGER", ...)').toBeTruthy();
    expect(labelCall!.colorAtCall).toBe(CAT_TRIGGER + "99");

    expect(fillRectCalls).toHaveLength(1);
    expect(fillRectCalls[0].colorAtCall).toBe(CAT_TRIGGER);
  });

  it("does NOT recolor the node icon — icon stays on the category (action) accent, not the trigger accent", () => {
    const node = new CanvasNode(makeNodeData(NODE_IDS.SCHEDULE, "action"));
    const { ctx, fillTextCalls } = makeMockCtx();
    node.draw(ctx, 0);

    // Fallback icon glyph for "action" category is "A" (bitmap cache is empty
    // in this environment, so the font-fallback branch is always taken).
    const iconCall = fillTextCalls.find(c => c.args[0] === "A");
    expect(iconCall, 'expected the icon fallback glyph fillText("A", ...)').toBeTruthy();
    expect(iconCall!.colorAtCall).toBe(CAT_ACTION);
    expect(iconCall!.colorAtCall).not.toBe(CAT_TRIGGER);
  });

  it("node_type stays the real 4-value union (\"action\") for trigger ids — no 5th NodeType value introduced", () => {
    const node = new CanvasNode(makeNodeData(NODE_IDS.WEBHOOK, "action"));
    expect(node.data.node_type).toBe("action");
  });

  it("leaves non-trigger action nodes unaffected (regression: label stays ACTION)", () => {
    const node = new CanvasNode(makeNodeData(NODE_IDS.HTTP_REQUEST, "action"));
    const { ctx, fillTextCalls } = makeMockCtx();
    node.draw(ctx, 0);
    expect(fillTextCalls.find(c => c.args[0] === "TRIGGER")).toBeUndefined();
    expect(fillTextCalls.find(c => c.args[0] === "ACTION")).toBeTruthy();
  });
});

describe("CanvasNode.draw — selection indicator (soft ring -> CAD corner brackets)", () => {
  it("draws four corner brackets (8 lineTo calls) when selected, none when not", () => {
    const node = new CanvasNode(makeNodeData(NODE_IDS.HTTP_REQUEST));

    const { ctx: ctxIdle, fillTextCalls: _t1 } = makeMockCtx();
    node.selected = false;
    node.draw(ctxIdle, 0);
    // roundedRect (body/header/etc.) never calls lineTo — only moveTo+arcTo.
    // The header separator is the only unconditional lineTo call.
    expect(ctxIdle.lineTo).toHaveBeenCalledTimes(1);

    const { ctx: ctxSelected } = makeMockCtx();
    node.selected = true;
    node.draw(ctxSelected, 0);
    // header separator (1) + 4 corners x 2 lineTo each (8) = 9.
    expect(ctxSelected.lineTo).toHaveBeenCalledTimes(9);
  });

  it("hover ring is untouched — still uses the old rounded-rect stroke (no extra lineTo calls)", () => {
    const node = new CanvasNode(makeNodeData(NODE_IDS.HTTP_REQUEST));
    node.selected = false;
    node.hovered  = true;
    const { ctx } = makeMockCtx();
    node.draw(ctx, 0);
    expect(ctx.lineTo).toHaveBeenCalledTimes(1); // just the header separator
  });
});
