use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::NodePorts;

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
                "index":      { "type": "number", "description": "Array index for array_get", "default": 0 }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "result": {} } })
    }

    fn ports(&self) -> NodePorts { NodePorts::default() }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let op = input.input["operation"].as_str().unwrap_or("extract");

        match op {
            "parse" => {
                let text = input.input["input_text"].as_str().unwrap_or("{}");
                match serde_json::from_str::<Value>(text) {
                    Ok(v)  => NodeOutput::success(json!({ "result": v })),
                    Err(e) => NodeOutput::failure(
                        NodeError::unrecoverable("PARSE_FAILED", e.to_string())
                    ),
                }
            }

            "stringify" => {
                let combined: Value = json!(input.context.node_outputs);
                let s = serde_json::to_string_pretty(&combined).unwrap_or_default();
                NodeOutput::success(json!({ "result": s }))
            }

            "extract" => {
                let pointer = input.input["pointer"].as_str().unwrap_or("/");
                let combined = json!(input.context.node_outputs);

                let ptr = if pointer.starts_with('/') {
                    pointer.to_string()
                } else {
                    format!("/{}", pointer)
                };

                let extracted = combined.pointer(&ptr).cloned().unwrap_or(Value::Null);
                NodeOutput::success_with_logs(
                    json!({ "result": extracted }),
                    vec![format!("Extracted pointer '{}'", ptr)],
                )
            }

            "merge" => {
                let mut merged = serde_json::Map::new();
                for output_val in input.context.node_outputs.values() {
                    if let Some(obj) = output_val.as_object() {
                        for (k, v) in obj {
                            merged.insert(k.clone(), v.clone());
                        }
                    }
                }
                NodeOutput::success(json!({ "result": merged }))
            }

            "array_get" => {
                let idx = input.input["index"].as_u64().unwrap_or(0) as usize;

                for output_val in input.context.node_outputs.values() {
                    if let Some(arr) = output_val.as_array() {
                        let item = arr.get(idx).cloned().unwrap_or(Value::Null);
                        return NodeOutput::success(json!({ "result": item, "index": idx, "length": arr.len() }));
                    }
                    // Check one level deeper
                    if let Some(obj) = output_val.as_object() {
                        for v in obj.values() {
                            if let Some(arr) = v.as_array() {
                                let item = arr.get(idx).cloned().unwrap_or(Value::Null);
                                return NodeOutput::success(json!({ "result": item, "index": idx, "length": arr.len() }));
                            }
                        }
                    }
                }

                NodeOutput::success(json!({ "result": null, "index": idx, "note": "no array found in context" }))
            }

            other => NodeOutput::failure(
                NodeError::unrecoverable("UNKNOWN_OPERATION", format!("'{}' is not a valid operation", other))
            ),
        }
    }
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
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext {
                variables: HashMap::new(),
                node_outputs: Arc::new(node_outputs),
                metadata: HashMap::new(),
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
