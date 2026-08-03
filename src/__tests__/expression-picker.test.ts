// @vitest-environment jsdom

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  closeExpressionPicker,
  showExpressionPicker,
} from "../expression-picker";

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
    expect(picker!.textContent).toContain("No connected predecessor nodes");
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
    expect(picker.textContent).toContain("{{Fetch.output.status}}");
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
