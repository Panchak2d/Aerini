use async_trait::async_trait;
use serde_json::{json, Value};

use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

/// Merge node — collects all upstream node outputs into a single object.
///
/// Config shape:
/// {
///   "mode": "object" | "array"   (default: "object")
///     "object" -> { "node_id_1": <output>, "node_id_2": <output>, ... }
///     "array"  -> [ <output1>, <output2>, ... ]  (order: insertion order of node_outputs)
/// }
pub struct MergeNode;

#[async_trait]
impl Node for MergeNode {
    fn type_id(&self) -> &'static str { "merge" }
    fn display_name(&self) -> &'static str { "Merge" }
    fn node_type(&self) -> NodeType { NodeType::Logic }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Wait for all incoming parallel branches to complete, then continue as a single execution path." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["object", "array"],
                    "description": "How to combine inputs: object (keyed by node ID) or array (values only)"
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "description": "All upstream node outputs merged into one value"
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let mode = input.input["mode"].as_str().unwrap_or("object");

        let outputs = &input.context.node_outputs;

        if outputs.is_empty() {
            return NodeOutput::success(json!({ "merged": {} }));
        }

        let merged: Value = match mode {
            "array" => {
                Value::Array(outputs.values().cloned().collect())
            }
            _ => {
                // "object" — key each output by its source node ID
                let mut map = serde_json::Map::new();
                for (node_id, output) in outputs.iter() {
                    map.insert(node_id.clone(), output.clone());
                }
                Value::Object(map)
            }
        };

        NodeOutput::success_with_logs(
            merged,
            vec![format!("Merged {} upstream output(s) as {}", outputs.len(), mode)],
        )
    }
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

    fn make_input(mode: Option<&str>, node_outputs: HashMap<String, Value>) -> NodeInput {
        let input = match mode {
            Some(m) => json!({ "mode": m }),
            None    => json!({}),
        };
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

    // ── Empty context ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn empty_context_returns_empty_merged() {
        let out = MergeNode.execute(make_input(None, HashMap::new())).await;
        assert!(out.success);
        // merged key present, value is empty object
        let data = out.output.unwrap();
        assert!(data["merged"].is_object());
        assert!(data["merged"].as_object().unwrap().is_empty());
    }

    // ── object mode ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn object_mode_keys_are_node_ids() {
        let mut outputs = HashMap::new();
        outputs.insert("node_a".to_string(), json!({ "x": 1 }));
        outputs.insert("node_b".to_string(), json!({ "y": 2 }));
        let out = MergeNode.execute(make_input(Some("object"), outputs)).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["node_a"]["x"], json!(1));
        assert_eq!(data["node_b"]["y"], json!(2));
    }

    #[tokio::test]
    async fn default_mode_is_object() {
        let mut outputs = HashMap::new();
        outputs.insert("n1".to_string(), json!(42));
        let out = MergeNode.execute(make_input(None, outputs)).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["n1"], json!(42));
    }

    // ── array mode ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn array_mode_produces_values_only() {
        let mut outputs = HashMap::new();
        outputs.insert("a".to_string(), json!(10));
        outputs.insert("b".to_string(), json!(20));
        let out = MergeNode.execute(make_input(Some("array"), outputs)).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert!(data.is_array(), "expected array output, got {:?}", data);
        let arr = data.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // Values present — order may vary (HashMap), so check membership
        let set: std::collections::HashSet<i64> =
            arr.iter().filter_map(|v| v.as_i64()).collect();
        assert!(set.contains(&10));
        assert!(set.contains(&20));
    }

    // ── conflicting keys in object mode ────────────────────────────────────

    #[tokio::test]
    async fn object_mode_different_node_ids_no_key_collision() {
        // Object mode keys by node_id, so two nodes with different IDs never collide
        let mut outputs = HashMap::new();
        outputs.insert("alpha".to_string(), json!({ "val": 1 }));
        outputs.insert("beta".to_string(),  json!({ "val": 2 }));
        let out = MergeNode.execute(make_input(Some("object"), outputs)).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["alpha"]["val"], json!(1));
        assert_eq!(data["beta"]["val"],  json!(2));
    }

    // ── log line ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn log_line_contains_output_count() {
        let mut outputs = HashMap::new();
        outputs.insert("x".to_string(), json!(1));
        outputs.insert("y".to_string(), json!(2));
        let out = MergeNode.execute(make_input(Some("object"), outputs)).await;
        assert!(out.success);
        let has_log = out.logs.iter().any(|l| l.contains('2'));
        assert!(has_log, "expected log with count, got: {:?}", out.logs);
    }

    // ── unknown mode falls back to object ──────────────────────────────────

    #[tokio::test]
    async fn unknown_mode_falls_back_to_object() {
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "k": "v" }));
        let out = MergeNode.execute(make_input(Some("invalid"), outputs)).await;
        assert!(out.success);
        let data = out.output.unwrap();
        // object-mode result: key is node_id "src"
        assert_eq!(data["src"]["k"], json!("v"));
    }
}
