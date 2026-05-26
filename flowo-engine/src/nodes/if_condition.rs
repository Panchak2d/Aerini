use async_trait::async_trait;
use serde_json::{json, Value};

use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

pub struct IfConditionNode;

#[async_trait]
impl Node for IfConditionNode {
    fn type_id(&self) -> &'static str { "if_condition" }
    fn display_name(&self) -> &'static str { "If / Condition" }
    fn node_type(&self) -> NodeType { NodeType::Logic }
    fn version(&self) -> &'static str { "1.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "condition": {
                    "type": "string",
                    "description": "Condition to evaluate. Examples: '{{temperature_2m}} > 20', '{{status}} == ok', '{{count}} >= 5'"
                }
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
            }],
            outputs: vec![
                PortDefinition {
                    id: "on_true".to_string(),
                    label: "True".to_string(),
                    position: PortPosition::Right,
                },
                PortDefinition {
                    id: "on_false".to_string(),
                    label: "False".to_string(),
                    position: PortPosition::Right,
                },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let condition = match input.input["condition"].as_str() {
            Some(c) if !c.trim().is_empty() => c.to_string(),
            _ => {
                return NodeOutput::success_with_logs(
                    json!({ "result": false, "branch": "on_false", "reason": "no condition specified" }),
                    vec!["No condition specified — defaulting to false".to_string()],
                );
            }
        };

        // The executor's build_input resolves all {{...}} expressions before this
        // node executes, so `condition` already contains real values (e.g. "42 > 20").
        // No secondary resolution needed here.
        let result = evaluate(&condition);

        NodeOutput::success_with_logs(
            json!({
                "result": result,
                "branch": if result { "on_true" } else { "on_false" },
                "condition": condition,
            }),
            vec![format!("Condition '{}' = {}", condition, result)],
        )
    }
}


fn evaluate(expr: &str) -> bool {
    let expr = expr.trim();

    // Try operators in longest-first order to avoid partial matches
    for op in &[">=", "<=", "!=", "==", ">", "<", "="] {
        if let Some(pos) = expr.find(op) {
            let lhs = expr[..pos].trim();
            let rhs = expr[pos + op.len()..].trim()
                .trim_matches('\'')
                .trim_matches('"');

            if let (Ok(l), Ok(r)) = (lhs.parse::<f64>(), rhs.parse::<f64>()) {
                return match *op {
                    ">=" => l >= r,
                    "<=" => l <= r,
                    "!=" => (l - r).abs() > f64::EPSILON,
                    ">"  => l > r,
                    "<"  => l < r,
                    _    => (l - r).abs() <= f64::EPSILON,
                };
            }

            return match *op {
                "==" | "=" => lhs.to_lowercase() == rhs.to_lowercase(),
                "!="       => lhs.to_lowercase() != rhs.to_lowercase(),
                _          => false,
            };
        }
    }

    if let Some(pos) = expr.find(" contains ") {
        let lhs = expr[..pos].trim().trim_matches('\'').trim_matches('"');
        let rhs = expr[pos + 10..].trim().trim_matches('\'').trim_matches('"');
        return lhs.contains(rhs);
    }

    match expr.to_lowercase().as_str() {
        "true" | "yes" | "1" => true,
        "false" | "no" | "0" | "null" | "" => false,
        other => other != "undefined",
    }
}
