import { describe, it, expect } from "vitest";
import { codeForLog, friendlyError, friendlyHint } from "../error-map";
import { renderErrorsTab } from "../output-renderer";
import type { WorkflowResult } from "../ipc/workflow";

describe("friendlyError", () => {
  it("maps a documented code to an explanation and a fix", () => {
    const f = friendlyError("PERSIST_UNAVAILABLE", "only_new needs the workflow database");
    expect(f?.explanation).toContain("database");
    expect(f?.fix).toContain("Only New");
  });

  it("covers_the_common_ai_timeout_and_database_codes_with_a_non_empty_fix", () => {
    for (const code of ["BAD_KEY", "TIMEOUT", "REFUSED", "EMPTY_RESPONSE", "QUERY_NOT_READ_ONLY", "ROW_LIMIT_EXCEEDED", "UNKNOWN_OPERATION", "OAUTH_NO_REFRESH_TOKEN", "SMTP_ERROR"]) {
      const f = friendlyError(code, "raw");
      expect(f?.explanation.length).toBeGreaterThan(0);
      expect(f?.fix.length).toBeGreaterThan(0);
    }
  });

  it("names the blank field for any MISSING_ code", () => {
    const f = friendlyError("MISSING_DATABASE_ID", "database_id is required");
    expect(f?.explanation).toBe("A required setting is blank: database id.");
    expect(f?.fix).toContain("database id");
  });

  it("returns null for an unknown code so the raw message stands alone", () => {
    expect(friendlyError("SOMETHING_NEW", "odd")).toBeNull();
    expect(friendlyError(undefined, "odd")).toBeNull();
    expect(friendlyHint("SOMETHING_NEW", "odd")).toBe("");
  });

  it("recognises the unregistered node type by message when no code is present", () => {
    const f = friendlyError(undefined, "Node 'x' failed: Node type 'foo' is not registered");
    expect(f?.fix).toContain("Plugins");
  });
});

describe("renderErrorsTab with codes", () => {
  const base = (code?: string): WorkflowResult => ({
    execution_id: "e", workflow_id: "w", success: false, node_outputs: {},
    logs: [{ timestamp: new Date().toISOString(), level: "error", message: "key field is required", code }],
  });

  it("shows the friendly hint and keeps the raw message", () => {
    const html = renderErrorsTab(base("MISSING_KEY"), new Map());
    expect(html).toContain("key field is required");
    expect(html).toContain("A required setting is blank: key.");
  });

  it("shows only the raw message for an old run with no code", () => {
    const html = renderErrorsTab(base(undefined), new Map());
    expect(html).toContain("key field is required");
    expect(html).not.toContain("A required setting is blank");
  });
});

describe("code of a failed node's log entry", () => {
  const ts = new Date().toISOString();
  const logs = [
    { timestamp: ts, level: "error" as const, node_id: "n1", message: "key field is required" },
    { timestamp: ts, level: "error" as const, message: "Node 'n1' failed: key field is required", code: "MISSING_KEY" },
  ];

  it("a_node_attributed_error_takes_the_code_from_the_matching_coded_entry", () => {
    expect(codeForLog(logs[0], logs)).toBe("MISSING_KEY");
    expect(codeForLog({ node_id: "n2" }, logs)).toBeUndefined();
  });

  it("errors_tab_shows_the_hint_for_a_node_attributed_entry", () => {
    const html = renderErrorsTab({ execution_id: "e", workflow_id: "w", success: false, node_outputs: {}, logs }, new Map());
    expect(html).toContain("A required setting is blank: key.");
  });
});
