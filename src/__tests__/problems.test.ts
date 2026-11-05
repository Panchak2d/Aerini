/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { Canvas } from "../canvas/Canvas";
import { CanvasNode } from "../canvas/Node";
import { UndoManager } from "../canvas/UndoManager";
import { setRunOutputs } from "../data-sources";
import { collectReferenceProblems, collectRuntimeProblems, setRunLogs } from "../problems";
import { applyProblemFix, markProblemNodes, nextProblemFocus, summarizeProblems } from "../problems-panel";
import { blankRequiredFields, collectProblems, validateWorkflow } from "../validation";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function node(
  id: string,
  name: string,
  config: Record<string, unknown> = {},
  extra: { type?: string; out?: Record<string, unknown>; inp?: Record<string, unknown>; credentials?: Record<string, string> } = {},
): CanvasNode {
  return new CanvasNode({
    id, node_type_id: extra.type ?? "notification", node_type: "action", name,
    config, credentials: extra.credentials ?? {}, position: { x: 0, y: 0 },
    ports: { inputs: [{ id: "input", label: "Input", position: "left" }], outputs: [{ id: "output", label: "Output", position: "right" }] },
    input_schema: extra.inp ?? {}, output_schema: extra.out ?? {}, retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null, dynamic_ports: false,
  });
}

function canvasOf(nodes: CanvasNode[], wires: Array<[string, string]> = []) {
  const canvas = Object.create(Canvas.prototype) as Canvas & Record<string, unknown>;
  canvas.nodes = new Map(nodes.map(n => [n.data.id, n]));
  canvas.connectors = new Map(wires.map(([a, b], i) => [`w${i}`, { data: { from_node: a, to_node: b } }])) as never;
  canvas.selectedNodes = new Set();
  canvas.selectedNode = null;
  canvas.selectedConn = null;
  canvas.onCanvasChanged = vi.fn();
  canvas.onWarn = vi.fn();
  canvas.undoMgr = new UndoManager(canvas);
  return canvas;
}

const SUMMARY = { properties: { content: { type: "string" }, title: { type: "string" } } };

beforeEach(() => { setRunOutputs(null); setRunLogs(null); });

describe("reference problems", () => {
  it("unknown_node_name_offers_the_closest_node_as_a_fix", () => {
    const c = canvasOf([node("a", "Summarize", {}, { out: SUMMARY }), node("b", "Notify", { body: "{{Sumarize.output.content}}" })], [["a", "b"]]);
    const [p] = collectReferenceProblems(c as never);
    expect(p.kind).toBe("unknown-node");
    expect(p.fix?.after).toBe("{{Summarize.output.content}}");
  });

  it("reference_to_a_node_that_is_not_connected_before_this_one_warns_without_a_fix", () => {
    const c = canvasOf([node("a", "Summarize", {}, { out: SUMMARY }), node("b", "Notify", { body: "{{Summarize.output.content}}" })]);
    const [p] = collectReferenceProblems(c as never);
    expect(p.kind).toBe("not-upstream");
    expect(p.fix).toBeUndefined();
  });

  it("missing_field_in_a_declared_schema_suggests_the_closest_field", () => {
    const c = canvasOf([node("a", "Summarize", {}, { out: SUMMARY }), node("b", "Notify", { body: "x {{Summarize.output.contnt}}" })], [["a", "b"]]);
    const [p] = collectReferenceProblems(c as never);
    expect(p.kind).toBe("missing-field");
    expect(p.fix?.after).toBe("{{Summarize.output.content}}");
  });

  it("valid_references_function_calls_and_run_variables_raise_no_problem", () => {
    const c = canvasOf([
      node("a", "Summarize", {}, { out: SUMMARY }),
      node("b", "Notify", { body: "{{Summarize.output.content}} {{upper(Summarize.output.title)}} {{$run.id}} {{now()}}" }),
    ], [["a", "b"]]);
    expect(collectReferenceProblems(c as never)).toEqual([]);
  });

  it("sticky_note_text_that_looks_like_a_reference_is_ignored", () => {
    const c = canvasOf([node("n", "Note", { text: "use {{Ghost.output.x}} here" }, { type: "note" })]);
    expect(collectReferenceProblems(c as never)).toEqual([]);
  });

  it("two_nodes_with_the_same_name_each_get_an_ambiguity_warning", () => {
    const c = canvasOf([node("a", "Same"), node("b", "Same")]);
    expect(collectReferenceProblems(c as never).filter(p => p.kind === "duplicate-name")).toHaveLength(2);
  });
});

describe("runtime problems from the last run log", () => {
  it("an_empty_field_warning_in_the_log_points_at_the_field_and_proposes_a_real_path", () => {
    const c = canvasOf([node("a", "Fetch", {}, { out: { properties: { body: {} } } }), node("b", "Notify", { body: "{{Fetch.output.bdy}}" })], [["a", "b"]]);
    setRunOutputs({ a: { body: "hi" } });
    setRunLogs([{ level: "warn", node_id: "b", message: "Expression '{{Fetch.output.bdy}}': field path 'bdy' not found in node 'Fetch' output." }]);
    const [p] = collectRuntimeProblems(c as never);
    expect(p).toMatchObject({ kind: "empty-field", field: "body", nodeId: "b" });
    expect(p.fix?.after).toBe("{{Fetch.output.body}}");
  });

  it("a_logged_warning_whose_text_was_since_edited_away_is_dropped", () => {
    const c = canvasOf([node("a", "Fetch"), node("b", "Notify", { body: "plain" })], [["a", "b"]]);
    setRunLogs([{ level: "warn", node_id: "b", message: "Expression '{{Fetch.output.bdy}}': field path 'bdy' not found in node 'Fetch' output." }]);
    expect(collectRuntimeProblems(c as never)).toEqual([]);
  });
});

describe("required fields", () => {
  const SLACK = { required: ["channel", "text"], properties: { channel: {}, text: {} } };

  it("required_keys_come_from_the_node_schema_and_blank_ones_block_run", () => {
    const slack = node("s", "Slack", { channel: "#a" }, { type: "slack", inp: SLACK });
    expect(blankRequiredFields(slack.data)).toEqual(["text"]);
    const errors = validateWorkflow(canvasOf([slack, node("t", "Trigger", {}, { type: "manual_trigger" })]));
    expect(errors).toContain('"Slack" — text is required.');
  });

  it("a_saved_credential_satisfies_a_required_key_and_whitespace_counts_as_blank", () => {
    const n = node("s", "Slack", { channel: "   ", text: "hi" }, { type: "slack", inp: SLACK, credentials: { channel: "cred1" } });
    expect(blankRequiredFields(n.data)).toEqual([]);
    n.data.credentials = {};
    expect(blankRequiredFields(n.data)).toEqual(["channel"]);
  });

  it("text_splitter_accepts_source_field_in_place_of_text", () => {
    const inp = { required: ["text"] };
    expect(blankRequiredFields(node("t", "Split", { source_field: "body" }, { type: "text_splitter", inp }).data)).toEqual([]);
    expect(blankRequiredFields(node("t", "Split", {}, { type: "text_splitter", inp }).data)).toEqual(["text"]);
  });

  it("collectProblems_lists_required_errors_before_reference_warnings", () => {
    const slack = node("s", "Slack", { channel: "#a", text: "{{Ghost.output.x}}" }, { type: "slack", inp: { required: ["channel", "text", "x"] } });
    const kinds = collectProblems(canvasOf([slack])).map(p => `${p.severity}:${p.kind}`);
    expect(kinds).toEqual(["error:required", "warning:unknown-node"]);
  });
});

describe("problems panel helpers", () => {
  it("applying_a_fix_rewrites_the_reference_as_one_undo_step_that_undo_restores", () => {
    const c = canvasOf([node("a", "Summarize", {}, { out: SUMMARY }), node("b", "Notify", { body: "x {{Summarize.output.contnt}}" })], [["a", "b"]]);
    const [p] = collectProblems(c);
    expect(applyProblemFix(c, p)).toBe(true);
    expect(c.nodes.get("b")!.data.config.body).toBe("x {{Summarize.output.content}}");
    expect((c.undoMgr as unknown as { stack: unknown[] }).stack).toHaveLength(1);
    c.undo();
    expect(c.nodes.get("b")!.data.config.body).toBe("x {{Summarize.output.contnt}}");
    expect(applyProblemFix(c, { ...p, fix: { label: "x", before: "{{gone}}", after: "y" } })).toBe(false);
  });

  it("only_reference_problems_flag_a_node_and_the_label_counts_all_problems", () => {
    const slack = node("s", "Slack", { channel: "#a", text: "{{Ghost.output.x}}" }, { type: "slack", inp: { required: ["channel", "text"] } });
    const blank = node("t", "Telegram", {}, { type: "telegram", inp: { required: ["chat_id"] } });
    const c = canvasOf([slack, blank]);
    const problems = collectProblems(c);
    markProblemNodes(c, problems);
    expect(slack.hasProblem).toBe(true);
    expect(blank.hasProblem).toBe(false);
    expect(summarizeProblems(problems)).toEqual({ errors: 1, warnings: 1, label: "2 problems" });
    expect(summarizeProblems([]).label).toBe("");
  });
});

describe("problems list keyboard", () => {
  it("arrow_keys_move_between_rows_and_stop_at_both_ends", () => {
    expect(nextProblemFocus(3, 0, "ArrowDown")).toBe(1);
    expect(nextProblemFocus(3, 2, "ArrowDown")).toBe(2);
    expect(nextProblemFocus(3, 0, "ArrowUp")).toBe(0);
    expect(nextProblemFocus(3, 1, "End")).toBe(2);
    expect(nextProblemFocus(3, 1, "a")).toBeNull();
  });
});
