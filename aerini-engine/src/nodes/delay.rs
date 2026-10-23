use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts};
use super::util::cfg_f64_opt;

pub struct DelayNode;

#[async_trait]
impl Node for DelayNode {
    fn type_id(&self) -> &'static str { "delay" }
    fn display_name(&self) -> &'static str { "Delay" }
    fn node_type(&self) -> NodeType { NodeType::Utility }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Pause the workflow for a fixed duration before continuing to the next node." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "duration": { "type": "number", "description": "How long to wait", "default": 1 },
                "unit":     { "type": "string",  "enum": ["ms","s","m","h"],        "default": "s" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    fn ports(&self) -> NodePorts {
        NodePorts::default()
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let duration = match cfg_f64_opt(&input.input["duration"], "duration") {
            Ok(v) => v.unwrap_or(1.0),
            Err(e) => return NodeOutput::failure(e),
        };
        let duration = duration.max(0.0);
        let unit = match input.input["unit"].as_str().map(str::trim).filter(|u| !u.is_empty()) {
            None => "s",
            Some(u @ ("ms" | "s" | "m" | "h")) => u,
            Some(other) => return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_CONFIG",
                format!("unit must be one of ms, s, m, h (got '{other}')"),
            )),
        };

        let ms: u64 = match unit {
            "ms" => duration,
            "m"  => duration * 60_000.0,
            "h"  => duration * 3_600_000.0,
            _    => duration * 1_000.0,
        }.min(3_600_000.0) as u64; // hard cap: 1 hour

        sleep(Duration::from_millis(ms)).await;

        NodeOutput::success_with_logs(
            json!({ "waited_ms": ms, "duration": duration, "unit": unit }),
            vec![format!("Delayed {} {}", duration, unit)],
        )
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;

    #[tokio::test]
    async fn duration_given_as_numeric_string_is_applied() {
        tokio::time::pause();

        let out = DelayNode.execute(NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "n1".into(),
            workflow_id:  "w1".into(),
            execution_id: "e1".into(),
            input:        json!({ "duration": "2", "unit": "s" }),
            context:      ExecutionContext::default(),
        }).await;

        assert!(out.success);
        assert_eq!(out.output.expect("expected output data")["waited_ms"], json!(2000));
    }

    #[tokio::test]
    async fn unparseable_duration_fails_with_invalid_config_instead_of_defaulting() {
        let out = DelayNode.execute(NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "n1".into(),
            workflow_id:  "w1".into(),
            execution_id: "e1".into(),
            input:        json!({ "duration": "abc", "unit": "s" }),
            context:      ExecutionContext::default(),
        }).await;

        assert!(!out.success);
        let err = out.error.expect("expected error");
        assert_eq!(err.code, "INVALID_CONFIG");
        assert!(!err.recoverable);
    }

    #[tokio::test]
    async fn unknown_unit_fails_with_invalid_config() {
        let out = DelayNode.execute(NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "n1".into(),
            workflow_id:  "w1".into(),
            execution_id: "e1".into(),
            input:        json!({ "duration": 5, "unit": "min" }),
            context:      ExecutionContext::default(),
        }).await;

        assert!(!out.success);
        assert_eq!(out.error.expect("expected error").code, "INVALID_CONFIG");
    }

    #[tokio::test]
    async fn negative_duration_clamps_to_zero() {
        let out = DelayNode.execute(NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "n1".into(),
            workflow_id:  "w1".into(),
            execution_id: "e1".into(),
            input:        json!({ "duration": -3, "unit": "s" }),
            context:      ExecutionContext::default(),
        }).await;

        assert!(out.success);
        assert_eq!(out.output.expect("expected output data")["waited_ms"], json!(0));
    }
}
