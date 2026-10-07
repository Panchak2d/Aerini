use async_trait::async_trait;
use serde_json::{json, Value};

use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};

pub struct StopNode;

#[async_trait]
impl Node for StopNode {
    fn type_id(&self) -> &'static str { "stop" }
    fn display_name(&self) -> &'static str { "Stop" }
    fn node_type(&self) -> NodeType { NodeType::Logic }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "End this branch of the workflow. Downstream nodes on this path will not run; other independent branches are unaffected." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "reason": { "type": "string", "description": "Reason to log when stopping" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortDefinition {
                id: "input".to_string(),
                label: "In".to_string(),
                position: PortPosition::Left,
                port_type: None,
                arity: PortArity::Single,
            }],
            outputs: vec![], // Stop has no outputs — it ends the branch
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let reason = input.input["reason"]
            .as_str().unwrap_or("Workflow stopped").to_string();

        NodeOutput::success_with_logs(
            json!({ "stopped": true, "reason": reason }),
            vec![format!("⏹ Stop: {}", reason)],
        )
    }
}

