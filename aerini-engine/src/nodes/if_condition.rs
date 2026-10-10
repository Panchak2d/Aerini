use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};

pub struct IfConditionNode;

#[async_trait]
impl Node for IfConditionNode {
    fn type_id(&self) -> &'static str { "if_condition" }
    fn display_name(&self) -> &'static str { "If / Condition" }
    fn node_type(&self) -> NodeType { NodeType::Logic }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Branch the workflow on a condition. True paths go to one output, false to another. Set Left Value, Operator and Right Value, or write one Condition string. Text comparisons (==, !=, contains) ignore case; numbers compare numerically." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "lhs": {
                    "type": "string",
                    "description": "Left Value. With Operator and Right Value, replaces Condition: each side is kept whole, so a value that contains '>' or ' contains ' is not misread."
                },
                "op": {
                    "type": "string",
                    "enum": STRUCTURED_OPS,
                    "description": "Operator for Left Value and Right Value. ==, != and contains ignore case; the others compare numbers."
                },
                "rhs": {
                    "type": "string",
                    "description": "Right Value."
                },
                "condition": {
                    "type": "string",
                    "description": "Condition as one string, used when Left Value and Right Value are both blank. Examples: '{{temperature_2m}} > 20', '{{status}} == ok', '{{count}} >= 5', '{{tags}} contains urgent'. ==, != and contains ignore case."
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
                port_type: None,
                arity: PortArity::Single,
            }],
            outputs: vec![
                PortDefinition {
                    id: "on_true".to_string(),
                    label: "True".to_string(),
                    position: PortPosition::Right,
                    port_type: None,
                    arity: PortArity::Single,
                },
                PortDefinition {
                    id: "on_false".to_string(),
                    label: "False".to_string(),
                    position: PortPosition::Right,
                    port_type: None,
                    arity: PortArity::Single,
                },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let (lhs, rhs) = (field_text(&input.input["lhs"]), field_text(&input.input["rhs"]));
        if !lhs.is_empty() || !rhs.is_empty() {
            let op = match field_text(&input.input["op"]) {
                o if o.is_empty() => "==".to_string(),
                o => o,
            };
            if !STRUCTURED_OPS.contains(&op.as_str()) {
                return NodeOutput::failure(NodeError::unrecoverable(
                    "INVALID_CONFIG",
                    format!("op must be one of: {}", STRUCTURED_OPS.join(", ")),
                ));
            }
            let result = compare(&op, &lhs, &rhs);
            let condition = format!("{lhs} {op} {rhs}");
            return NodeOutput::success_with_logs(
                json!({
                    "result": result,
                    "branch": if result { "on_true" } else { "on_false" },
                    "condition": condition,
                }),
                vec![format!("Condition '{}' = {}", condition, result)],
            );
        }

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


const SYMBOL_OPS: [&str; 7] = [">=", "<=", "!=", "==", ">", "<", "="];

/// Operators the Operator field offers.
const STRUCTURED_OPS: [&str; 7] = ["==", "!=", ">", ">=", "<", "<=", "contains"];

/// A config field as comparison text: strings are trimmed, numbers and bools
/// are printed, anything else is empty.
fn field_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// `lhs op rhs` for an operator from `SYMBOL_OPS` or `contains`. Two numbers
/// compare numerically; anything else compares as text, ignoring case.
fn compare(op: &str, lhs: &str, rhs: &str) -> bool {
    if op == "contains" {
        return lhs.to_lowercase().contains(&rhs.to_lowercase());
    }
    if let (Some(l), Some(r)) = (parse_number(lhs), parse_number(rhs)) {
        return match op {
            ">=" => l >= r,
            "<=" => l <= r,
            "!=" => (l - r).abs() > f64::EPSILON,
            ">"  => l > r,
            "<"  => l < r,
            _    => (l - r).abs() <= f64::EPSILON,
        };
    }
    match op {
        "==" | "=" => lhs.to_lowercase() == rhs.to_lowercase(),
        "!="       => lhs.to_lowercase() != rhs.to_lowercase(),
        _          => false,
    }
}

fn evaluate(expr: &str) -> bool {
    let expr = expr.trim();

    if let Some((lhs, rhs)) = split_contains(expr) {
        return compare("contains", unquote(lhs), unquote(rhs));
    }

    if let Some((pos, op)) = find_symbol_operator(expr) {
        return compare(op, unquote(&expr[..pos]), unquote(&expr[pos + op.len()..]));
    }

    match expr.to_lowercase().as_str() {
        "true" | "yes" | "1" => true,
        "false" | "no" | "0" | "null" | "" => false,
        other => other != "undefined",
    }
}

/// Splits on the first whitespace-delimited `contains`. A missing left or
/// right side is an empty string, so an unset `{{value}}` does not fall
/// through to the bare-value rule.
fn split_contains(expr: &str) -> Option<(&str, &str)> {
    if let Some(pos) = expr.find(" contains ") {
        return Some((&expr[..pos], &expr[pos + " contains ".len()..]));
    }
    if let Some(rest) = expr.strip_prefix("contains ") {
        return Some(("", rest));
    }
    expr.strip_suffix(" contains").map(|lhs| (lhs, ""))
}

/// The symbol operator that starts earliest in `expr`; at the same position
/// the longest one wins (`>=` over `>`, `==` over `=`).
fn find_symbol_operator(expr: &str) -> Option<(usize, &'static str)> {
    let mut best: Option<(usize, &'static str)> = None;
    for op in SYMBOL_OPS {
        if let Some(pos) = expr.find(op) {
            if best.is_none_or(|(b, _)| pos < b) {
                best = Some((pos, op));
            }
        }
    }
    best
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('\'').trim_matches('"')
}

fn parse_number(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|n| n.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ExecutionContext, NodeInput};
    use serde_json::json;
    use std::collections::HashMap;

    fn make_input(condition: &str) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({ "condition": condition }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        }
    }

    // ── evaluate() unit tests ──────────────────────────────────────────────

    #[test]
    fn numeric_greater_than_true()  { assert!(evaluate("42 > 20")); }
    #[test]
    fn numeric_greater_than_false() { assert!(!evaluate("5 > 20")); }
    #[test]
    fn numeric_equal()              { assert!(evaluate("10 == 10")); }
    #[test]
    fn numeric_not_equal()         { assert!(evaluate("10 != 5")); }
    #[test]
    fn numeric_lte()               { assert!(evaluate("5 <= 5")); }
    #[test]
    fn string_eq_case_insensitive(){ assert!(evaluate("ok == OK")); }
    #[test]
    fn string_neq()                { assert!(evaluate("yes != no")); }
    #[test]
    fn contains_operator()         { assert!(evaluate("hello world contains world")); }
    #[test]
    fn contains_miss()             { assert!(!evaluate("hello world contains xyz")); }
    #[test]
    fn contains_ignores_case()     { assert!(evaluate("Hello World contains WORLD")); }
    #[test]
    fn contains_wins_over_symbol_in_value() {
        assert!(evaluate("a=b contains b"));
        assert!(evaluate("x>=1 contains >="));
        assert!(!evaluate("a=b contains z"));
    }
    #[test]
    fn contains_with_empty_side_does_not_fall_through() {
        assert!(!evaluate("contains foo"));
        assert!(evaluate("foo contains"));
    }
    #[test]
    fn same_position_prefers_longest_operator() {
        assert!(evaluate("5>=5"));
        assert!(!evaluate("5>5"));
        assert!(evaluate("5<=5"));
        assert!(evaluate("10 = 10"));
        assert!(!evaluate("a = a<b"));
    }
    #[test]
    fn non_finite_words_compare_as_text() {
        assert!(evaluate("nan == NaN"));
        assert!(evaluate("inf == inf"));
        assert!(!evaluate("nan > 1"));
    }
    #[test]
    fn quoted_left_side_matches_unquoted_right() { assert!(evaluate("'paid' == paid")); }
    #[test]
    fn bare_true_string()          { assert!(evaluate("true")); }
    #[test]
    fn bare_false_string()         { assert!(!evaluate("false")); }
    #[test]
    fn empty_string_false()        { assert!(!evaluate("")); }

    // ── structured fields ──────────────────────────────────────────────────

    #[test]
    fn structured_sides_are_compared_whole_even_when_they_contain_operator_text() {
        assert!(compare("==", "a=b", "A=B"));
        assert!(compare("contains", "x contains y", "contains"));
        assert!(compare(">=", "10", "9.5"));
        assert!(!compare("<", "10", "9.5"));
        assert!(compare("!=", "a>=b", "a>b"));
        assert!(!compare(">", "abc", "abd"));
    }

    fn structured_input(fields: serde_json::Value) -> NodeInput {
        let mut input = make_input("1 > 3");
        input.input = fields;
        input
    }

    #[tokio::test]
    async fn structured_fields_replace_condition_only_when_a_side_is_set() {
        let used = IfConditionNode
            .execute(structured_input(json!({ "condition": "1 > 3", "lhs": "a=b", "op": "==", "rhs": "a=b" })))
            .await;
        assert_eq!(used.output.as_ref().unwrap()["branch"], "on_true");

        let legacy = IfConditionNode
            .execute(structured_input(json!({ "condition": "5 > 3", "lhs": "", "op": "==", "rhs": "" })))
            .await;
        assert_eq!(legacy.output.as_ref().unwrap()["branch"], "on_true");
        assert_eq!(legacy.output.as_ref().unwrap()["condition"], "5 > 3");

        let default_op = IfConditionNode
            .execute(structured_input(json!({ "lhs": "Paid", "rhs": "paid" })))
            .await;
        assert_eq!(default_op.output.as_ref().unwrap()["branch"], "on_true");
    }

    #[tokio::test]
    async fn unknown_structured_operator_fails_with_invalid_config() {
        let out = IfConditionNode
            .execute(structured_input(json!({ "lhs": "1", "op": "~=", "rhs": "1" })))
            .await;
        assert!(!out.success);
        let err = out.error.expect("expected error");
        assert_eq!(err.code, "INVALID_CONFIG");
        assert!(!err.recoverable);
    }

    // ── execute() output shape ─────────────────────────────────────────────

    #[tokio::test]
    async fn true_branch_output() {
        let out = IfConditionNode.execute(make_input("5 > 3")).await;
        assert!(out.success);
        let o = out.output.as_ref().unwrap();
        assert_eq!(o["branch"], "on_true");
        assert_eq!(o["result"], true);
    }

    #[tokio::test]
    async fn false_branch_output() {
        let out = IfConditionNode.execute(make_input("1 > 3")).await;
        assert!(out.success);
        let o = out.output.as_ref().unwrap();
        assert_eq!(o["branch"], "on_false");
        assert_eq!(o["result"], false);
    }

    #[tokio::test]
    async fn no_condition_defaults_false() {
        let input = NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({}),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        };
        let out = IfConditionNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.as_ref().unwrap()["branch"], "on_false");
    }
}

