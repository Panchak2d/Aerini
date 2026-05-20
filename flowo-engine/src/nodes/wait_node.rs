use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration, Instant};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};
use super::util::traverse_dotpath;

/// Wait node — pauses the workflow for a fixed duration or until a condition becomes true.
///
/// Modes:
///   duration — simple delay (like Delay node but with a timeout output port)
///   condition — polls a field value every `poll_interval_secs` until it matches `expected`,
///               or until `timeout_secs` expires
pub struct WaitNode;

#[async_trait]
impl Node for WaitNode {
    fn type_id(&self)      -> &'static str { "wait" }
    fn display_name(&self) -> &'static str { "Wait" }
    fn node_type(&self)    -> NodeType     { NodeType::Utility }
    fn version(&self)      -> &'static str { "1.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["mode"],
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["duration", "condition"],
                    "description": "duration: wait a fixed time. condition: poll until a value matches."
                },
                "duration_secs": {
                    "type": "number",
                    "description": "Seconds to wait (duration mode). Default 5."
                },
                "field": {
                    "type": "string",
                    "description": "Dot-path into context to check, e.g. 'node_http.status' (condition mode)."
                },
                "expected": {
                    "description": "Value the field must equal to stop waiting (condition mode)."
                },
                "poll_interval_secs": {
                    "type": "number",
                    "description": "How often to recheck the field, in seconds. Default 2."
                },
                "timeout_secs": {
                    "type": "number",
                    "description": "Max time to wait before routing to the 'timed_out' port. Default 60."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "waited_ms":  { "type": "number" },
                "timed_out":  { "type": "boolean" },
                "mode":       { "type": "string" }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortDefinition { id: "input".to_string(),     label: "In".to_string(),       position: PortPosition::Left  },
            ],
            outputs: vec![
                PortDefinition { id: "output".to_string(),    label: "Done".to_string(),     position: PortPosition::Right },
                PortDefinition { id: "timed_out".to_string(), label: "Timed out".to_string(),position: PortPosition::Right },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let mode = input.input["mode"].as_str().unwrap_or("duration");

        match mode {
            "duration" => {
                let secs = input.input["duration_secs"].as_f64().unwrap_or(5.0)
                    .clamp(0.0, 3600.0);
                let ms = (secs * 1000.0) as u64;
                sleep(Duration::from_millis(ms)).await;
                NodeOutput::success_with_logs(
                    json!({ "waited_ms": ms, "timed_out": false, "mode": "duration" }),
                    vec![format!("Waited {:.1}s", secs)],
                )
            }

            "condition" => {
                let field          = input.input["field"].as_str().unwrap_or("").to_string();
                let expected       = input.input["expected"].clone();
                let poll_secs      = input.input["poll_interval_secs"].as_f64().unwrap_or(2.0).max(0.5);
                let timeout_secs   = input.input["timeout_secs"].as_f64().unwrap_or(60.0).max(1.0);

                if field.is_empty() {
                    return NodeOutput::failure(NodeError::unrecoverable(
                        "MISSING_FIELD", "field is required for condition mode"
                    ));
                }

                let deadline = Instant::now() + Duration::from_secs_f64(timeout_secs);
                let poll_ms  = (poll_secs * 1000.0) as u64;

                loop {
                    // Check the field in context node_outputs
                    let context_val = json!(input.context.node_outputs);
                    let current = traverse_dotpath(&context_val, &field);

                    if current == expected {
                        let waited = Instant::now()
                            .duration_since(deadline - Duration::from_secs_f64(timeout_secs))
                            .as_millis() as u64;
                        return NodeOutput::success_with_logs(
                            json!({ "waited_ms": waited, "timed_out": false, "mode": "condition", "matched_value": current }),
                            vec![format!("Condition met: {} == {:?}", field, expected)],
                        );
                    }

                    if Instant::now() >= deadline {
                        return NodeOutput::success_with_logs(
                            json!({ "waited_ms": (timeout_secs * 1000.0) as u64, "timed_out": true, "mode": "condition" }),
                            vec![format!("Condition timed out after {:.0}s — {} never matched {:?}", timeout_secs, field, expected)],
                        );
                    }

                    sleep(Duration::from_millis(poll_ms)).await;
                }
            }

            other => NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_MODE", format!("Unknown mode: '{}'. Use 'duration' or 'condition'.", other)
            )),
        }
    }
}

