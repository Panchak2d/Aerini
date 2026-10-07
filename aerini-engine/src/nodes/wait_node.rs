use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};
use super::util::traverse_dotpath;

/// Wait node — pauses the workflow for a fixed duration, or checks a field once.
///
/// Modes:
///   duration  — simple delay (like Delay node but with a timeout output port)
///   condition — checks `field` against `expected` once, immediately. A match leaves
///               through `output`; a mismatch leaves through `timed_out`.
///               `poll_interval_secs` and `timeout_secs` are still accepted so saved
///               workflows load, but have no effect: `context.node_outputs` is fixed
///               when the node starts, so waiting could never change the result.
pub struct WaitNode;

#[async_trait]
impl Node for WaitNode {
    fn type_id(&self)      -> &'static str { "wait" }
    fn display_name(&self) -> &'static str { "Wait" }
    fn node_type(&self)    -> NodeType     { NodeType::Utility }
    fn version(&self)      -> &'static str { "1.0.0" }
    fn description(&self)  -> &'static str { "Pause the workflow for a fixed duration, or check a field once and route to Done (match) or Timed out (no match)." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["mode"],
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["duration", "condition"],
                    "description": "duration: wait a fixed time. condition: check once whether a value matches; routes to Done if it does, Timed out if not (no waiting)."
                },
                "duration_secs": {
                    "type": "number",
                    "description": "Seconds to wait (duration mode). Default 5."
                },
                "field": {
                    "type": "string",
                    "description": "Dot-path into upstream node outputs to check, e.g. 'node_http.status' (condition mode)."
                },
                "expected": {
                    "description": "Value the field must equal to leave through Done (condition mode)."
                },
                "poll_interval_secs": {
                    "type": "number",
                    "description": "No longer used. Kept so saved workflows load; the condition is checked once."
                },
                "timeout_secs": {
                    "type": "number",
                    "description": "No longer used. Kept so saved workflows load; a condition that does not match routes to Timed out immediately."
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
                PortDefinition { id: "input".to_string(),     label: "In".to_string(),       position: PortPosition::Left,  port_type: None, arity: PortArity::Single },
            ],
            outputs: vec![
                PortDefinition { id: "output".to_string(),    label: "Done".to_string(),     position: PortPosition::Right, port_type: None, arity: PortArity::Single },
                PortDefinition { id: "timed_out".to_string(), label: "Timed out".to_string(),position: PortPosition::Right, port_type: None, arity: PortArity::Single },
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
                let field    = input.input["field"].as_str().unwrap_or("").to_string();
                let expected = input.input["expected"].clone();

                if field.is_empty() {
                    return NodeOutput::failure(NodeError::unrecoverable(
                        "MISSING_FIELD", "field is required for condition mode"
                    ));
                }

                let context_val = json!(input.context.node_outputs);
                let current = traverse_dotpath(&context_val, &field);

                if current == expected {
                    return NodeOutput::success_with_logs(
                        json!({ "waited_ms": 0, "timed_out": false, "mode": "condition", "matched_value": current }),
                        vec![format!("Condition met: {} == {:?}", field, expected)],
                    );
                }

                NodeOutput::success_with_logs(
                    json!({ "waited_ms": 0, "timed_out": true, "mode": "condition", "branch": "timed_out" }),
                    vec![format!(
                        "Condition not met: {} is {}, expected {}. Routing to Timed out without waiting; upstream outputs cannot change while this node runs",
                        field, current, expected
                    )],
                )
            }

            other => NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_MODE", format!("Unknown mode: '{}'. Use 'duration' or 'condition'.", other)
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use crate::node::Node;

    fn make_input(val: Value) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "n1".into(),
            workflow_id:  "w1".into(),
            execution_id: "e1".into(),
            input:        val,
            context:      ExecutionContext::default(),
        }
    }

    #[tokio::test]
    async fn condition_mismatch_times_out_without_sleeping() {
        tokio::time::pause();
        let started = tokio::time::Instant::now();

        let out = WaitNode.execute(make_input(json!({
            "mode": "condition",
            "field": "never.matches",
            "expected": "x",
            "poll_interval_secs": 100.0,
            "timeout_secs": 999_999.0
        }))).await;

        assert!(started.elapsed() < Duration::from_millis(1));
        assert!(out.success);
        let data = out.output.expect("expected output data");
        assert_eq!(data["timed_out"], json!(true));
        assert_eq!(data["waited_ms"], json!(0));
        assert_eq!(data["branch"], json!("timed_out"));
        assert!(out.logs.iter().any(|l| l.contains("Condition not met")));
    }

    #[tokio::test]
    async fn condition_mismatch_routes_to_timed_out_port() {
        let out = WaitNode.execute(make_input(json!({
            "mode": "condition",
            "field": "never.matches",
            "expected": "x"
        }))).await;

        assert_eq!(
            crate::executor::WorkflowExecutor::resolve_taken_port(&out),
            "timed_out"
        );
    }

    #[tokio::test]
    async fn condition_requires_field() {
        let out = WaitNode.execute(make_input(json!({ "mode": "condition", "expected": 1 }))).await;
        assert!(!out.success);
    }

    #[tokio::test]
    async fn condition_match_stays_on_done_port() {
        let mut input = make_input(json!({
            "mode": "condition",
            "field": "n.v",
            "expected": 1
        }));
        input.context.node_outputs = std::sync::Arc::new(std::collections::HashMap::from([
            ("n".to_string(), json!({ "v": 1 })),
        ]));

        let out = WaitNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.as_ref().unwrap()["timed_out"], json!(false));
        assert_eq!(
            crate::executor::WorkflowExecutor::resolve_taken_port(&out),
            "output"
        );
    }
}

