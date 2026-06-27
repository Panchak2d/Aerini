use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

/// Transform node — takes a previous node's output and maps/extracts fields.
/// Config shape:
/// {
///   "source_node": "node_id",        // which node's output to pull from
///   "mappings": [
///     { "from": "/body/user/name", "to": "username" },
///     { "from": "/body/user/email", "to": "email" }
///   ]
/// }
pub struct TransformNode;

#[async_trait]
impl Node for TransformNode {
    fn type_id(&self) -> &'static str { "transform" }
    fn display_name(&self) -> &'static str { "Transform Data" }
    fn node_type(&self) -> NodeType { NodeType::Utility }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Reshape or extract data from the previous node's output using a template or expression." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["mappings"],
            "properties": {
                "source_node": { "type": "string", "description": "Node ID to pull output from" },
                "mappings": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["from", "to"],
                        "properties": {
                            "from": { "type": "string", "description": "JSON pointer into source" },
                            "to":   { "type": "string", "description": "Key in output object" }
                        }
                    }
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "description": "Mapped output fields" })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let mappings = match input.input["mappings"].as_array() {
            Some(m) => m.clone(),
            None => return NodeOutput::failure(
                NodeError::unrecoverable("MISSING_MAPPINGS", "mappings array is required")
            ),
        };

        let source: Value = if let Some(source_node) = input.input["source_node"].as_str() {
            match input.context.node_outputs.get(source_node) {
                Some(v) => v.clone(),
                None => return NodeOutput::failure(
                    NodeError::unrecoverable(
                        "SOURCE_NOT_FOUND",
                        format!("Node '{}' has no output in context", source_node),
                    )
                ),
            }
        } else {
            serde_json::to_value(&input.context.node_outputs).unwrap_or(Value::Null)
        };

        let mut result = serde_json::Map::new();
        let mut logs = vec![];

        for mapping in &mappings {
            let from = mapping["from"].as_str().unwrap_or("");
            let to   = mapping["to"].as_str().unwrap_or("");

            if from.is_empty() || to.is_empty() { continue; }

            // Resolve JSON pointer (e.g. "/body/user/name")
            let value = resolve_pointer(&source, from);
            match value {
                Some(v) => {
                    logs.push(format!("Mapped {} -> {}", from, to));
                    result.insert(to.to_string(), v.clone());
                }
                None => {
                    logs.push(format!("Warning: path '{}' not found in source", from));
                    result.insert(to.to_string(), Value::Null);
                }
            }
        }

        NodeOutput::success_with_logs(Value::Object(result), logs)
    }
}

/// Resolve a JSON pointer like "/body/user/name" against a Value.
fn resolve_pointer<'a>(value: &'a Value, pointer: &str) -> Option<&'a Value> {
    if pointer.is_empty() || pointer == "/" {
        return Some(value);
    }

    let mut current = value;
    for part in pointer.trim_start_matches('/').split('/') {
        // Unescape JSON pointer tokens
        let key = part.replace("~1", "/").replace("~0", "~");
        current = match current {
            Value::Object(map) => map.get(&key)?,
            Value::Array(arr) => {
                let idx: usize = key.parse().ok()?;
                arr.get(idx)?
            }
            _ => return None,
        };
    }
    Some(current)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
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

    // ── Happy path ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn single_mapping_from_pointer() {
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "body": { "name": "Alice" } }));
        let input = make_input(
            json!({
                "source_node": "src",
                "mappings": [{ "from": "/body/name", "to": "username" }]
            }),
            outputs,
        );
        let out = TransformNode.execute(input).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["username"], json!("Alice"));
    }

    #[tokio::test]
    async fn multiple_mappings_all_resolved() {
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "a": 1, "b": "two" }));
        let input = make_input(
            json!({
                "source_node": "src",
                "mappings": [
                    { "from": "/a", "to": "num" },
                    { "from": "/b", "to": "str" }
                ]
            }),
            outputs,
        );
        let out = TransformNode.execute(input).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["num"], json!(1));
        assert_eq!(data["str"], json!("two"));
    }

    #[tokio::test]
    async fn nested_path_traversal() {
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "a": { "b": { "c": 42 } } }));
        let input = make_input(
            json!({
                "source_node": "src",
                "mappings": [{ "from": "/a/b/c", "to": "deep" }]
            }),
            outputs,
        );
        let out = TransformNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["deep"], json!(42));
    }

    #[tokio::test]
    async fn array_index_path() {
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "items": ["x", "y", "z"] }));
        let input = make_input(
            json!({
                "source_node": "src",
                "mappings": [{ "from": "/items/1", "to": "second" }]
            }),
            outputs,
        );
        let out = TransformNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["second"], json!("y"));
    }

    #[tokio::test]
    async fn no_source_node_uses_all_context_outputs() {
        let mut outputs = HashMap::new();
        outputs.insert("n_a".to_string(), json!({ "val": 99 }));
        let input = make_input(
            json!({
                "mappings": [{ "from": "/n_a/val", "to": "result" }]
            }),
            outputs,
        );
        let out = TransformNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["result"], json!(99));
    }

    // ── Edge cases ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn missing_mappings_returns_failure() {
        let input = make_input(json!({}), HashMap::new());
        let out = TransformNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "MISSING_MAPPINGS");
    }

    #[tokio::test]
    async fn source_node_not_in_context_returns_failure() {
        let input = make_input(
            json!({
                "source_node": "ghost",
                "mappings": [{ "from": "/x", "to": "y" }]
            }),
            HashMap::new(),
        );
        let out = TransformNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SOURCE_NOT_FOUND");
    }

    #[tokio::test]
    async fn wrong_path_returns_null_not_panic() {
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "a": 1 }));
        let input = make_input(
            json!({
                "source_node": "src",
                "mappings": [{ "from": "/missing/deeply/nested", "to": "out" }]
            }),
            outputs,
        );
        let out = TransformNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["out"], Value::Null);
    }

    #[tokio::test]
    async fn empty_from_or_to_skipped() {
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "x": 1 }));
        let input = make_input(
            json!({
                "source_node": "src",
                "mappings": [
                    { "from": "", "to": "a" },
                    { "from": "/x", "to": "" },
                    { "from": "/x", "to": "good" }
                ]
            }),
            outputs,
        );
        let out = TransformNode.execute(input).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["good"], json!(1));
        // Empty from/to rows produce no key at all
        assert_eq!(data.as_object().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn json_pointer_escape_tilde1() {
        // '~1' in JSON Pointer represents '/'
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "a/b": "slash" }));
        let input = make_input(
            json!({
                "source_node": "src",
                "mappings": [{ "from": "/a~1b", "to": "slashkey" }]
            }),
            outputs,
        );
        let out = TransformNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["slashkey"], json!("slash"));
    }

    // ── resolve_pointer unit tests ──────────────────────────────────────────

    #[test]
    fn resolve_pointer_root_returns_whole_value() {
        let val = json!({ "a": 1 });
        assert_eq!(resolve_pointer(&val, "/"), Some(&val));
    }

    #[test]
    fn resolve_pointer_empty_string_returns_whole_value() {
        let val = json!(42);
        assert_eq!(resolve_pointer(&val, ""), Some(&val));
    }

    #[test]
    fn resolve_pointer_missing_key_returns_none() {
        let val = json!({ "a": 1 });
        assert_eq!(resolve_pointer(&val, "/b"), None);
    }

    #[test]
    fn resolve_pointer_non_container_returns_none() {
        let val = json!(42);
        assert_eq!(resolve_pointer(&val, "/a"), None);
    }
}
