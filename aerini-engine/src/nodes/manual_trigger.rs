use async_trait::async_trait;
use serde_json::{json, Value};

use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};

pub struct ManualTriggerNode;

#[async_trait]
impl Node for ManualTriggerNode {
    fn type_id(&self) -> &'static str { "manual_trigger" }
    fn display_name(&self) -> &'static str { "Manual Trigger" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Run a workflow manually via the Run button in the app or the server API." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mock_payload": {
                    "type": "string",
                    "description": "Optional JSON payload to inject when running manually"
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: vec![PortDefinition {
                id: "output".to_string(),
                label: "Start".to_string(),
                position: PortPosition::Right,
                port_type: None,
                arity: PortArity::Single,
            }],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        if let Some(s) = input.input["mock_payload"].as_str() {
            if !s.trim().is_empty() {
                if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                    return NodeOutput::success_with_logs(
                        parsed,
                        vec!["Manual trigger fired with payload".to_string()],
                    );
                }
                return NodeOutput::success(json!({ "payload": s }));
            }
        }
        NodeOutput::success_with_logs(
            json!({ "triggered": true, "timestamp": chrono::Utc::now().to_rfc3339() }),
            vec!["Manual trigger fired".to_string()],
        )
    }
}

