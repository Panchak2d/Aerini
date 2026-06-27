use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

pub struct ScheduleNode;

#[async_trait]
impl Node for ScheduleNode {
    fn type_id(&self) -> &'static str { "schedule" }
    fn display_name(&self) -> &'static str { "Schedule" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Run a workflow on a cron schedule, fixed interval, or one-time delay." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["mode"],
            "properties": {
                "mode":          { "type": "string", "enum": ["interval", "cron", "once"] },
                "interval_secs": { "type": "number", "description": "Seconds between runs (interval mode)" },
                "cron_expr":     { "type": "string", "description": "Cron expression e.g. 0 9 * * 1-5" },
                "run_at":        { "type": "string", "description": "ISO timestamp (once mode)" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "triggered_at": { "type": "string" },
                "mode":         { "type": "string" }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: vec![PortDefinition {
                id: "output".to_string(),
                label: "Triggered".to_string(),
                position: PortPosition::Right,
                port_type: None,
            }],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        // The scheduler daemon owns all timing — it sleeps the configured interval
        // before calling the executor. This node must NOT sleep here; doing so causes
        // a double-sleep in background runs (daemon sleep + node sleep = 2× the
        // configured interval). Return immediately with metadata so downstream nodes
        // receive trigger context.
        let mode = match input.input["mode"].as_str() {
            Some(m) => m.to_string(),
            None => return NodeOutput::failure(
                NodeError::unrecoverable("MISSING_MODE", "mode is required")
            ),
        };

        match mode.as_str() {
            "interval" => {
                let secs = input.input["interval_secs"].as_u64().unwrap_or(60).max(10);
                NodeOutput::success(json!({
                    "triggered_at": chrono::Utc::now().to_rfc3339(),
                    "mode": "interval",
                    "interval_secs": secs
                }))
            }
            "once" => {
                NodeOutput::success(json!({
                    "triggered_at": chrono::Utc::now().to_rfc3339(),
                    "mode": "once",
                    "run_at": input.input["run_at"]
                }))
            }
            "cron" => {
                let expr = match input.input["cron_expr"].as_str() {
                    Some(e) if !e.is_empty() => e.to_string(),
                    _ => return NodeOutput::failure(
                        NodeError::unrecoverable("MISSING_CRON", "cron_expr required for cron mode")
                    ),
                };
                NodeOutput::success(json!({
                    "triggered_at": chrono::Utc::now().to_rfc3339(),
                    "mode": "cron",
                    "cron_expr": expr
                }))
            }
            _ => NodeOutput::failure(
                NodeError::unrecoverable("INVALID_MODE", format!("Unknown mode: {}", mode))
            ),
        }
    }
}

