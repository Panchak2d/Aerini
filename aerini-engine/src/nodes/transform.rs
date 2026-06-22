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
