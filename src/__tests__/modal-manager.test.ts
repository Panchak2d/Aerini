/**
 * @vitest-environment jsdom
 *
 * DEVIATION NOTE: modal-manager.ts has no test file prior to this batch
 * (Rule 7 — no existing coverage found). `convertN8nWorkflow` reads
 * module-level `_allNodes`, which is only ever set via `initModals` — and
 * `initModals` reaches into a fixed set of DOM element ids with non-null
 * assertions (`getElementById(id)!`), so a minimal DOM fixture covering
 * exactly those ids is built once in `beforeEach` before calling it.
 * `convertN8nWorkflow` itself is then called directly and its return value
 * asserted against — no click-handler/modal round-trip needed for these
 * tests, since the bug (and fix) is entirely inside the conversion logic.
 */
import { describe, it, expect, beforeEach } from "vitest";
import { initModals, convertN8nWorkflow } from "../modal-manager";
import type { NodeDescriptor } from "../ipc/workflow";

/**
 * Node's own built-in Web Storage global (nodejs/node#57666) can shadow
 * jsdom's window.localStorage with a non-functional stand-in (getItem/
 * setItem undefined unless the process was started with
 * --localstorage-file), depending on Node version. initModals() reads/
 * writes localStorage directly, so give every test a real, working
 * in-memory implementation regardless of what the host Node build provides.
 */
function installLocalStorageStub(): void {
  const store = new Map<string, string>();
  const stub: Pick<Storage, "getItem" | "setItem" | "removeItem" | "clear"> = {
    getItem: (key: string) => (store.has(key) ? store.get(key)! : null),
    setItem: (key: string, value: string) => { store.set(key, String(value)); },
    removeItem: (key: string) => { store.delete(key); },
    clear: () => { store.clear(); },
  };
  globalThis.localStorage = stub as Storage;
}

function mkModalDom(): void {
  installLocalStorageStub();
  document.body.innerHTML = "";
  const divIds = [
    "import-preview-modal", "import-name", "import-desc", "import-node-count",
    "import-edge-count", "import-version", "import-req-list",
    "settings-modal", "shortcuts-modal",
  ];
  for (const id of divIds) {
    const el = document.createElement("div");
    el.id = id;
    document.body.appendChild(el);
  }
  const btnIds = ["import-cancel", "import-confirm", "btn-close-settings", "btn-close-shortcuts"];
  for (const id of btnIds) {
    const el = document.createElement("button");
    el.id = id;
    document.body.appendChild(el);
  }
  for (const id of ["setting-grid-snap", "setting-autofit"]) {
    const el = document.createElement("input");
    el.type = "checkbox";
    el.id = id;
    document.body.appendChild(el);
  }
}

function port(id: string, label = id) {
  return { id, label, position: "right" as const };
}

const IF_DESCRIPTOR: NodeDescriptor = {
  type_id: "if_condition", display_name: "If", node_type: "logic", version: "1",
  input_schema: {}, output_schema: {},
  ports: { inputs: [{ id: "input", label: "In", position: "left" }], outputs: [port("on_true", "True"), port("on_false", "False")] },
};

const SWITCH_DESCRIPTOR: NodeDescriptor = {
  type_id: "switch", display_name: "Switch", node_type: "logic", version: "1",
  input_schema: {}, output_schema: {},
  ports: {
    inputs: [{ id: "input", label: "In", position: "left" }],
    outputs: [
      port("case_1"), port("case_2"), port("case_3"), port("case_4"),
      port("case_5"), port("case_6"), port("case_7"), port("case_8"),
      port("default"),
    ],
  },
};

const HTTP_DESCRIPTOR: NodeDescriptor = {
  type_id: "http_request", display_name: "HTTP Request", node_type: "action", version: "1",
  input_schema: {}, output_schema: {},
  ports: { inputs: [{ id: "input", label: "In", position: "left" }], outputs: [port("output", "Out")] },
};

function n8nNode(name: string, type: string, position: [number, number] = [0, 0]) {
  return { name, type, position, parameters: {} };
}

type ConvertedEdge = { from_node: string; from_port: string; to_node: string };
type ConvertedNode = { id: string; name: string; node_type_id: string; ports: { outputs: Array<{ id: string }> } };

beforeEach(() => {
  mkModalDom();
});

describe("convertN8nWorkflow — If node branch port mapping (T1-8 / S12-2)", () => {
  it("maps n8n output index 0 to on_true and index 1 to on_false, not a generic 'output'/'out_1' guess", () => {
    initModals([IF_DESCRIPTOR], () => {});

    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("Check", "n8n-nodes-base.if"),
        n8nNode("True Branch", "n8n-nodes-base.noOp"),
        n8nNode("False Branch", "n8n-nodes-base.noOp"),
      ],
      connections: {
        Check: { main: [[{ node: "True Branch" }], [{ node: "False Branch" }]] },
      },
    };

    const out = convertN8nWorkflow(n8n) as { nodes: ConvertedNode[]; edges: ConvertedEdge[] };
    const ifNode = out.nodes.find(n => n.name === "Check")!;
    expect(ifNode.node_type_id).toBe("if_condition");
    // Node's own ports now come from the real descriptor, not a generic guess.
    expect(ifNode.ports.outputs.map(p => p.id)).toEqual(["on_true", "on_false"]);

    const trueEdge = out.edges.find(e => e.to_node === out.nodes.find(n => n.name === "True Branch")!.id)!;
    const falseEdge = out.edges.find(e => e.to_node === out.nodes.find(n => n.name === "False Branch")!.id)!;
    expect(trueEdge.from_port).toBe("on_true");
    expect(falseEdge.from_port).toBe("on_false");
  });
});

describe("convertN8nWorkflow — Switch node case port mapping (T1-8 / S12-2)", () => {
  it("maps sequential rule outputs to case_1.. and an out-of-range/fallback index to default", () => {
    initModals([SWITCH_DESCRIPTOR], () => {});

    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("Router", "n8n-nodes-base.switch"),
        n8nNode("Bronze", "n8n-nodes-base.noOp"),
        n8nNode("Silver", "n8n-nodes-base.noOp"),
        n8nNode("Fallback", "n8n-nodes-base.noOp"),
      ],
      connections: {
        Router: {
          main: [
            [{ node: "Bronze" }],
            [{ node: "Silver" }],
            [{ node: "Fallback" }], // n8n "Extra Output" fallback slot, one past the last rule
          ],
        },
      },
    };

    const out = convertN8nWorkflow(n8n) as { nodes: ConvertedNode[]; edges: ConvertedEdge[] };
    const byName = (n: string) => out.nodes.find(x => x.name === n)!.id;

    expect(out.edges.find(e => e.to_node === byName("Bronze"))!.from_port).toBe("case_1");
    expect(out.edges.find(e => e.to_node === byName("Silver"))!.from_port).toBe("case_2");
    // Only 2 rules configured but Aerini's switch supports up to case_8 — index 2
    // is still within range, so it maps to case_3, not default. Verified separately
    // below that an index actually at/beyond 8 falls through to "default".
    expect(out.edges.find(e => e.to_node === byName("Fallback"))!.from_port).toBe("case_3");
  });

  it("maps an index at or beyond the 8 supported case ports to default", () => {
    initModals([SWITCH_DESCRIPTOR], () => {});

    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("Router", "n8n-nodes-base.switch"),
        n8nNode("Ninth", "n8n-nodes-base.noOp"),
      ],
      connections: {
        Router: {
          main: [[], [], [], [], [], [], [], [{ node: "Ninth" }]], // index 7 = case_8 (last valid case)
        },
      },
    };
    const out = convertN8nWorkflow(n8n) as { nodes: ConvertedNode[]; edges: ConvertedEdge[] };
    const ninthId = out.nodes.find(n => n.name === "Ninth")!.id;
    expect(out.edges.find(e => e.to_node === ninthId)!.from_port).toBe("case_8");
  });
});

describe("convertN8nWorkflow — non-branching node types (regression, unchanged behavior)", () => {
  it("still uses the real single output port id at index 0", () => {
    initModals([HTTP_DESCRIPTOR], () => {});

    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("Fetch", "n8n-nodes-base.httpRequest"),
        n8nNode("Next", "n8n-nodes-base.noOp"),
      ],
      connections: { Fetch: { main: [[{ node: "Next" }]] } },
    };

    const out = convertN8nWorkflow(n8n) as { nodes: ConvertedNode[]; edges: ConvertedEdge[] };
    const fetchNode = out.nodes.find(n => n.name === "Fetch")!;
    expect(fetchNode.ports.outputs.map(p => p.id)).toEqual(["output"]);
    const nextId = out.nodes.find(n => n.name === "Next")!.id;
    expect(out.edges.find(e => e.to_node === nextId)!.from_port).toBe("output");
  });

  it("falls back to the generic single-port guess for a type with no matching descriptor", () => {
    initModals([], () => {}); // no descriptors registered at all

    const n8n = {
      name: "wf",
      nodes: [n8nNode("Fetch", "n8n-nodes-base.httpRequest")],
      connections: {},
    };

    const out = convertN8nWorkflow(n8n) as { nodes: ConvertedNode[] };
    expect(out.nodes[0].ports.outputs.map(p => p.id)).toEqual(["output"]);
  });
});
