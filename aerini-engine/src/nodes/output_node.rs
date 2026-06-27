use async_trait::async_trait;
use serde_json::{json, Value};

use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};
use super::util::traverse_dotpath;

/// Output node — the final display node in a workflow.
/// It passes through whatever value arrives on its input port unchanged,
/// and the frontend uses its output to drive the dedicated output viewer.
/// Users see the result directly on the canvas without opening any drawer.
pub struct OutputNode;

#[async_trait]
impl Node for OutputNode {
    fn type_id(&self) -> &'static str { "output" }
    fn display_name(&self) -> &'static str { "Output" }
    fn node_type(&self) -> NodeType { NodeType::Utility }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Mark the final output of the workflow. Required for the Chat Panel and the embeddable widget." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "label":       { "type": "string", "description": "Label shown above the result (default: 'Result')" },
                "source_node": { "type": "string", "description": "Node ID to display output from. Leave blank to use incoming data." },
                "field":       { "type": "string", "description": "Specific field to extract, e.g. 'content' or 'body.text'. Leave blank to show everything." }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "value":       { "description": "The displayed value" },
                "label":       { "type": "string" },
                "type":        { "type": "string", "description": "Detected value type: string | number | boolean | object | array | null" },
                "output_type": { "type": "string", "description": "Set to 'media_batch' when value is a media contract payload" }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortDefinition { id: "input".to_string(), label: "In".to_string(), position: PortPosition::Left, port_type: None },
            ],
            outputs: vec![
                PortDefinition { id: "output".to_string(), label: "Out".to_string(), position: PortPosition::Right, port_type: None },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let label = input.input["label"].as_str().unwrap_or("Result").to_string();
        let field = input.input["field"].as_str().unwrap_or("").to_string();

        let source: Value = if let Some(src_id) = input.input["source_node"].as_str() {
            input.context.node_outputs.get(src_id).cloned().unwrap_or(Value::Null)
        } else {
            // Use all context outputs merged, preferring the most recent node's output
            // by taking the last entry in node_outputs that isn't this node itself
            let mut last: Option<Value> = None;
            for (id, val) in input.context.node_outputs.iter() {
                if id != &input.node_id {
                    last = Some(val.clone());
                }
            }
            last.unwrap_or(input.input.clone())
        };

        let value = if field.is_empty() {
            source
        } else {
            traverse_dotpath(&source, &field)
        };

        let type_name = match &value {
            Value::String(_)  => "string",
            Value::Number(_)  => "number",
            Value::Bool(_)    => "boolean",
            Value::Object(_)  => "object",
            Value::Array(_)   => "array",
            Value::Null       => "null",
        };

        // Detect media contract: object with non-empty "files" array where
        // each element carries filename, data, and mime_type fields.
        let is_media_batch = value
            .get("files")
            .and_then(|f| f.as_array())
            .filter(|arr| !arr.is_empty())
            .map(|arr| {
                arr[0].get("filename").is_some()
                    && arr[0].get("data").is_some()
                    && arr[0].get("mime_type").is_some()
            })
            .unwrap_or(false);

        let mut out = json!({
            "value": value,
            "label": label,
            "type":  type_name
        });
        if is_media_batch {
            out["output_type"] = json!("media_batch");
        }

        NodeOutput::success_with_logs(
            out,
            vec![format!("Output: {} ({})", label, type_name)],
        )
    }
}

