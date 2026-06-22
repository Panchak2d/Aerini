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

