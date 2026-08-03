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
            "required": ["mappings", "source_node"],
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

        // T1-1i: unify to reject — a null/absent source_node used to merge
        // every upstream node's output into one object keyed by node id.
        // Every "from" JSON pointer in mappings is written assuming direct
        // access to a single node's shape (e.g. "/body/name"); against the
        // merged object those pointers actually need an extra "/node_id"
        // prefix, so the merge silently produced null for every ordinarily-
        // written mapping unless the author knew to account for the
        // wrapping. That fallback is removed; source_node is now required,
        // matching switch.rs/loop_node.rs in this same batch. An explicit-
        // but-unresolvable string is unchanged (still SOURCE_NOT_FOUND
        // below).
        let source_node_value = &input.input["source_node"];
        let source: Value = if source_node_value.is_null() {
            return NodeOutput::failure(NodeError::unrecoverable(
                "SOURCE_NODE_REQUIRED",
                "source_node is required. Leaving it blank previously merged every upstream node's output into one object keyed by node id, which silently broke any \"from\" pointer written for a single node's shape (e.g. \"/body/name\") unless it also accounted for the node-id wrapping. Set source_node to the specific node you want to pull data from.",
            ));
        } else if let Some(source_node) = source_node_value.as_str() {
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
            return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_SOURCE_NODE",
                format!(
                    "source_node is set to a non-string value ({}), which is not a valid node ID",
                    source_node_value
                ),
            ));
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
                ..Default::default()
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
    async fn missing_source_node_key_is_rejected() {
        // T1-1i: unify to reject — the key omitted entirely (the shape the
        // canvas most commonly sends for an unconfigured field) must be
        // rejected. Previously this merged every upstream output into one
        // object and succeeded.
        let mut outputs = HashMap::new();
        outputs.insert("n_a".to_string(), json!({ "val": 99 }));
        let input = make_input(
            json!({
                "mappings": [{ "from": "/n_a/val", "to": "result" }]
            }),
            outputs,
        );
        let out = TransformNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SOURCE_NODE_REQUIRED");
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
    async fn non_string_source_node_returns_error() {
        // Batch A / T1-1g pattern (unchanged by T1-1i): a bare number for
        // "source_node" is a malformed value, not "unset" — rejected with a
        // distinct code from the is_null() reject path (SOURCE_NODE_REQUIRED).
        let input = make_input(
            json!({
                "source_node": 5,
                "mappings": [{ "from": "/x", "to": "y" }]
            }),
            HashMap::new(),
        );
        let out = TransformNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_SOURCE_NODE");
    }

    #[tokio::test]
    async fn explicit_null_source_node_is_rejected() {
        // T1-1i: explicit JSON null is treated the same as an absent key —
        // both are rejected now, unlike a non-string value which was
        // already rejected before this batch (see
        // non_string_source_node_returns_error).
        let mut outputs = HashMap::new();
        outputs.insert("n_a".to_string(), json!({ "val": 99 }));
        let input = make_input(
            json!({
                "source_node": null,
                "mappings": [{ "from": "/n_a/val", "to": "result" }]
            }),
            outputs,
        );
        let out = TransformNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SOURCE_NODE_REQUIRED");
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
