/**
 * @vitest-environment jsdom
 *
 * Batch R: validation.ts residuals found on re-audit of REQUIRED_FIELDS
 * against each node's own Rust-side `input_schema().required` array
 * (aerini-engine/src/nodes/*.rs):
 *
 *  - email_send checked only ["to", "subject"], but email.rs's schema
 *    requires ["smtp_host", "from", "to", "subject", "body"] — a workflow
 *    author could leave smtp_host/from/body blank, pass frontend
 *    validation, and only find out at actual execution time.
 *  - database checked a flat ["db_path", "query"] unconditionally, but the
 *    Rust database node has no unconditional required list at all — the
 *    real requirement depends on db_type (sqlite: db_path+query;
 *    postgres/mysql: connection_url+query; redis: connection_url+
 *    operation+key, per sqlite.rs/postgres.rs/redis.rs's own MISSING_*
 *    checks). The flat list falsely required db_path for every non-sqlite
 *    backend and never checked redis's actual fields at all.
 *  - ai_agent checked only ["goal"], but ai_agent.rs's schema requires
 *    ["goal", "provider"] (found via the same audit pass, Rule 6 inline fix).
 *
 * DEVIATION NOTE: matches the precedent in canvas-safety.test.ts —
 * validateWorkflow(canvas) only reads canvas.nodes, so a minimal duck-typed
 * `{ nodes }` stands in for a real Canvas instead of constructing one (a
 * real Canvas needs a 2D canvas context jsdom doesn't provide).
 */
import { describe, it, expect } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import { CanvasNode, type CanvasNodeData } from "../canvas/Node";
import { validateWorkflow } from "../validation";

// CanvasNode → icon-cache → @tauri-apps/api/core (invoke at module level)
import { vi } from "vitest";
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

function makeNode(
  id: string,
  typeId: string,
  config: Record<string, unknown> = {},
): CanvasNode {
  const data: CanvasNodeData = {
    id,
    node_type_id: typeId,
    node_type: "action",
    name: `Node ${id}`,
    config,
    credentials: {},
    position: { x: 0, y: 0 },
    ports: {
      inputs:  [{ id: "input",  label: "Input",  position: "left"  }],
      outputs: [{ id: "output", label: "Output", position: "right" }],
    },
    input_schema: {},
    output_schema: {},
    retry: { max_attempts: 1, backoff_ms: 500 },
    fallback_node: null,
    dynamic_ports: false,
  };
  return new CanvasNode(data);
}

function canvasOf(...nodes: CanvasNode[]): Canvas {
  const map = new Map(nodes.map(n => [n.data.id, n]));
  // Always include a trigger so hasTrigger's separate error doesn't mask
  // the field-required errors these tests are actually checking.
  const trigger = makeNode("trigger", "manual_trigger");
  map.set(trigger.data.id, trigger);
  return { nodes: map } as unknown as Canvas;
}

describe("validateWorkflow — email_send required fields", () => {
  it("flags smtp_host/from/body as missing even when to+subject are set (regression: pre-fix only checked to+subject)", () => {
    const node = makeNode("n1", "email_send", { to: "a@example.com", subject: "hi" });
    const errors = validateWorkflow(canvasOf(node));
    expect(errors.some(e => e.includes("smtp host"))).toBe(true);
    expect(errors.some(e => e.includes('"Node n1" — from is required'))).toBe(true);
    expect(errors.some(e => e.includes("body is required"))).toBe(true);
  });

  it("passes with all five backend-required fields set", () => {
    const node = makeNode("n1", "email_send", {
      smtp_host: "smtp.example.com", from: "me@example.com",
      to: "a@example.com", subject: "hi", body: "hello",
    });
    const errors = validateWorkflow(canvasOf(node));
    expect(errors).toEqual([]);
  });
});

describe("validateWorkflow — database, db_type-conditional required fields", () => {
  it("sqlite (default): requires db_path + query, not connection_url", () => {
    const missing = makeNode("n1", "database", {});
    expect(validateWorkflow(canvasOf(missing)).some(e => e.includes("db path is required"))).toBe(true);

    const ok = makeNode("n1", "database", { db_path: "./x.db", query: "SELECT 1" });
    expect(validateWorkflow(canvasOf(ok))).toEqual([]);
  });

  it("postgres: requires connection_url + query, NOT db_path (regression: pre-fix falsely required db_path here)", () => {
    const node = makeNode("n1", "database", {
      db_type: "postgres", connection_url: "postgres://localhost/db", query: "SELECT 1",
    });
    expect(validateWorkflow(canvasOf(node))).toEqual([]);
  });

  it("postgres with only db_path set (no connection_url) is still flagged missing", () => {
    const node = makeNode("n1", "database", { db_type: "postgres", db_path: "./x.db", query: "SELECT 1" });
    const errors = validateWorkflow(canvasOf(node));
    expect(errors.some(e => e.includes("connection url is required"))).toBe(true);
  });

  it("mysql: requires connection_url + query, same as postgres", () => {
    const node = makeNode("n1", "database", {
      db_type: "mysql", connection_url: "mysql://localhost/db", query: "SELECT 1",
    });
    expect(validateWorkflow(canvasOf(node))).toEqual([]);
  });

  it("redis: requires connection_url + operation + key, NOT query (regression: pre-fix never checked redis's real fields)", () => {
    const missingAll = makeNode("n1", "database", { db_type: "redis" });
    const errors = validateWorkflow(canvasOf(missingAll));
    expect(errors.some(e => e.includes("connection url is required"))).toBe(true);
    expect(errors.some(e => e.includes("operation is required"))).toBe(true);
    expect(errors.some(e => e.includes("key is required"))).toBe(true);
    // redis has no "query" field at all — must never be flagged as missing.
    expect(errors.some(e => e.includes("query is required"))).toBe(false);

    const ok = makeNode("n1", "database", {
      db_type: "redis", connection_url: "redis://localhost", operation: "get", key: "k",
    });
    expect(validateWorkflow(canvasOf(ok))).toEqual([]);
  });
});

describe("validateWorkflow — ai_agent required fields (Rule 6 inline fix)", () => {
  it("flags provider as missing even when goal is set (regression: pre-fix only checked goal)", () => {
    const node = makeNode("n1", "ai_agent", { goal: "do the thing" });
    const errors = validateWorkflow(canvasOf(node));
    expect(errors.some(e => e.includes("provider is required"))).toBe(true);
  });

  it("passes with both goal and provider set", () => {
    const node = makeNode("n1", "ai_agent", { goal: "do the thing", provider: "anthropic" });
    expect(validateWorkflow(canvasOf(node))).toEqual([]);
  });
});
