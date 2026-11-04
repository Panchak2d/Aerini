// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountExpressionUi } from "../popover/expression-field";
import { closeExpressionPicker } from "../expression-picker";
import { setRunOutputs } from "../data-sources";
import { listReferences } from "../field-preview";
import type { CanvasNode } from "../canvas/Node";

function setup(value: string) {
  const canvasEl = document.createElement("canvas");
  const nodes = new Map<string, { data: { id: string; name: string; node_type_id?: string } }>();
  nodes.set("up", { data: { id: "up", name: "Feed", node_type_id: "feed_parser" } });
  nodes.set("cur", { data: { id: "cur", name: "Notify" } });
  const revealNode = vi.fn();
  (canvasEl as unknown as { __canvas: unknown }).__canvas = {
    connectors: new Map([["e", { data: { from_node: "up", to_node: "cur" } }]]),
    nodes,
    revealNode,
  };
  const host = document.createElement("div");
  const field = document.createElement("textarea");
  host.appendChild(field);
  document.body.appendChild(host);
  mountExpressionUi(field, host, { data: { id: "cur" } } as unknown as CanvasNode, canvasEl, false);
  vi.advanceTimersByTime(400);
  field.value = value;
  field.dispatchEvent(new Event("input", { bubbles: true }));
  return { host, field, revealNode };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  closeExpressionPicker();
  setRunOutputs(null);
  document.body.innerHTML = "";
});

describe("listReferences", () => {
  it("reports_position_node_and_path_segments_for_plain_references_only", () => {
    const text = "a {{Feed.output.items[0].title}} b {{$run.id}} {{Side.output}}";
    const refs = listReferences(text);
    expect(refs.map((r) => [r.nodeName, r.path])).toEqual([["Feed", ["items[0]", "title"]], ["Side", []]]);
    expect(text.slice(refs[0].start, refs[0].end)).toBe("{{Feed.output.items[0].title}}");
  });
});

describe("function-wrapped references", () => {
  it("listReferences_finds_a_reference_that_is_the_first_argument_of_one_function", () => {
    const refs = listReferences('{{upper(Feed.output.title)}} {{join(Feed.output.tags, ", ")}} {{Feed.output.a}}');
    expect(refs.map((r) => [r.nodeName, r.path, r.wrapped])).toEqual([
      ["Feed", ["title"], true],
      ["Feed", ["tags"], true],
      ["Feed", ["a"], false],
    ]);
  });

  it("a_wrapped_reference_gets_a_chip_without_a_remove_control", () => {
    const { host, field } = setup("{{upper(Feed.output.title)}}");
    const chips = host.querySelectorAll(".ref-pill");
    expect(chips.length).toBe(1);
    expect(chips[0].querySelector(".ref-pill-name")!.textContent).toBe("Feed");
    expect(chips[0].querySelector(".ref-pill-close")).toBeNull();
    expect(field.value).toBe("{{upper(Feed.output.title)}}");
  });
});

describe("preview refresh", () => {
  it("the_preview_updates_when_run_outputs_arrive_while_the_field_stays_open", () => {
    const { host } = setup("{{Feed.output.title}}");
    const text = () => host.querySelector(".field-expr-preview-text")!.textContent;
    expect(text()).toContain("Run the workflow once");
    setRunOutputs({ up: { title: "Hello" } });
    expect(text()).toBe("Hello");
  });
});

describe("reference chips", () => {
  it("each_reference_gets_one_chip_with_node_name_and_path_and_the_stored_text_is_unchanged", () => {
    const { host, field } = setup("Hi {{Feed.output.title}} and {{Feed.output.link}}");
    const chips = host.querySelectorAll(".ref-pill");
    expect(chips.length).toBe(2);
    expect(chips[0].querySelector(".ref-pill-name")!.textContent).toBe("Feed");
    expect(chips[0].querySelector(".ref-pill-seg")!.textContent).toBe("title");
    expect(chips[1].querySelector(".ref-pill-seg")!.textContent).toBe("link");
    expect(field.value).toBe("Hi {{Feed.output.title}} and {{Feed.output.link}}");
  });

  it("the_close_control_removes_only_that_reference_through_one_input_event", () => {
    const { host, field } = setup("A {{Feed.output.title}} B {{Feed.output.link}}");
    const onInput = vi.fn();
    field.addEventListener("input", onInput);
    host.querySelectorAll<HTMLButtonElement>(".ref-pill-close")[0].click();
    expect(field.value).toBe("A  B {{Feed.output.link}}");
    expect(onInput).toHaveBeenCalledTimes(1);
    expect(host.querySelectorAll(".ref-pill").length).toBe(1);
    expect(field.selectionStart).toBe(2);
  });

  it("backspace_on_a_focused_chip_removes_it_and_a_click_reveals_the_node", () => {
    const { host, field, revealNode } = setup("{{Feed.output.title}}");
    const main = host.querySelector<HTMLButtonElement>(".ref-pill-main")!;
    main.click();
    expect(revealNode).toHaveBeenCalledTimes(1);
    closeExpressionPicker();
    main.dispatchEvent(new KeyboardEvent("keydown", { key: "Backspace", bubbles: true, cancelable: true }));
    expect(field.value).toBe("");
    expect(host.querySelector(".ref-pill")).toBeNull();
  });
});

describe("long preview", () => {
  it("long_preview_is_cut_and_show_all_reveals_the_rest_then_show_less_cuts_it_again", () => {
    const long = "x".repeat(500);
    setRunOutputs({ up: { digest: long } });
    const { host } = setup("{{Feed.output.digest}}");
    const text = () => host.querySelector(".field-expr-preview-text")!.textContent!;
    expect(text().length).toBeLessThan(200);
    (host.querySelector(".field-expr-more") as HTMLButtonElement).click();
    expect(text()).toBe(long);
    (host.querySelector(".field-expr-more") as HTMLButtonElement).click();
    expect(text().length).toBeLessThan(200);
  });
});
