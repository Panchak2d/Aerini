use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::NodePorts;

pub struct DelayNode;

#[async_trait]
impl crate::node::Node for DelayNode {
    fn type_id(&self) -> &'static str { "delay" }
    fn display_name(&self) -> &'static str { "Delay" }
    fn node_type(&self) -> NodeType { NodeType::Utility }
    fn version(&self) -> &'static str { "1.0.0" }

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
        let duration = input.input["duration"].as_f64().unwrap_or(1.0);
        let unit     = input.input["unit"].as_str().unwrap_or("s");

        let ms: u64 = match unit {
            "ms" => duration as u64,
            "s"  => (duration * 1_000.0)     as u64,
            "m"  => (duration * 60_000.0)    as u64,
            "h"  => (duration * 3_600_000.0) as u64,
            _    => (duration * 1_000.0)     as u64,
        }.min(3_600_000); // hard cap: 1 hour

        sleep(Duration::from_millis(ms)).await;

        NodeOutput::success_with_logs(
            json!({ "waited_ms": ms, "duration": duration, "unit": unit }),
            vec![format!("Delayed {} {}", duration, unit)],
        )
    }
}
