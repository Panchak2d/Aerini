// @vitest-environment jsdom

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  closeExpressionPicker,
  showExpressionPicker,
} from "../expression-picker";
import { setRunOutputs } from "../data-sources";

// expression-picker.ts has zero external imports — no mocking required.

function makeCanvas(
  predecessors: Array<{ from: string; to: string; fromId: string; fromName: string; schema?: Record<string, unknown> }>
): HTMLCanvasElement {
  const el = document.createElement("canvas");
  const connectors = new Map(
    predecessors.map((p, i) => [
      `e${i}`,
      { data: { from_node: p.from, to_node: p.to } },
    ])
  );
  const nodes = new Map(
    predecessors.map((p) => [
      p.from,
      { data: { id: p.fromId, name: p.fromName, output_schema: p.schema ?? {} } },
    ])
  );
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (el as any).__canvas = { connectors, nodes };
  return el;
}

let anchorEl: HTMLButtonElement;
let targetInput: HTMLInputElement;

beforeEach(() => {
  anchorEl = document.createElement("button");
  targetInput = document.createElement("input");
  document.body.appendChild(anchorEl);
  document.body.appendChild(targetInput);
});

afterEach(() => {
  closeExpressionPicker();
  setRunOutputs(null);
  document.body.innerHTML = "";
});

describe("closeExpressionPicker", () => {
  it("is a no-op when no picker is open", () => {
    expect(() => closeExpressionPicker()).not.toThrow();
  });

  it("removes the picker from DOM when one is open", () => {
    const canvasEl = makeCanvas([
      { from: "node_1", to: "current", fromId: "node_1", fromName: "HTTP" },
    ]);
    document.body.appendChild(canvasEl);

    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);
    expect(document.querySelector(".expr-picker")).not.toBeNull();

    closeExpressionPicker();
    expect(document.querySelector(".expr-picker")).toBeNull();
  });
});

describe("showExpressionPicker — missing canvas", () => {
  it("returns early without creating a picker when __canvas is absent", () => {
    const canvasEl = document.createElement("canvas"); // no __canvas property
    document.body.appendChild(canvasEl);

    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);

    expect(document.querySelector(".expr-picker")).toBeNull();
  });
});

describe("showExpressionPicker — no predecessors", () => {
  it("creates a picker with the 'no predecessor' hint", () => {
    const canvasEl = makeCanvas([]); // empty connectors and nodes
    document.body.appendChild(canvasEl);

    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);

    const picker = document.querySelector(".expr-picker");
    expect(picker).not.toBeNull();
    expect(picker!.textContent).toContain("Connect a node to this one first");
  });

  it("picker always includes the Run variables section", () => {
    const canvasEl = makeCanvas([]);
    document.body.appendChild(canvasEl);

    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);

    const picker = document.querySelector(".expr-picker")!;
    expect(picker.textContent).toContain("Run variables");
    expect(picker.textContent).toContain("$run.id");
    expect(picker.textContent).toContain("$run.timestamp");
    expect(picker.textContent).toContain("$run.workflow_name");
  });
});

describe("showExpressionPicker — with predecessor node", () => {
  it("renders a section header for the predecessor node", () => {
    const canvasEl = makeCanvas([
      { from: "node_1", to: "current", fromId: "node_1", fromName: "HTTP Request" },
    ]);
    document.body.appendChild(canvasEl);

    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);

    expect(document.querySelector(".expr-picker")!.textContent).toContain("HTTP Request");
  });

  it("renders a clickable item for a schema property", () => {
    const schema = {
      properties: {
        status: { type: "number", description: "HTTP status code" },
      },
    };
    const canvasEl = makeCanvas([
      { from: "node_1", to: "current", fromId: "node_1", fromName: "Fetch", schema },
    ]);
    document.body.appendChild(canvasEl);

    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);

    const picker = document.querySelector(".expr-picker")!;
    expect(picker.querySelector('[data-expr="{{Fetch.output.status}}"]')).not.toBeNull();
    expect(picker.textContent).toContain("HTTP status code");
  });

  it("replaces a stale picker when called twice — only one picker in DOM", () => {
    const canvasEl = makeCanvas([]);
    document.body.appendChild(canvasEl);

    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);
    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);

    expect(document.querySelectorAll(".expr-picker").length).toBe(1);
  });
});

const clickItem = (expr: string) => {
  const el = document.querySelector<HTMLElement>(`[data-expr="${expr}"]`)!;
  el.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
};

describe("showExpressionPicker — tabs", () => {
  it("functions_are_on_their_own_tab_not_in_the_data_list", () => {
    const canvasEl = makeCanvas([]);
    document.body.appendChild(canvasEl);
    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);
    const picker = document.querySelector(".expr-picker")!;
    expect(picker.textContent).not.toContain("upper(string)");
    picker.querySelectorAll<HTMLElement>(".expr-picker-tab")[1].click();
    expect(picker.textContent).toContain("upper(string)");
  });
});

describe("showExpressionPicker — data tab", () => {
  const schema = {
    properties: {
      digest: { type: "string", "x-aerini-primary-output": true },
      count: { type: "number", description: "Items found" },
    },
  };

  it("main_result_is_pinned_with_its_sample_and_other_fields_sit_behind_show_all", () => {
    setRunOutputs({ node_1: { digest: "Today's news", count: 3 } });
    const canvasEl = makeCanvas([{ from: "node_1", to: "current", fromId: "node_1", fromName: "Feed", schema }]);
    document.body.appendChild(canvasEl);
    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);
    const picker = document.querySelector(".expr-picker")!;
    expect(picker.textContent).toContain("Main result");
    expect(picker.textContent).toContain("Today's news");
    expect(picker.querySelector('[data-expr="{{Feed.output.count}}"]')).toBeNull();
    picker.querySelector<HTMLElement>(".expr-picker-more")!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    expect(picker.querySelector('[data-expr="{{Feed.output.count}}"]')).not.toBeNull();
  });

  it("clicking_main_result_inserts_the_reference_at_the_caret_and_closes", () => {
    const canvasEl = makeCanvas([{ from: "node_1", to: "current", fromId: "node_1", fromName: "Feed", schema }]);
    document.body.appendChild(canvasEl);
    targetInput.value = "Hi !";
    targetInput.setSelectionRange(3, 3);
    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);
    clickItem("{{Feed.output.digest}}");
    expect(targetInput.value).toBe("Hi {{Feed.output.digest}}!");
    expect(document.querySelector(".expr-picker")).toBeNull();
  });

  it("nested_paths_come_from_the_sample_when_the_schema_is_untyped", () => {
    setRunOutputs({ node_1: { latest: { title: "Hello" } } });
    const canvasEl = makeCanvas([{ from: "node_1", to: "current", fromId: "node_1", fromName: "Fetch" }]);
    document.body.appendChild(canvasEl);
    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);
    expect(document.querySelector('[data-expr="{{Fetch.output.latest.title}}"]')).not.toBeNull();
  });

  it("only_the_nearest_upstream_node_is_expanded_at_first", () => {
    const canvasEl = makeCanvas([
      { from: "near", to: "current", fromId: "near", fromName: "Near", schema: { properties: { a: { type: "string" }, b: { type: "string" } } } },
      { from: "far", to: "near", fromId: "far", fromName: "Far", schema: { properties: { z: { type: "string" }, y: { type: "string" } } } },
    ]);
    document.body.appendChild(canvasEl);
    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);
    expect(document.querySelector('[data-expr="{{Near.output.a}}"]')).not.toBeNull();
    expect(document.querySelector('[data-expr="{{Far.output.z}}"]')).toBeNull();
  });

  it("not_yet_run_node_tells_the_user_how_to_get_values", () => {
    const canvasEl = makeCanvas([{ from: "node_1", to: "current", fromId: "node_1", fromName: "Feed", schema }]);
    document.body.appendChild(canvasEl);
    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);
    expect(document.querySelector(".expr-picker")!.textContent).toContain("Run once to see values");
  });

  it("search_filters_by_field_name", () => {
    const canvasEl = makeCanvas([{ from: "node_1", to: "current", fromId: "node_1", fromName: "Feed", schema }]);
    document.body.appendChild(canvasEl);
    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);
    const search = document.querySelector<HTMLInputElement>(".expr-picker-search")!;
    search.value = "count";
    search.dispatchEvent(new Event("input"));
    expect(document.querySelector('[data-expr="{{Feed.output.count}}"]')).not.toBeNull();
    expect(document.querySelector('[data-expr="{{Feed.output.digest}}"]')).toBeNull();
  });
});

describe("showExpressionPicker — auto mode", () => {
  it("typing_in_the_field_closes_an_auto_opened_picker", () => {
    const canvasEl = makeCanvas([{ from: "node_1", to: "current", fromId: "node_1", fromName: "Feed" }]);
    document.body.appendChild(canvasEl);
    showExpressionPicker(anchorEl, targetInput, "current", canvasEl, { auto: true });
    expect(document.querySelector(".expr-picker")).not.toBeNull();
    targetInput.dispatchEvent(new Event("input", { bubbles: true }));
    expect(document.querySelector(".expr-picker")).toBeNull();
  });

  it("escape_closes_and_returns_focus_to_the_field", () => {
    const canvasEl = makeCanvas([]);
    document.body.appendChild(canvasEl);
    showExpressionPicker(anchorEl, targetInput, "current", canvasEl);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(document.querySelector(".expr-picker")).toBeNull();
    expect(document.activeElement).toBe(targetInput);
  });
});
