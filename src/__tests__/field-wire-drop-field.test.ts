// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { mountExpressionUi } from "../popover/expression-field";
import { closeExpressionPicker } from "../expression-picker";
import { beginFieldWireDrag, cancelFieldWireDrag, endFieldWireDrag, fieldDropOwnsPress } from "../field-wire-drop";
import type { CanvasNode } from "../canvas/Node";

const withMain = { properties: { digest: { type: "string", "x-aerini-primary-output": true } } };

function setup(sourceAt: { current: string | null }) {
  const canvasEl = document.createElement("canvas");
  const nodes = new Map<string, { data: { id: string; name: string; output_schema?: Record<string, unknown> } }>();
  nodes.set("up", { data: { id: "up", name: "Feed", output_schema: withMain } });
  nodes.set("side", { data: { id: "side", name: "Side", output_schema: withMain } });
  nodes.set("cur", { data: { id: "cur", name: "Notify" } });
  (canvasEl as unknown as { __canvas: unknown }).__canvas = {
    connectors: new Map([["e", { data: { from_node: "up", to_node: "cur" } }]]),
    nodes,
    outputPortNodeAtEvent: () => sourceAt.current,
  };
  const host = document.createElement("div");
  const field = document.createElement("input");
  host.appendChild(field);
  document.body.appendChild(host);
  mountExpressionUi(field, host, { data: { id: "cur" } } as unknown as CanvasNode, canvasEl, false);
  return { host, field };
}
const over = (el: Element) => { (document as unknown as { elementFromPoint: () => Element }).elementFromPoint = () => el; };

afterEach(() => {
  cancelFieldWireDrag();
  closeExpressionPicker();
  document.body.innerHTML = "";
});

describe("wire dropped on a mounted field", () => {
  it("inserts_the_upstream_main_result_at_the_end_of_an_unfocused_field_and_never_for_a_non_upstream_node", () => {
    const src = { current: "up" as string | null };
    const { field } = setup(src);
    field.value = "Hello ";
    expect(fieldDropOwnsPress(new MouseEvent("mousedown"))).toBe(true);
    beginFieldWireDrag("up", "output");
    over(field);
    expect(endFieldWireDrag(new MouseEvent("mouseup"))).toBe(true);
    expect(field.value).toBe("Hello {{Feed.output.digest}}");

    src.current = "side";
    expect(fieldDropOwnsPress(new MouseEvent("mousedown"))).toBe(false);
    beginFieldWireDrag("side", "output");
    over(field);
    expect(endFieldWireDrag(new MouseEvent("mouseup"))).toBe(false);
    expect(field.value).toBe("Hello {{Feed.output.digest}}");
  });

  it("shift_opens_the_field_list_and_leaves_the_text_unchanged", () => {
    const { field } = setup({ current: "up" });
    beginFieldWireDrag("up", "output");
    over(field);
    expect(endFieldWireDrag(new MouseEvent("mouseup", { shiftKey: true }))).toBe(true);
    expect(field.value).toBe("");
    expect(document.querySelector(".expr-picker")).not.toBeNull();
  });
});
