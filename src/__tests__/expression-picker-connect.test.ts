// @vitest-environment jsdom

import { describe, it, expect, vi, afterEach } from "vitest";
import { closeExpressionPicker, showExpressionPicker } from "../expression-picker";

function canvasWith(connectAndInsert: (s: string, t: string, apply: () => void) => boolean, candidates = [{ id: "far", name: "Fetch Data", expr: "{{Fetch Data.output.body}}" }]) {
  const el = document.createElement("canvas");
  const config: Record<string, unknown> = {};
  const nodes = new Map([
    ["cur", { data: { id: "cur", name: "Notify Me", config } }],
    ["far", { data: { id: "far", name: "Fetch Data", node_type_id: "http_request" } }],
  ]);
  (el as unknown as { __canvas: unknown }).__canvas = {
    nodes, connectors: new Map(), connectableTo: () => candidates, connectAndInsert,
  };
  return { el, config };
}

function setup() {
  const anchor = document.createElement("button");
  const input = document.createElement("textarea");
  document.body.append(anchor, input);
  return { anchor, input };
}

afterEach(() => { closeExpressionPicker(); document.body.innerHTML = ""; });

describe("data picker: nodes that are not connected yet", () => {
  it("lists_them_under_not_connected_and_connects_on_choose_writing_the_field_once", () => {
    const { anchor, input } = setup();
    input.value = "Hello ";
    input.setSelectionRange(6, 6);
    const onInput = vi.fn();
    input.addEventListener("input", onInput);
    const connect = vi.fn((_s: string, _t: string, apply: () => void) => { apply(); return true; });
    const { el, config } = canvasWith(connect);

    showExpressionPicker(anchor, input, "cur", el, { fieldKey: "body" });
    const row = document.querySelector<HTMLElement>(".expr-picker-item--unconnected")!;
    expect(document.body.textContent).toContain("Not connected");
    expect(document.body.textContent).toContain("No node feeds this one yet");
    row.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));

    expect(connect).toHaveBeenCalledWith("far", "cur", expect.any(Function));
    expect(config.body).toBe("Hello {{Fetch Data.output.body}}");
    expect(input.value).toBe("Hello {{Fetch Data.output.body}}");
    expect(onInput).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".expr-picker")).toBeNull();
  });

  it("leaves_the_field_untouched_when_the_connection_is_refused", () => {
    const { anchor, input } = setup();
    input.value = "keep";
    const { el, config } = canvasWith(() => false);
    showExpressionPicker(anchor, input, "cur", el, { fieldKey: "body" });
    document.querySelector<HTMLElement>(".expr-picker-item--unconnected")!
      .dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    expect(input.value).toBe("keep");
    expect(config.body).toBeUndefined();
  });

  it("hides_the_group_when_the_caller_gives_no_field_key", () => {
    const { anchor, input } = setup();
    const { el } = canvasWith(() => true);
    showExpressionPicker(anchor, input, "cur", el);
    expect(document.querySelector(".expr-picker-item--unconnected")).toBeNull();
    expect(document.body.textContent).toContain("Connect a node to this one first");
  });
});
