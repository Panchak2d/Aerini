use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};
use super::util::traverse_dotpath;

pub struct SwitchNode;

#[async_trait]
impl Node for SwitchNode {
    fn type_id(&self) -> &'static str { "switch" }
    fn display_name(&self) -> &'static str { "Switch" }
    fn node_type(&self) -> NodeType { NodeType::Logic }
    fn version(&self) -> &'static str { "1.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["field", "cases"],
            "properties": {
                "field": {
                    "type": "string",
                    "description": "Field path to switch on, e.g. status or response.code"
                },
                "source_node": {
                    "type": "string",
                    "description": "Node ID to read field from"
                },
                "cases": {
                    "type": "string",
                    "description": "JSON array of cases: [{\"match\": \"ok\", \"port\": \"case_ok\"}, ...]"
                },
                "default_port": {
                    "type": "string",
                    "description": "Port to route to when no case matches (default: 'default')"
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "matched_case": { "type": "string" },
                "value": {},
                "data": {}
            }
        })
    }

    fn ports(&self) -> NodePorts {
        // 8 static case ports + 1 default.
        // Labels here are generic; the canvas reads the actual case config to show
        // user-defined labels (e.g. "Payment success") alongside the port ID.
        // Long-term fix: dynamic ports via a ports_for_config() method on Node trait
        // — see PLAN.md item 35. This near-term fix unblocks the majority of use cases.
        NodePorts {
            inputs: vec![PortDefinition {
                id: "input".to_string(), label: "In".to_string(), position: PortPosition::Left,
            }],
            outputs: vec![
                PortDefinition { id: "case_1".to_string(), label: "Case 1".to_string(), position: PortPosition::Right },
                PortDefinition { id: "case_2".to_string(), label: "Case 2".to_string(), position: PortPosition::Right },
                PortDefinition { id: "case_3".to_string(), label: "Case 3".to_string(), position: PortPosition::Right },
                PortDefinition { id: "case_4".to_string(), label: "Case 4".to_string(), position: PortPosition::Right },
                PortDefinition { id: "case_5".to_string(), label: "Case 5".to_string(), position: PortPosition::Right },
                PortDefinition { id: "case_6".to_string(), label: "Case 6".to_string(), position: PortPosition::Right },
                PortDefinition { id: "case_7".to_string(), label: "Case 7".to_string(), position: PortPosition::Right },
                PortDefinition { id: "case_8".to_string(), label: "Case 8".to_string(), position: PortPosition::Right },
                PortDefinition { id: "default".to_string(), label: "Default".to_string(), position: PortPosition::Right },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let field = match input.input["field"].as_str() {
            Some(f) if !f.is_empty() => f.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_FIELD", "field is required")),
        };

        let cases_raw = match input.input["cases"].as_str() {
            Some(c) if !c.is_empty() => c,
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_CASES", "cases is required")),
        };

        let cases: Vec<Value> = match serde_json::from_str(cases_raw) {
            Ok(v) => v,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("INVALID_CASES", e.to_string())),
        };

        let data: Value = if let Some(source_node) = input.input["source_node"].as_str() {
            input.context.node_outputs.get(source_node).cloned().unwrap_or(Value::Null)
        } else {
            Value::Object(input.context.node_outputs.iter().map(|(k,v)| (k.clone(), v.clone())).collect())
        };

        let value = traverse_dotpath(&data, &field);
        let value_str = match &value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b)   => b.to_string(),
            Value::Null      => "null".to_string(),
            other            => other.to_string(),
        };

        let default_port = input.input["default_port"].as_str().unwrap_or("default").to_string();

        for case in &cases {
            let match_val = case["match"].as_str().unwrap_or("");
            let port = case["port"].as_str().unwrap_or("case_1");
            if value_str == match_val {
                return NodeOutput::success_with_logs(
                    json!({ "matched_case": match_val, "port": port, "value": value, "data": data }),
                    vec![format!("Switch: field '{}' = '{}' → port '{}'", field, value_str, port)],
                );
            }
        }

        NodeOutput::success_with_logs(
            json!({ "matched_case": null, "port": default_port, "value": value, "data": data }),
            vec![format!("Switch: field '{}' = '{}' → default", field, value_str)],
        )
    }
}
