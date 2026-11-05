import { describe, it, expect } from "vitest";
import { CONNECTABLE_MAX, isDownstreamOf, listConnectable, type ConnectGraph, type ConnectNode } from "../connect-insert";

function n(id: string, over: Partial<ConnectNode["data"]> & { ports?: ConnectNode["ports"]; disabled?: boolean } = {}): ConnectNode {
  const { ports, disabled, ...data } = over;
  return {
    disabled,
    data: { id, name: id, ...data },
    ports: ports ?? [{ id: "input", isInput: true }, { id: "output", isInput: false }],
  };
}

function graph(nodes: ConnectNode[], wires: Array<[string, string]> = []): ConnectGraph {
  return {
    nodes: new Map(nodes.map((x) => [x.data.id, x])),
    connectors: new Map(wires.map(([a, b], i) => [`c${i}`, { data: { from_node: a, to_node: b } }])),
  };
}

const ids = (g: ConnectGraph, target: string, free?: () => boolean) => listConnectable(g, target, free).map((c) => c.id);

describe("connect-and-insert candidates", () => {
  it("offers_only_nodes_that_are_neither_upstream_nor_the_target_itself", () => {
    const g = graph([n("a"), n("b"), n("c"), n("t")], [["a", "t"]]);
    expect(ids(g, "t")).toEqual(["b", "c"]);
  });

  it("skips_nodes_downstream_of_the_target_because_a_wire_would_close_a_loop", () => {
    const g = graph([n("t"), n("d1"), n("d2"), n("free")], [["t", "d1"], ["d1", "d2"]]);
    expect(isDownstreamOf(g, "t", "d2")).toBe(true);
    expect(ids(g, "t")).toEqual(["free"]);
  });

  it("skips_nodes_without_a_plain_output_port_notes_disabled_and_unreferenceable_names", () => {
    const g = graph([
      n("t"),
      n("branch", { ports: [{ id: "input", isInput: true }, { id: "true", isInput: false }] }),
      n("sticky", { node_type_id: "note" }),
      n("off", { disabled: true }),
      n("dotted", { name: "Fetch v1.2" }),
      n("ok", { name: "Fetch" }),
    ]);
    expect(ids(g, "t")).toEqual(["ok"]);
  });

  it("offers_nothing_when_the_target_has_no_input_or_its_input_would_be_displaced", () => {
    const noInput = graph([n("t", { ports: [{ id: "output", isInput: false }] }), n("x")]);
    expect(ids(noInput, "t")).toEqual([]);
    expect(ids(graph([n("t"), n("x")]), "t", () => false)).toEqual([]);
  });

  it("points_at_the_main_result_when_declared_else_at_the_whole_output", () => {
    const schema = { properties: { content: { "x-aerini-primary-output": true }, model: {} } };
    const g = graph([n("t"), n("Summarize", { output_schema: schema }), n("Plain")]);
    expect(listConnectable(g, "t").map((c) => c.expr)).toEqual(["{{Summarize.output.content}}", "{{Plain.output}}"]);
  });

  it("never_offers_more_than_the_cap_on_a_large_canvas", () => {
    const many = Array.from({ length: CONNECTABLE_MAX + 25 }, (_, k) => n(`x${k}`));
    expect(ids(graph([...many, n("t")]), "t")).toHaveLength(CONNECTABLE_MAX);
  });
});
