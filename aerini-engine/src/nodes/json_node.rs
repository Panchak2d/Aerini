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

