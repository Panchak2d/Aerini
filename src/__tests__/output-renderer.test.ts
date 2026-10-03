import { describe, it, expect, vi } from "vitest";
import { extractPreview, syntaxHighlight, renderErrorsTab, renderSummaryTab, renderLogsView, renderDebugView, ICON_CIRCLE_ALERT } from "../output-renderer";
import type { WorkflowResult } from "../ipc/workflow";

// output-renderer.ts imports invoke/convertFileSrc from @tauri-apps/api/core.
// Mock the module so the import does not throw in Node.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((path: string) => path),
}));

// ---------------------------------------------------------------------------
// extractPreview — null / undefined
// ---------------------------------------------------------------------------

describe("extractPreview — null / undefined", () => {
  it("returns 'null' for null input", () => {
    expect(extractPreview(null)).toBe("null");
  });

  it("returns 'null' for undefined input", () => {
    expect(extractPreview(undefined)).toBe("null");
  });
});

describe("extractPreview — primitive values", () => {
  it("returns a plain string directly (no HTML escaping)", () => {
    expect(extractPreview("hello world")).toBe("hello world");
  });

  it("returns angle-bracket strings unchanged (not escaped)", () => {
    // extractPreview is a preview function, not a renderer; it never HTML-escapes
    expect(extractPreview("<b>bold</b>")).toBe("<b>bold</b>");
  });

  it("truncates strings longer than 120 characters", () => {
    const long = "x".repeat(200);
    expect(extractPreview(long).length).toBe(120);
  });

  it("returns string form of a number", () => {
    expect(extractPreview(42)).toBe("42");
  });

  it("returns string form of booleans", () => {
    expect(extractPreview(true)).toBe("true");
    expect(extractPreview(false)).toBe("false");
  });
});

describe("extractPreview — object priority keys", () => {
  it("extracts 'content' key first", () => {
    expect(extractPreview({ content: "AI response" })).toBe("AI response");
  });

  it("extracts 'result' key", () => {
    expect(extractPreview({ result: "computed" })).toBe("computed");
  });

  it("extracts 'body' key", () => {
    expect(extractPreview({ body: "http body" })).toBe("http body");
  });

  it("extracts 'value' key", () => {
    expect(extractPreview({ value: "stored" })).toBe("stored");
  });

  it("extracts 'text' key", () => {
    expect(extractPreview({ text: "plain text" })).toBe("plain text");
  });

  it("falls back to JSON.stringify for objects with no priority key", () => {
    const result = extractPreview({ unknown_key: "something" });
    expect(result).toContain("unknown_key");
  });
});

// ---------------------------------------------------------------------------
// syntaxHighlight — HTML escaping contract
// ---------------------------------------------------------------------------

describe("syntaxHighlight — XSS escaping", () => {
  it("escapes < and > so raw script tags never appear in output", () => {
    const out = syntaxHighlight("<script>alert(1)</script>");
    expect(out).not.toContain("<script>");
    expect(out).toContain("&lt;script&gt;");
  });

  it("escapes & so ampersands don't produce double-encoding elsewhere", () => {
    const out = syntaxHighlight("a & b");
    expect(out).toContain("&amp;");
    expect(out).not.toContain("&amp;amp;");
  });
});

// ---------------------------------------------------------------------------
// syntaxHighlight — span wrapping (values that survive escapeHtml unchanged)
// ---------------------------------------------------------------------------

describe("syntaxHighlight — literal keyword spans", () => {
  it("wraps null in json-null span", () => {
    expect(syntaxHighlight("null")).toContain('<span class="json-null">null</span>');
  });

  it("wraps true in json-bool span", () => {
    expect(syntaxHighlight("true")).toContain('<span class="json-bool">true</span>');
  });

  it("wraps false in json-bool span", () => {
    expect(syntaxHighlight("false")).toContain('<span class="json-bool">false</span>');
  });

  it("wraps a standalone integer in json-num span", () => {
    expect(syntaxHighlight("42")).toContain('<span class="json-num">42</span>');
  });

  it("wraps a negative number in json-num span", () => {
    expect(syntaxHighlight("-7")).toContain('<span class="json-num">-7</span>');
  });
});

describe("syntaxHighlight — balanced span tags", () => {
  it("all opened spans are closed — no orphan tags", () => {
    // Use JSON with only literals that survive escapeHtml (no string values)
    const out = syntaxHighlight(JSON.stringify([1, true, null, false, 42]));
    const opens  = (out.match(/<span/g)  ?? []).length;
    const closes = (out.match(/<\/span>/g) ?? []).length;
    expect(opens).toBe(closes);
    expect(opens).toBeGreaterThan(0);
  });
});

describe("syntaxHighlight — string values (behaviour note)", () => {
  it("double-quote chars are HTML-escaped to &quot; — json-str spans are NOT produced", () => {
    // This documents the actual behaviour: escapeHtml converts " → &quot; before
    // the span-wrapping regex runs, so no json-str or json-key spans are emitted.
    const out = syntaxHighlight('"hello"');
    expect(out).toBe("&quot;hello&quot;");
    expect(out).not.toContain("json-str");
  });
});

// ---------------------------------------------------------------------------
// renderErrorsTab — shared error icon
// ---------------------------------------------------------------------------

describe("renderErrorsTab — error icon", () => {
  it("includes the shared circle-alert icon markup for each error card", () => {
    const result: WorkflowResult = {
      execution_id: "e1", workflow_id: "w1", success: false,
      node_outputs: {},
      logs: [{ level: "error", message: "boom", node_id: "n1", timestamp: new Date().toISOString() }],
    };
    const html = renderErrorsTab(result, new Map());
    expect(html).toContain(ICON_CIRCLE_ALERT);
  });

  it("returns the empty-state message when there are no error-level logs", () => {
    const result: WorkflowResult = {
      execution_id: "e1", workflow_id: "w1", success: true,
      node_outputs: {},
      logs: [],
    };
    expect(renderErrorsTab(result, new Map())).toContain("No errors");
  });
});

// ---------------------------------------------------------------------------
// renderSummaryTab — shared error icon
// ---------------------------------------------------------------------------

describe("renderSummaryTab — node type label escaping", () => {
  it("escapes an unmapped node type id taken from the workflow file", () => {
    const result: WorkflowResult = {
      execution_id: "e1", workflow_id: "w1", success: true,
      node_outputs: { n1: "ok" },
      logs: [],
    };
    const nodes = new Map([["n1", { data: { name: "n", node_type_id: "<img src=x>" } }]]) as never;
    const html = renderSummaryTab(result, nodes);
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img src=x&gt;");
  });
});

describe("renderSummaryTab — error icon", () => {
  it("includes the shared circle-alert icon in the error card when the run failed", () => {
    const result: WorkflowResult = {
      execution_id: "e1", workflow_id: "w1", success: false,
      node_outputs: {},
      logs: [{ level: "error", message: "boom", timestamp: new Date().toISOString() }],
    };
    expect(renderSummaryTab(result, new Map())).toContain(ICON_CIRCLE_ALERT);
  });

  it("omits the error card entirely when success is false but no error-level log exists", () => {
    const result: WorkflowResult = {
      execution_id: "e1", workflow_id: "w1", success: false,
      node_outputs: {},
      logs: [{ level: "warn", message: "hmm", timestamp: new Date().toISOString() }],
    };
    expect(renderSummaryTab(result, new Map())).not.toContain("sum-error-card");
  });
});

// ---------------------------------------------------------------------------
// renderDebugView — node_id escaping
// ---------------------------------------------------------------------------

describe("renderDebugView — node_id escaping", () => {
  it("normal case: a plain alphanumeric node id renders unchanged inside the bracket", () => {
    const result: WorkflowResult = {
      execution_id: "e1", workflow_id: "w1", success: true,
      node_outputs: {},
      logs: [{ level: "info", message: "ok", node_id: "node_1727291091", timestamp: new Date().toISOString() }],
    };
    expect(renderDebugView(result)).toContain("[node_1727291]"); // sliced to 12 chars
  });

  it("edge case: a node id containing HTML metacharacters is escaped, not injected raw", () => {
    const result: WorkflowResult = {
      execution_id: "e1", workflow_id: "w1", success: true,
      node_outputs: {},
      logs: [{ level: "info", message: "ok", node_id: `"><img src=x>`, timestamp: new Date().toISOString() }],
    };
    const html = renderDebugView(result);
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
  });
});

// ---------------------------------------------------------------------------
// renderLogsView — click-to-node data
// ---------------------------------------------------------------------------

describe("renderLogsView — data-node-id", () => {
  it("normal case: a log entry with a node id gets a data-node-id attribute and a visible badge", () => {
    const result: WorkflowResult = {
      execution_id: "e1", workflow_id: "w1", success: true,
      node_outputs: {},
      logs: [{ level: "info", message: "hello", node_id: "node_abc123", timestamp: new Date().toISOString() }],
    };
    const html = renderLogsView(result);
    expect(html).toContain('data-node-id="node_abc123"');
    expect(html).toContain('<span class="log-node">[node_abc123]</span>');
    expect(html).toContain('tabindex="0"');
    expect(html).toContain('role="button"');
  });

  it("edge case: a log entry with no node id gets no attribute and no badge (unchanged from before)", () => {
    const result: WorkflowResult = {
      execution_id: "e1", workflow_id: "w1", success: true,
      node_outputs: {},
      logs: [{ level: "info", message: "hello", timestamp: new Date().toISOString() }],
    };
    const html = renderLogsView(result);
    expect(html).not.toContain("data-node-id");
    expect(html).not.toContain("log-node");
  });

  it("edge case: a node id with quote/angle-bracket characters is escaped in both the attribute and the badge", () => {
    const result: WorkflowResult = {
      execution_id: "e1", workflow_id: "w1", success: true,
      node_outputs: {},
      logs: [{ level: "info", message: "hello", node_id: `n"><script>`, timestamp: new Date().toISOString() }],
    };
    const html = renderLogsView(result);
    expect(html).not.toContain('"><script>');
    expect(html).not.toContain("<script>");
    expect(html).toContain("&quot;&gt;&lt;script&gt;");
  });
});
