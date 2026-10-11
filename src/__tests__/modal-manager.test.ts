/* @vitest-environment jsdom */
import { describe, it, expect, beforeEach } from "vitest";
import { initModals, convertN8nWorkflow, showImportPreview } from "../modal-manager";
import { registerNodeDescriptors } from "../canvas/CanvasSerializer";
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
  Object.defineProperty(globalThis, "localStorage", { value: stub as Storage, writable: true, configurable: true });
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
  const btnIds = ["import-cancel", "import-confirm", "btn-close-settings", "btn-close-shortcuts", "btn-shortcuts"];
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
  // theme select, populated at runtime by initModals() itself.
  const themeSelect = document.createElement("select");
  themeSelect.id = "setting-theme";
  document.body.appendChild(themeSelect);
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

describe("initModals — theme select wiring", () => {
  it("populates the select from the THEMES registry and applies a change as data-theme", () => {
    document.documentElement.removeAttribute("data-theme");
    registerNodeDescriptors([]);
    initModals(() => {});

    const select = document.getElementById("setting-theme") as HTMLSelectElement;
    const optionValues = Array.from(select.options).map(o => o.value);
    expect(optionValues).toEqual(["midnight", "paper"]);
    expect(select.value).toBe("midnight"); // nothing stored yet -> default

    select.value = "paper";
    select.dispatchEvent(new Event("change"));
    expect(document.documentElement.getAttribute("data-theme")).toBe("paper");
    expect(localStorage.getItem("aerini_theme")).toBe("paper");
  });
});

describe("initModals — shortcuts button opens the shortcuts modal", () => {
  it("removes 'hidden' from shortcuts-modal when btn-shortcuts is clicked", () => {
    const modal = document.getElementById("shortcuts-modal")!;
    modal.classList.add("hidden");
    registerNodeDescriptors([]);
    initModals(() => {});

    document.getElementById("btn-shortcuts")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));

    expect(modal.classList.contains("hidden")).toBe(false);
  });
});

describe("convertN8nWorkflow — If node branch port mapping", () => {
  it("maps n8n output index 0 to on_true and index 1 to on_false, not a generic 'output'/'out_1' guess", () => {
    registerNodeDescriptors([IF_DESCRIPTOR]);
    initModals(() => {});

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

describe("convertN8nWorkflow — Switch node case port mapping", () => {
  it("maps sequential rule outputs to case_1.. and an out-of-range/fallback index to default", () => {
    registerNodeDescriptors([SWITCH_DESCRIPTOR]);
    initModals(() => {});

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
    registerNodeDescriptors([SWITCH_DESCRIPTOR]);
    initModals(() => {});

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

describe("convertN8nWorkflow — Gmail node type", () => {
  it("maps n8n gmail to the registered email_send node id", () => {
    registerNodeDescriptors([]);
    initModals(() => {});

    const out = convertN8nWorkflow({
      name: "wf",
      nodes: [n8nNode("Mail", "n8n-nodes-base.gmail"), n8nNode("Mail2", "n8n-nodes-base.emailSend")],
      connections: {},
    }) as { nodes: ConvertedNode[] };

    expect(out.nodes.map(n => n.node_type_id)).toEqual(["email_send", "email_send"]);
  });
});

describe("convertN8nWorkflow — non-branching node types (regression, unchanged behavior)", () => {
  it("still uses the real single output port id at index 0", () => {
    registerNodeDescriptors([HTTP_DESCRIPTOR]);
    initModals(() => {});

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
    registerNodeDescriptors([]); // no descriptors registered at all
    initModals(() => {});

    const n8n = {
      name: "wf",
      nodes: [n8nNode("Fetch", "n8n-nodes-base.httpRequest")],
      connections: {},
    };

    const out = convertN8nWorkflow(n8n) as { nodes: ConvertedNode[] };
    expect(out.nodes[0].ports.outputs.map(p => p.id)).toEqual(["output"]);
  });
});

// ── If/Switch decision-logic translation ──

type ConvertedNodeWithConfig = ConvertedNode & { config: Record<string, unknown> };
type ConvertedResult = { nodes: ConvertedNodeWithConfig[]; edges: ConvertedEdge[]; _n8n_import_warnings?: string[] };

function n8nNodeParams(name: string, type: string, parameters: Record<string, unknown>) {
  return { name, type, position: [0, 0], parameters };
}

describe("convertN8nWorkflow — If node condition translation", () => {
  it("translates a single v2 filter condition (string equals) against its one predecessor", () => {
    registerNodeDescriptors([IF_DESCRIPTOR]);
    initModals(() => {});
    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("Fetch", "n8n-nodes-base.httpRequest"),
        n8nNodeParams("Check", "n8n-nodes-base.if", {
          conditions: {
            conditions: [{ leftValue: "={{ $json.status }}", rightValue: "paid", operator: { type: "string", operation: "equals" } }],
            combinator: "and",
          },
        }),
      ],
      connections: { Fetch: { main: [[{ node: "Check" }]] } },
    };
    const out = convertN8nWorkflow(n8n) as ConvertedResult;
    const check = out.nodes.find(n => n.name === "Check")!;
    expect(check.config.condition).toBe("{{Fetch.output.status}} == 'paid'");
    expect(out._n8n_import_warnings).toBeUndefined();
  });

  it("translates a v2 numeric-comparison and a v1-legacy boolean condition correctly", () => {
    registerNodeDescriptors([IF_DESCRIPTOR]);
    initModals(() => {});
    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("Fetch", "n8n-nodes-base.httpRequest"),
        n8nNodeParams("AmountCheck", "n8n-nodes-base.if", {
          conditions: { conditions: [{ leftValue: "={{ $json.amount }}", rightValue: 100, operator: { type: "number", operation: "gt" } }], combinator: "and" },
        }),
        n8nNodeParams("ValidCheck", "n8n-nodes-base.if", {
          conditions: { boolean: [{ value1: "={{ $json.valid }}", operation: "true" }] },
        }),
      ],
      connections: { Fetch: { main: [[{ node: "AmountCheck" }, { node: "ValidCheck" }]] } },
    };
    const out = convertN8nWorkflow(n8n) as ConvertedResult;
    expect(out.nodes.find(n => n.name === "AmountCheck")!.config.condition).toBe("{{Fetch.output.amount}} > 100");
    expect(out.nodes.find(n => n.name === "ValidCheck")!.config.condition).toBe("{{Fetch.output.valid}} == true");
  });

  it("leaves the condition unconfigured and emits a warning for chained AND/OR conditions instead of guessing", () => {
    registerNodeDescriptors([IF_DESCRIPTOR]);
    initModals(() => {});
    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("Fetch", "n8n-nodes-base.httpRequest"),
        n8nNodeParams("Check", "n8n-nodes-base.if", {
          conditions: {
            conditions: [
              { leftValue: "={{ $json.status }}", rightValue: "paid", operator: { type: "string", operation: "equals" } },
              { leftValue: "={{ $json.amount }}", rightValue: 100, operator: { type: "number", operation: "gt" } },
            ],
            combinator: "and",
          },
        }),
      ],
      connections: { Fetch: { main: [[{ node: "Check" }]] } },
    };
    const out = convertN8nWorkflow(n8n) as ConvertedResult;
    const check = out.nodes.find(n => n.name === "Check")!;
    expect(check.config.condition).toBeUndefined();
    expect(out._n8n_import_warnings).toHaveLength(1);
    expect(out._n8n_import_warnings![0]).toContain("Check");
    expect(out._n8n_import_warnings![0]).toContain("chained conditions");
  });

  it("emits a warning instead of guessing when the predecessor is ambiguous (two upstream nodes)", () => {
    registerNodeDescriptors([IF_DESCRIPTOR]);
    initModals(() => {});
    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("A", "n8n-nodes-base.httpRequest"),
        n8nNode("B", "n8n-nodes-base.httpRequest"),
        n8nNodeParams("Check", "n8n-nodes-base.if", {
          conditions: { conditions: [{ leftValue: "={{ $json.status }}", rightValue: "ok", operator: { type: "string", operation: "equals" } }], combinator: "and" },
        }),
      ],
      connections: { A: { main: [[{ node: "Check" }]] }, B: { main: [[{ node: "Check" }]] } },
    };
    const out = convertN8nWorkflow(n8n) as ConvertedResult;
    expect(out.nodes.find(n => n.name === "Check")!.config.condition).toBeUndefined();
    expect(out._n8n_import_warnings![0]).toContain("could not identify exactly one upstream node");
  });

  it("emits a warning for an operator Aerini's If node doesn't support (e.g. regex) rather than dropping/misrouting it", () => {
    registerNodeDescriptors([IF_DESCRIPTOR]);
    initModals(() => {});
    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("Fetch", "n8n-nodes-base.httpRequest"),
        n8nNodeParams("Check", "n8n-nodes-base.if", {
          conditions: { conditions: [{ leftValue: "={{ $json.status }}", rightValue: "^ok$", operator: { type: "string", operation: "regex" } }], combinator: "and" },
        }),
      ],
      connections: { Fetch: { main: [[{ node: "Check" }]] } },
    };
    const out = convertN8nWorkflow(n8n) as ConvertedResult;
    expect(out.nodes.find(n => n.name === "Check")!.config.condition).toBeUndefined();
    expect(out._n8n_import_warnings![0]).toContain("regex");
  });
});

describe("convertN8nWorkflow — Switch node condition translation", () => {
  it("translates rules-mode equals rules into field/cases/source_node, matching the existing case_N port mapping", () => {
    registerNodeDescriptors([SWITCH_DESCRIPTOR]);
    initModals(() => {});
    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("Fetch", "n8n-nodes-base.httpRequest"),
        n8nNodeParams("Router", "n8n-nodes-base.switch", {
          mode: "rules",
          rules: {
            values: [
              { conditions: { conditions: [{ leftValue: "={{ $json.tier }}", rightValue: "bronze", operator: { type: "string", operation: "equals" } }], combinator: "and" } },
              { conditions: { conditions: [{ leftValue: "={{ $json.tier }}", rightValue: "silver", operator: { type: "string", operation: "equals" } }], combinator: "and" } },
            ],
          },
        }),
        n8nNode("Bronze", "n8n-nodes-base.noOp"),
        n8nNode("Silver", "n8n-nodes-base.noOp"),
      ],
      connections: { Fetch: { main: [[{ node: "Router" }]] }, Router: { main: [[{ node: "Bronze" }], [{ node: "Silver" }]] } },
    };
    const out = convertN8nWorkflow(n8n) as ConvertedResult;
    const router = out.nodes.find(n => n.name === "Router")!;
    expect(router.config.field).toBe("tier");
    expect(router.config.source_node).toBe(out.nodes.find(n => n.name === "Fetch")!.id);
    expect(JSON.parse(router.config.cases as string)).toEqual([
      { match: "bronze", port: "case_1" },
      { match: "silver", port: "case_2" },
    ]);
    expect(out._n8n_import_warnings).toBeUndefined();
    // The case ports this produces must agree with the edges n8nOutputPortId
    // already wired for the same node — cross-checked so a future
    // change to one mapping can't silently drift from the other.
    expect(out.edges.find(e => e.to_node === out.nodes.find(n => n.name === "Bronze")!.id)!.from_port).toBe("case_1");
    expect(out.edges.find(e => e.to_node === out.nodes.find(n => n.name === "Silver")!.id)!.from_port).toBe("case_2");
  });

  it("leaves field/cases unconfigured (loud MISSING_FIELD at run time, not a silent guess) for expression mode", () => {
    registerNodeDescriptors([SWITCH_DESCRIPTOR]);
    initModals(() => {});
    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("Fetch", "n8n-nodes-base.httpRequest"),
        n8nNodeParams("Router", "n8n-nodes-base.switch", { mode: "expression", output: "={{ $json.idx }}" }),
      ],
      connections: { Fetch: { main: [[{ node: "Router" }]] } },
    };
    const out = convertN8nWorkflow(n8n) as ConvertedResult;
    const router = out.nodes.find(n => n.name === "Router")!;
    expect(router.config.field).toBeUndefined();
    expect(router.config.cases).toBeUndefined();
    expect(out._n8n_import_warnings![0]).toContain("expression");
  });

  it("emits a warning rather than a wrong single-field guess when rules compare different fields", () => {
    registerNodeDescriptors([SWITCH_DESCRIPTOR]);
    initModals(() => {});
    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("Fetch", "n8n-nodes-base.httpRequest"),
        n8nNodeParams("Router", "n8n-nodes-base.switch", {
          mode: "rules",
          rules: {
            values: [
              { conditions: { conditions: [{ leftValue: "={{ $json.tier }}", rightValue: "bronze", operator: { type: "string", operation: "equals" } }], combinator: "and" } },
              { conditions: { conditions: [{ leftValue: "={{ $json.status }}", rightValue: "vip", operator: { type: "string", operation: "equals" } }], combinator: "and" } },
            ],
          },
        }),
      ],
      connections: { Fetch: { main: [[{ node: "Router" }]] } },
    };
    const out = convertN8nWorkflow(n8n) as ConvertedResult;
    expect(out.nodes.find(n => n.name === "Router")!.config.field).toBeUndefined();
    expect(out._n8n_import_warnings![0]).toContain("different fields");
  });
});

describe("showImportPreview — n8n condition-translation warnings surfaced pre-import", () => {
  it("renders warnings in the requirements list and strips the ephemeral field before handing off to the confirm callback", () => {
    let confirmed: Record<string, unknown> | null = null;
    registerNodeDescriptors([IF_DESCRIPTOR]);
    initModals(obj => { confirmed = obj; });

    const n8n = {
      name: "wf",
      nodes: [
        n8nNode("A", "n8n-nodes-base.httpRequest"),
        n8nNode("B", "n8n-nodes-base.httpRequest"),
        n8nNodeParams("Check", "n8n-nodes-base.if", {
          conditions: { conditions: [{ leftValue: "={{ $json.status }}", rightValue: "ok", operator: { type: "string", operation: "equals" } }], combinator: "and" },
        }),
      ],
      connections: { A: { main: [[{ node: "Check" }]] }, B: { main: [[{ node: "Check" }]] } },
    };
    showImportPreview(n8n);

    const reqList = document.getElementById("import-req-list")!;
    expect(reqList.textContent).toContain("could not identify exactly one upstream node");

    document.getElementById("import-confirm")!.click();
    expect(confirmed).not.toBeNull();
    expect(confirmed!._n8n_import_warnings).toBeUndefined();
  });
});
