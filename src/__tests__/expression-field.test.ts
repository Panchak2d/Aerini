// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountExpressionUi } from "../popover/expression-field";
import { closeExpressionPicker } from "../expression-picker";
import { setRunOutputs } from "../data-sources";
import type { CanvasNode } from "../canvas/Node";

function setup(schema: Record<string, unknown> | undefined, primary: boolean) {
  const canvasEl = document.createElement("canvas");
  const nodes = new Map<string, { data: { id: string; name: string; output_schema?: Record<string, unknown> } }>();
  if (schema) nodes.set("up", { data: { id: "up", name: "Feed", output_schema: schema } });
  nodes.set("cur", { data: { id: "cur", name: "Notify" } });
  const revealNode = vi.fn();
  (canvasEl as unknown as { __canvas: unknown }).__canvas = {
    connectors: new Map(schema ? [["e", { data: { from_node: "up", to_node: "cur" } }]] : []),
    nodes,
    revealNode,
  };
  const host = document.createElement("div");
  const field = document.createElement("input");
  host.appendChild(field);
  document.body.appendChild(host);
  mountExpressionUi(field, host, { data: { id: "cur" } } as unknown as CanvasNode, canvasEl, primary);
  return { host, field, revealNode };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  closeExpressionPicker();
  setRunOutputs(null);
  document.body.innerHTML = "";
});

const schema = { properties: { digest: { type: "string", "x-aerini-primary-output": true } } };

describe("mountExpressionUi", () => {
  it("renders_a_labelled_insert_data_button", () => {
    const { host } = setup(schema, false);
    const b = host.querySelector<HTMLButtonElement>(".field-insert-data")!;
    expect(b.textContent).toBe("Insert data");
    expect(b.querySelector("svg")).not.toBeNull();
    expect(b.getAttribute("aria-label")).toBeTruthy();
  });

  it("suggestion_chip_shows_only_for_a_blank_primary_field_and_inserts_on_click", () => {
    const { host, field } = setup(schema, true);
    const chip = host.querySelector<HTMLButtonElement>(".field-expr-suggest")!;
    expect(chip.hidden).toBe(false);
    chip.click();
    expect(field.value).toBe("{{Feed.output.digest}}");
    expect(chip.hidden).toBe(true);
  });

  it("no_chip_when_the_field_is_not_the_primary_input", () => {
    const { host } = setup(schema, false);
    expect(host.querySelector<HTMLElement>(".field-expr-suggest")!.hidden).toBe(true);
  });

  it("shows_the_resolved_value_and_expression_styling_for_a_reference", () => {
    setRunOutputs({ up: { digest: "Big news" } });
    const { host, field } = setup(schema, false);
    field.value = "{{Feed.output.digest}}";
    field.dispatchEvent(new Event("input"));
    expect(field.classList.contains("field-expr")).toBe(true);
    expect(host.querySelector(".field-expr-preview")!.textContent).toContain("Big news");
  });

  it("empty_reference_shows_a_warning_and_reveal_selects_the_source_node", () => {
    setRunOutputs({ up: { other: "x" } });
    const { host, field, revealNode } = setup(schema, false);
    field.value = "{{Feed.output.digest}}";
    field.dispatchEvent(new Event("input"));
    const prev = host.querySelector(".field-expr-preview")!;
    expect(prev.classList.contains("field-expr-preview--warn")).toBe(true);
    expect(prev.textContent).toContain("Empty: Feed");
    host.querySelector<HTMLButtonElement>(".field-expr-reveal")!.click();
    expect(revealNode).toHaveBeenCalledTimes(1);
  });

  it("focus_opens_the_picker_only_after_the_programmatic_focus_window", () => {
    const { field } = setup(schema, false);
    field.dispatchEvent(new Event("focus"));
    expect(document.querySelector(".expr-picker")).toBeNull();
    vi.advanceTimersByTime(400);
    field.dispatchEvent(new Event("focus"));
    expect(document.querySelector(".expr-picker")).not.toBeNull();
  });

  it("focus_does_not_open_the_picker_when_the_field_has_text_or_no_upstream_node", () => {
    const a = setup(schema, false);
    a.field.value = "typed";
    vi.advanceTimersByTime(400);
    a.field.dispatchEvent(new Event("focus"));
    expect(document.querySelector(".expr-picker")).toBeNull();
    document.body.innerHTML = "";
    const b = setup(undefined, false);
    vi.advanceTimersByTime(400);
    b.field.dispatchEvent(new Event("focus"));
    expect(document.querySelector(".expr-picker")).toBeNull();
  });
});
