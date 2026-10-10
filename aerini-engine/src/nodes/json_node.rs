use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::NodePorts;
use super::util::{cfg_u64_opt, ordered_node_outputs};

pub struct JsonNode;

#[async_trait]
impl crate::node::Node for JsonNode {
    fn type_id(&self) -> &'static str { "json" }
    fn display_name(&self) -> &'static str { "JSON" }
    fn node_type(&self) -> NodeType { NodeType::Utility }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Parse a JSON string into an object, or serialise an object into a JSON string." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["operation"],
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["parse", "stringify", "extract", "merge", "array_get"]
                },
                "input_text": { "type": "string", "description": "JSON string to parse" },
                "pointer":    { "type": "string", "description": "JSON pointer e.g. /user/name or /0/temperature" },
                "index":      { "type": "number", "description": "Array index for array_get", "default": 0 },
                "source_node": { "type": "string", "description": "Node ID whose output stringify, extract, merge and array_get read. Leave blank to read the whole run context (deprecated)." }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "result": {} } })
    }

    fn ports(&self) -> NodePorts { NodePorts::default() }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let op = input.input["operation"].as_str().unwrap_or("extract");

        if op == "parse" {
            let text = input.input["input_text"].as_str().unwrap_or("{}");
            return match serde_json::from_str::<Value>(text) {
                Ok(v)  => NodeOutput::success(json!({ "result": v })),
                Err(e) => NodeOutput::failure(
                    NodeError::unrecoverable("PARSE_FAILED", e.to_string())
                ),
            };
        }

        if !matches!(op, "stringify" | "extract" | "merge" | "array_get") {
            return NodeOutput::failure(
                NodeError::unrecoverable("UNKNOWN_OPERATION", format!("'{}' is not a valid operation", op))
            );
        }

        let scope = match source_scope(&input) {
            Ok(s) => s,
            Err(failure) => return failure,
        };

        let mut logs = Vec::new();
        if scope.is_none() {
            tracing::warn!(node_id = %input.node_id, "{}", WHOLE_CONTEXT_WARNING);
            logs.push(format!("Warning: {}", WHOLE_CONTEXT_WARNING));
        }

        let result = match op {
            "stringify" => {
                let s = match scope {
                    Some(v) => serde_json::to_string_pretty(v),
                    None    => serde_json::to_string_pretty(&json!(input.context.node_outputs)),
                };
                json!({ "result": s.unwrap_or_default() })
            }

            "extract" => {
                let pointer = input.input["pointer"].as_str();
                match (scope, pointer) {
                    (Some(v), None | Some("")) => json!({ "result": v }),
                    (Some(v), Some(p)) => {
                        let ptr = normalize_pointer(p);
                        logs.push(format!("Extracted pointer '{}'", ptr));
                        json!({ "result": v.pointer(&ptr).cloned().unwrap_or(Value::Null) })
                    }
                    (None, p) => {
                        let ptr = normalize_pointer(p.unwrap_or("/"));
                        logs.push(format!("Extracted pointer '{}'", ptr));
                        let combined = json!(input.context.node_outputs);
                        json!({ "result": combined.pointer(&ptr).cloned().unwrap_or(Value::Null) })
                    }
                }
            }

            "merge" => {
                let merged = match scope {
                    Some(v) => v.as_object().cloned().unwrap_or_default(),
                    None => {
                        let mut merged = serde_json::Map::new();
                        for (_, output_val) in ordered_node_outputs(&input.context) {
                            if let Some(obj) = output_val.as_object() {
                                for (k, v) in obj {
                                    merged.insert(k.clone(), v.clone());
                                }
                            }
                        }
                        merged
                    }
                };
                json!({ "result": merged })
            }

            _ => {
                let idx = match cfg_u64_opt(&input.input["index"], "index") {
            Ok(v) => v.unwrap_or(0) as usize,
            Err(e) => return NodeOutput::failure(e),
        };
                let picked = match scope {
                    Some(v) => first_array(v).map(|a| pick_item(a, idx)),
                    None => ordered_node_outputs(&input.context)
                        .iter()
                        .find_map(|(_, v)| first_array(v).map(|a| pick_item(a, idx))),
                };
                let note = if scope.is_some() { "no array found in source node" } else { "no array found in context" };
                picked.unwrap_or_else(|| json!({ "result": null, "index": idx, "note": note }))
            }
        };

        NodeOutput::success_with_logs(result, logs)
    }
}

const WHOLE_CONTEXT_WARNING: &str = "json: reading the whole run context; set source_node";

fn source_scope(input: &NodeInput) -> Result<Option<&Value>, NodeOutput> {
    match &input.input["source_node"] {
        Value::Null => Ok(None),
        Value::String(id) if id.trim().is_empty() => Ok(None),
        Value::String(id) => match input.context.node_outputs.get(id.as_str()) {
            Some(out) => Ok(Some(out)),
            None => Err(NodeOutput::failure(NodeError::unrecoverable(
                "SOURCE_NOT_FOUND",
                format!("Node '{}' has no output in context", id),
            ))),
        },
        other => Err(NodeOutput::failure(NodeError::unrecoverable(
            "INVALID_SOURCE_NODE",
            format!(
                "source_node is set to a non-string value ({}), which is not a valid node ID",
                other
            ),
        ))),
    }
}

fn normalize_pointer(pointer: &str) -> String {
    if pointer.starts_with('/') {
        pointer.to_string()
    } else {
        format!("/{}", pointer)
    }
}

fn first_array(value: &Value) -> Option<&Vec<Value>> {
    value
        .as_array()
        .or_else(|| value.as_object()?.values().find_map(Value::as_array))
}

fn pick_item(arr: &[Value], idx: usize) -> Value {
    json!({ "result": arr.get(idx).cloned().unwrap_or(Value::Null), "index": idx, "length": arr.len() })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use crate::node::Node;
    use serde_json::json;
    use std::sync::Arc;
    use std::collections::HashMap;

    fn make_input(input: Value, node_outputs: HashMap<String, Value>) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext {
                variables: HashMap::new(),
                node_outputs: Arc::new(node_outputs),
                metadata: HashMap::new(),
                ..Default::default()
            },
        }
    }

    /// Same as make_input, but also seeds execution_order — needed for the
    /// determinism tests, which assert on *which* node's data wins, not just
    /// that some deterministic winner exists.
    fn make_input_ordered(input: Value, node_outputs: HashMap<String, Value>, order: Vec<&str>) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext {
                variables: HashMap::new(),
                node_outputs: Arc::new(node_outputs),
                metadata: HashMap::new(),
                execution_order: Arc::new(order.into_iter().map(String::from).collect()),
            },
        }
    }

    // ── parse ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn parse_valid_json_string() {
        let input = make_input(
            json!({ "operation": "parse", "input_text": "{\"x\":1}" }),
            HashMap::new(),
        );
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["result"], json!({ "x": 1 }));
    }

    #[tokio::test]
    async fn parse_invalid_json_returns_failure() {
        let input = make_input(
            json!({ "operation": "parse", "input_text": "not json {{{" }),
            HashMap::new(),
        );
        let out = JsonNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "PARSE_FAILED");
    }

    // ── stringify ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn stringify_serialises_context_outputs() {
        let mut outputs = HashMap::new();
        outputs.insert("n_a".to_string(), json!({ "val": 7 }));
        let input = make_input(json!({ "operation": "stringify" }), outputs);
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        let result = out.output.unwrap();
        let s = result["result"].as_str().unwrap();
        // Must be parseable JSON containing our key
        let reparsed: Value = serde_json::from_str(s).unwrap();
        assert_eq!(reparsed["n_a"]["val"], json!(7));
    }

    #[tokio::test]
    async fn stringify_empty_context_produces_empty_object() {
        let input = make_input(json!({ "operation": "stringify" }), HashMap::new());
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        let s = out.output.unwrap()["result"].as_str().unwrap().to_string();
        let reparsed: Value = serde_json::from_str(&s).unwrap();
        assert!(reparsed.as_object().unwrap().is_empty());
    }

    // ── extract ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn extract_by_pointer() {
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "name": "Bob" }));
        let input = make_input(
            json!({ "operation": "extract", "pointer": "/src/name" }),
            outputs,
        );
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["result"], json!("Bob"));
    }

    #[tokio::test]
    async fn extract_pointer_without_leading_slash_auto_fixed() {
        let mut outputs = HashMap::new();
        outputs.insert("x".to_string(), json!(42));
        let input = make_input(
            json!({ "operation": "extract", "pointer": "x" }),
            outputs,
        );
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["result"], json!(42));
    }

    #[tokio::test]
    async fn extract_missing_path_returns_null_not_failure() {
        let input = make_input(
            json!({ "operation": "extract", "pointer": "/no/such/key" }),
            HashMap::new(),
        );
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["result"], Value::Null);
    }

    // ── merge ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn merge_combines_all_object_outputs() {
        let mut outputs = HashMap::new();
        outputs.insert("a".to_string(), json!({ "x": 1 }));
        outputs.insert("b".to_string(), json!({ "y": 2 }));
        let input = make_input(json!({ "operation": "merge" }), outputs);
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        let result = &out.output.unwrap()["result"];
        assert_eq!(result["x"], json!(1));
        assert_eq!(result["y"], json!(2));
    }

    #[tokio::test]
    async fn merge_skips_non_object_outputs() {
        let mut outputs = HashMap::new();
        outputs.insert("arr".to_string(), json!([1, 2, 3]));
        outputs.insert("obj".to_string(), json!({ "k": "v" }));
        let input = make_input(json!({ "operation": "merge" }), outputs);
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        let result = &out.output.unwrap()["result"];
        assert_eq!(result["k"], json!("v"));
        // Array was skipped — no numeric keys
        assert!(result.get("0").is_none());
    }

    #[tokio::test]
    async fn merge_conflicting_key_deterministically_prefers_most_recently_completed() {
        // two upstream nodes sharing a key "x" must resolve the
        // same way on every run of an identical workflow — the more
        // recently completed node's value wins, per execution_order, not
        // whichever the raw HashMap happened to enumerate last.
        let mut outputs = HashMap::new();
        outputs.insert("z_first".to_string(), json!({ "x": "stale" }));
        outputs.insert("a_second".to_string(), json!({ "x": "fresh" }));
        let input = make_input_ordered(
            json!({ "operation": "merge" }),
            outputs,
            vec!["z_first", "a_second"],
        );
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["result"]["x"], json!("fresh"));
    }

    // ── array_get ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn array_get_returns_item_at_index() {
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!([10, 20, 30]));
        let input = make_input(
            json!({ "operation": "array_get", "index": 2 }),
            outputs,
        );
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["result"], json!(30));
    }

    #[tokio::test]
    async fn array_get_index_out_of_bounds_returns_null() {
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!([1, 2]));
        let input = make_input(
            json!({ "operation": "array_get", "index": 99 }),
            outputs,
        );
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["result"], Value::Null);
    }

    #[tokio::test]
    async fn array_get_no_array_in_context_returns_null() {
        let input = make_input(
            json!({ "operation": "array_get", "index": 0 }),
            HashMap::new(),
        );
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["result"], Value::Null);
    }

    #[tokio::test]
    async fn array_get_deterministically_picks_first_completed_array() {
        // two upstream nodes both carry an array — "first
        // array found" must mean first in execution_order, deterministically,
        // not whichever the raw HashMap enumerated first.
        let mut outputs = HashMap::new();
        outputs.insert("z_second".to_string(), json!([99, 98]));
        outputs.insert("a_first".to_string(), json!([1, 2, 3]));
        let input = make_input_ordered(
            json!({ "operation": "array_get", "index": 0 }),
            outputs,
            vec!["a_first", "z_second"],
        );
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["result"], json!(1));
    }

    // ── source_node ────────────────────────────────────────────────────────

    fn two_node_context() -> HashMap<String, Value> {
        let mut outputs = HashMap::new();
        outputs.insert("a".to_string(), json!({ "name": "Ann", "rows": [1, 2, 3] }));
        outputs.insert("b".to_string(), json!({ "name": "Bob", "secret": "s3cret", "rows": [9] }));
        outputs
    }

    #[tokio::test]
    async fn source_node_limits_every_operation_to_that_nodes_output() {
        let run = |op: Value| async move {
            let out = JsonNode.execute(make_input(op, two_node_context())).await;
            assert!(out.success, "expected success, got: {:?}", out.error);
            out.output.unwrap()
        };

        let s = run(json!({ "operation": "stringify", "source_node": "a" })).await;
        let reparsed: Value = serde_json::from_str(s["result"].as_str().unwrap()).unwrap();
        assert_eq!(reparsed, json!({ "name": "Ann", "rows": [1, 2, 3] }), "stringify must serialise only node a");

        let e = run(json!({ "operation": "extract", "source_node": "b", "pointer": "/name" })).await;
        assert_eq!(e["result"], json!("Bob"), "extract pointer is relative to node b's output");

        let m = run(json!({ "operation": "merge", "source_node": "a" })).await;
        assert_eq!(m["result"], json!({ "name": "Ann", "rows": [1, 2, 3] }), "merge must not pull in node b's keys");

        let g = run(json!({ "operation": "array_get", "source_node": "b", "index": 0 })).await;
        assert_eq!(g["result"], json!(9), "array_get must read node b's array");
    }

    #[tokio::test]
    async fn source_node_extract_without_pointer_returns_whole_output() {
        let input = make_input(
            json!({ "operation": "extract", "source_node": "a" }),
            two_node_context(),
        );
        let out = JsonNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["result"], json!({ "name": "Ann", "rows": [1, 2, 3] }));
    }

    #[tokio::test]
    async fn unknown_source_node_fails_with_source_not_found() {
        let input = make_input(
            json!({ "operation": "merge", "source_node": "ghost" }),
            two_node_context(),
        );
        let out = JsonNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SOURCE_NOT_FOUND");
    }

    #[tokio::test]
    async fn non_string_source_node_fails_with_invalid_source_node() {
        let input = make_input(
            json!({ "operation": "stringify", "source_node": 5 }),
            two_node_context(),
        );
        let out = JsonNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_SOURCE_NODE");
    }

    #[tokio::test]
    async fn unset_or_blank_source_node_warns_about_reading_whole_context() {
        for cfg in [
            json!({ "operation": "stringify" }),
            json!({ "operation": "stringify", "source_node": "  " }),
        ] {
            let out = JsonNode.execute(make_input(cfg, two_node_context())).await;
            assert!(out.success);
            assert!(
                out.logs.iter().any(|l| l.contains("reading the whole run context; set source_node")),
                "expected whole-context warning, got: {:?}", out.logs
            );
        }
        let scoped = JsonNode
            .execute(make_input(json!({ "operation": "stringify", "source_node": "a" }), two_node_context()))
            .await;
        assert!(scoped.logs.iter().all(|l| !l.contains("whole run context")));
    }

    // ── unknown operation ──────────────────────────────────────────────────

    #[tokio::test]
    async fn unknown_operation_returns_failure() {
        let input = make_input(
            json!({ "operation": "explode" }),
            HashMap::new(),
        );
        let out = JsonNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "UNKNOWN_OPERATION");
    }
}
