// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountExpressionUi } from "../popover/expression-field";
import { closeExpressionPicker } from "../expression-picker";
import { setRunOutputs } from "../data-sources";
import type { CanvasNode } from "../canvas/Node";

const SAMPLE = {
  latest: { title: "Hello world" },
  count: 7,
  a: { b: { c: { d: { e: "needle-value" } } } },
};

function setup(fieldKey: string, withUpstream = true) {
  const canvasEl = document.createElement("canvas");
  const nodes = new Map<string, { data: { id: string; name: string; output_schema?: Record<string, unknown> } }>();
  nodes.set("up", { data: { id: "up", name: "Feed", output_schema: { properties: { count: { type: "number" } } } } });
  nodes.set("cur", { data: { id: "cur", name: "Notify" } });
  const editNode = vi.fn((_n: unknown, mutate: () => void) => mutate());
  (canvasEl as unknown as { __canvas: unknown }).__canvas = {
    connectors: new Map(withUpstream ? [["e", { data: { from_node: "up", to_node: "cur" } }]] : []),
    nodes,
    editNode,
  };
  const host = document.createElement("div");
  const field = document.createElement("textarea");
  host.appendChild(field);
  document.body.appendChild(host);
  const node = { data: { id: "cur", config: {} as Record<string, unknown> } } as unknown as CanvasNode;
  mountExpressionUi(field, host, node, canvasEl, false, fieldKey);
  return { host, field, node, editNode };
}

function type(field: HTMLTextAreaElement, value: string, data: string | null, inputType = "insertText"): void {
  field.value = value;
  field.selectionStart = field.selectionEnd = value.length;
  field.dispatchEvent(new InputEvent("input", { bubbles: true, data, inputType }));
}

const picker = () => document.querySelector<HTMLElement>(".expr-picker");
const exprs = () => [...document.querySelectorAll<HTMLElement>(".expr-picker-item")].map((e) => e.dataset.expr);
const live = (host: HTMLElement) => host.querySelector<HTMLElement>(".field-expr-live")!;

beforeEach(() => {
  vi.useFakeTimers();
  setRunOutputs({ up: SAMPLE });
});
afterEach(() => {
  vi.useRealTimers();
  closeExpressionPicker();
  setRunOutputs(null);
  document.body.innerHTML = "";
});

describe("@ mention", () => {
  it("opens_at_a_word_start_filters_as_you_type_and_replaces_the_at_text_on_enter", () => {
    const { field } = setup("mention-a");
    type(field, "Hi @", "@");
    expect(picker()).not.toBeNull();
    expect(picker()!.querySelector<HTMLElement>(".expr-picker-search")!.hidden).toBe(true);
    type(field, "Hi @hello", "o");
    expect(exprs()).toContain("{{Feed.output.latest.title}}");
    expect(exprs()).not.toContain("{{Feed.output.count}}");
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    expect(field.value).toBe("Hi {{Feed.output.latest.title}}");
    expect(picker()).toBeNull();
  });

  it("does_not_open_for_an_email_a_discord_mention_a_paste_or_a_node_with_no_upstream", () => {
    const a = setup("mention-b");
    type(a.field, "me@", "@");
    type(a.field, "<@", "@");
    type(a.field, "@", null, "insertFromPaste");
    expect(picker()).toBeNull();
    document.body.innerHTML = "";
    const b = setup("mention-c", false);
    type(b.field, "@", "@");
    expect(picker()).toBeNull();
  });

  it("closes_when_the_at_is_deleted_or_nothing_matches", () => {
    const { field } = setup("mention-d");
    type(field, "@", "@");
    expect(picker()).not.toBeNull();
    type(field, "@zzzzqq", "q");
    expect(picker()).toBeNull();
    type(field, "@", "@");
    type(field, "", null, "deleteContentBackward");
    expect(picker()).toBeNull();
  });

  it("finds_deep_values_through_the_value_index", () => {
    const { field } = setup("mention-e");
    type(field, "@", "@");
    type(field, "@needle", "e");
    expect(exprs()).toContain("{{Feed.output.a.b.c.d.e}}");
    expect(document.querySelector(".expr-picker-section")?.textContent).toBe("Matching values");
  });
});

describe("smart paste prompt", () => {
  it("shows_on_paste_and_never_converts_by_itself", () => {
    const { host, field, editNode } = setup("paste-a");
    type(field, "Hello world", null, "insertFromPaste");
    expect(live(host).hidden).toBe(false);
    expect(live(host).textContent).toContain("That's from Feed");
    expect(live(host).textContent).toContain("Make it live?");
    vi.advanceTimersByTime(5000);
    expect(field.value).toBe("Hello world");
    expect(editNode).not.toHaveBeenCalled();
  });

  it("waits_for_typing_to_pause_and_hides_when_the_text_changes", () => {
    const { host, field } = setup("paste-b");
    type(field, "Hello world", "d");
    expect(live(host).hidden).toBe(true);
    vi.advanceTimersByTime(600);
    expect(live(host).hidden).toBe(false);
    type(field, "Hello world!", "!");
    expect(live(host).hidden).toBe(true);
  });

  it("convert_makes_one_undo_step_and_writes_the_reference", () => {
    const { host, field, node, editNode } = setup("paste-c");
    type(field, "Hello world", null, "insertFromPaste");
    live(host).querySelector<HTMLButtonElement>(".field-expr-live-convert")!.click();
    expect(field.value).toBe("{{Feed.output.latest.title}}");
    expect(editNode).toHaveBeenCalledTimes(1);
    expect(node.data.config["paste-c"]).toBe("{{Feed.output.latest.title}}");
    expect(live(host).hidden).toBe(true);
  });

  it("dismiss_is_remembered_for_that_field_and_value_after_the_form_is_rebuilt", () => {
    const first = setup("paste-d");
    type(first.field, "Hello world", null, "insertFromPaste");
    live(first.host).querySelector<HTMLButtonElement>(".field-expr-live-dismiss")!.click();
    expect(live(first.host).hidden).toBe(true);
    document.body.innerHTML = "";
    const again = setup("paste-d");
    type(again.field, "Hello world", null, "insertFromPaste");
    expect(live(again.host).hidden).toBe(true);
    const other = setup("paste-other-field");
    type(other.field, "Hello world", null, "insertFromPaste");
    expect(live(other.host).hidden).toBe(false);
  });
});
