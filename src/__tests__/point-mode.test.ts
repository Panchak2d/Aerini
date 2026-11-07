// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountExpressionUi } from "../popover/expression-field";
import { closeExpressionPicker } from "../expression-picker";
import { setRunOutputs } from "../data-sources";
import { armPointMode, consumePointClick, deferStatusHint, disarmPointMode, exitPointModeOnEscape, isPointModeArmed, pointModeOwns } from "../point-mode";
import { buildJsonTree } from "../json-tree";
import { referencedNames } from "../field-preview";
import { Canvas } from "../canvas/Canvas";
import type { CanvasNode } from "../canvas/Node";

const schema = { properties: { digest: { type: "string", "x-aerini-primary-output": true } } };
const multi = { properties: { a: { type: "string" }, b: { type: "string" } } };

function setup(upSchema: Record<string, unknown>, hitId: { current: string | null }) {
  const canvasEl = document.createElement("canvas");
  const nodes = new Map<string, { data: { id: string; name: string; output_schema?: Record<string, unknown> } }>();
  nodes.set("up", { data: { id: "up", name: "Feed", output_schema: upSchema } });
  nodes.set("other", { data: { id: "other", name: "Side" } });
  nodes.set("cur", { data: { id: "cur", name: "Notify" } });
  const setRefHighlights = vi.fn();
  (canvasEl as unknown as { __canvas: unknown }).__canvas = {
    connectors: new Map([["e", { data: { from_node: "up", to_node: "cur" } }]]),
    nodes,
    revealNode: vi.fn(),
    nodeIdAtEvent: () => hitId.current,
    setRefHighlights,
  };
  const host = document.createElement("div");
  const field = document.createElement("input");
  host.appendChild(field);
  document.body.appendChild(host);
  mountExpressionUi(field, host, { data: { id: "cur" } } as unknown as CanvasNode, canvasEl, false);
  vi.advanceTimersByTime(400);
  return { host, field, setRefHighlights };
}

const down = () => new MouseEvent("mousedown", { bubbles: true, button: 0 });

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  disarmPointMode();
  closeExpressionPicker();
  setRunOutputs(null);
  document.body.innerHTML = "";
});

describe("point mode", () => {
  it("clicking_an_upstream_node_inserts_its_main_result_at_the_caret_and_keeps_the_field_focused", () => {
    const hit = { current: "up" as string | null };
    const { field } = setup(schema, hit);
    field.value = "Hi  there";
    field.focus();
    field.selectionStart = field.selectionEnd = 3;
    expect(isPointModeArmed()).toBe(true);
    const e = down();
    expect(pointModeOwns(e)).toBe(true);
    expect(consumePointClick(e)).toBe(true);
    expect(field.value).toBe("Hi {{Feed.output.digest}} there");
    expect(field.selectionStart).toBe("Hi {{Feed.output.digest}}".length);
    expect(document.activeElement).toBe(field);
  });

  it("nodes_that_are_not_upstream_stay_inert", () => {
    const hit = { current: "other" as string | null };
    const { field } = setup(schema, hit);
    field.focus();
    expect(pointModeOwns(down())).toBe(false);
    expect(consumePointClick(down())).toBe(false);
    expect(field.value).toBe("");
  });

  it("clicks_pass_through_when_not_armed_and_after_blur", () => {
    const hit = { current: "up" as string | null };
    const { field } = setup(schema, hit);
    expect(consumePointClick(down())).toBe(false);
    field.focus();
    field.blur();
    expect(isPointModeArmed()).toBe(false);
    expect(consumePointClick(down())).toBe(false);
  });

  it("escape_leaves_point_mode_once_and_then_falls_through_to_the_popover", () => {
    const { field } = setup(schema, { current: null });
    field.focus();
    expect(exitPointModeOnEscape()).toBe(true);
    expect(exitPointModeOnEscape()).toBe(false);
  });

  it("clicking_the_same_node_again_opens_the_field_list_expanded_on_it", () => {
    const hit = { current: "up" as string | null };
    const { field } = setup(schema, hit);
    field.focus();
    closeExpressionPicker();
    consumePointClick(down());
    expect(document.querySelector(".expr-picker")).toBeNull();
    expect(field.value).toBe("{{Feed.output.digest}}");
    consumePointClick(down());
    expect(document.querySelector(".expr-picker")).not.toBeNull();
  });

  it("a_node_with_no_single_main_result_opens_the_field_list_on_first_click", () => {
    const hit = { current: "up" as string | null };
    const { field } = setup(multi, hit);
    field.focus();
    closeExpressionPicker();
    consumePointClick(down());
    expect(document.querySelector(".expr-picker")).not.toBeNull();
    expect(field.value).toBe("");
  });
});

describe("reference colors", () => {
  it("referencedNames_lists_each_plain_reference_source_once", () => {
    expect(referencedNames("a {{Feed.output.x}} b {{Feed.output.y}} {{Side.output}}")).toEqual(["Feed", "Side"]);
  });

  it("a_pill_names_each_referenced_node_and_hovering_it_highlights_that_node", () => {
    const { host, field, setRefHighlights } = setup(schema, { current: null });
    field.value = "{{Feed.output.digest}}";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    const pill = host.querySelector<HTMLElement>(".ref-pill")!;
    expect(pill.querySelector(".ref-pill-name")!.textContent).toBe("Feed");
    pill.dispatchEvent(new MouseEvent("mouseenter"));
    const arg = setRefHighlights.mock.calls[setRefHighlights.mock.calls.length - 1][0] as Map<string, number>;
    expect([...arg.keys()]).toEqual(["up"]);
  });
});

describe("wire data bubble", () => {
  function draw(output: unknown) {
    const texts: string[] = [];
    const ctx = new Proxy({ measureText: () => ({ width: 50 }), fillText: (t: string) => texts.push(t) } as Record<string, unknown>, {
      get: (t, k) => (k in t ? t[k as string] : () => undefined),
      set: () => true,
    });
    const canvas = Object.create(Canvas.prototype) as Record<string, unknown>;
    Object.assign(canvas, { ctx, zoom: 1, panX: 0, panY: 0, dpr: 1 });
    canvas.deleteHandleTarget = () => ({ conn: { data: { from_node: "up" } }, x: 100, y: 100 });
    setRunOutputs(output === undefined ? null : { up: output });
    (Canvas.prototype as unknown as { drawWireDataBubble(this: unknown, w: number): void }).drawWireDataBubble.call(canvas, 800);
    return texts;
  }

  it("shows_what_flowed_through_the_wire_after_a_run", () => {
    expect(draw({ digest: "hello" }).join("")).toContain("hello");
  });

  it("shows_nothing_before_any_run", () => {
    expect(draw(undefined)).toEqual([]);
  });
});

describe("data tree", () => {
  const data = { title: "Hello", items: [{ id: 7 }], "a.b": "x" };

  it("clicking_a_value_inserts_its_exact_path_into_the_focused_field", () => {
    const { field } = setup(schema, { current: null });
    field.focus();
    const tree = buildJsonTree(data, { nodeId: "up", nodeName: "Feed" });
    document.body.appendChild(tree);
    const leaves = [...tree.querySelectorAll<HTMLButtonElement>(".json-tree-leaf")];
    leaves.find((b) => b.textContent === "Hello")!.click();
    leaves.find((b) => b.textContent === "7")!.click();
    expect(field.value).toBe("{{Feed.output.title}}{{Feed.output.items[0].id}}");
    expect(leaves.find((b) => b.textContent === "x")!.disabled).toBe(true);
  });

  it("values_of_a_node_that_is_not_upstream_do_nothing", () => {
    const { field } = setup(schema, { current: null });
    field.focus();
    const tree = buildJsonTree(data, { nodeId: "other", nodeName: "Side" });
    document.body.appendChild(tree);
    tree.querySelector<HTMLButtonElement>(".json-tree-leaf")!.click();
    expect(field.value).toBe("");
  });

  it("a_mousedown_on_an_upstream_value_is_not_an_outside_click_for_the_popover", () => {
    setup(schema, { current: null }).field.focus();
    const up = buildJsonTree(data, { nodeId: "up", nodeName: "Feed" });
    const other = buildJsonTree(data, { nodeId: "other", nodeName: "Side" });
    document.body.append(up, other);
    const fire = (el: Element) => { let r = false; el.addEventListener("mousedown", (e) => { r = pointModeOwns(e as MouseEvent); }); el.dispatchEvent(new MouseEvent("mousedown", { bubbles: true })); return r; };
    expect(fire(up.querySelector(".json-tree-leaf")!)).toBe(true);
    expect(fire(other.querySelector(".json-tree-leaf")!)).toBe(false);
  });
});

describe("canvas input", () => {
  it("double_click_on_a_pickable_node_does_not_open_that_nodes_settings_while_armed", async () => {
    const { InputHandler } = await import("../canvas/InputHandler");
    const hit = { current: "up" as string | null };
    const { field } = setup(schema, hit);
    field.focus();
    const openNodeConfig = vi.fn();
    const canvas = { evSX: () => ({ sx: 0, sy: 0 }), s2w: () => ({ x: 0, y: 0 }), nodeAt: () => ({ containsPoint: () => true }), noteAt: () => null, selectNode: vi.fn(), openNodeConfig };
    const ih = Object.create(InputHandler.prototype) as { canvas: unknown; onDbl(e: MouseEvent): void };
    ih.canvas = canvas;
    ih.onDbl(new MouseEvent("dblclick"));
    expect(openNodeConfig).not.toHaveBeenCalled();
    disarmPointMode();
    ih.onDbl(new MouseEvent("dblclick"));
    expect(openNodeConfig).toHaveBeenCalledTimes(1);
  });
});

describe("status hint while armed", () => {
  it("a_new_normal_hint_given_while_armed_is_shown_after_disarm_and_not_before", () => {
    const el = document.createElement("div");
    el.id = "status-hint";
    el.textContent = "old";
    document.body.appendChild(el);
    armPointMode({ eligible: new Set(["a"]), hit: () => null, insert: () => {}, onPick: () => {} });
    expect(deferStatusHint("new")).toBe(true);
    expect(el.textContent).not.toBe("new");
    disarmPointMode();
    expect(el.textContent).toBe("new");
    expect(deferStatusHint("again")).toBe(false);
  });
});
