use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashSet;

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
    fn description(&self) -> &'static str { "Route the workflow to one of several branches based on a value match, like a switch statement." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["field", "cases", "source_node"],
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
                id: "input".to_string(), label: "In".to_string(), position: PortPosition::Left, port_type: None,
            }],
            outputs: vec![
                PortDefinition { id: "case_1".to_string(), label: "Case 1".to_string(), position: PortPosition::Right, port_type: None },
                PortDefinition { id: "case_2".to_string(), label: "Case 2".to_string(), position: PortPosition::Right, port_type: None },
                PortDefinition { id: "case_3".to_string(), label: "Case 3".to_string(), position: PortPosition::Right, port_type: None },
                PortDefinition { id: "case_4".to_string(), label: "Case 4".to_string(), position: PortPosition::Right, port_type: None },
                PortDefinition { id: "case_5".to_string(), label: "Case 5".to_string(), position: PortPosition::Right, port_type: None },
                PortDefinition { id: "case_6".to_string(), label: "Case 6".to_string(), position: PortPosition::Right, port_type: None },
                PortDefinition { id: "case_7".to_string(), label: "Case 7".to_string(), position: PortPosition::Right, port_type: None },
                PortDefinition { id: "case_8".to_string(), label: "Case 8".to_string(), position: PortPosition::Right, port_type: None },
                PortDefinition { id: "default".to_string(), label: "Default".to_string(), position: PortPosition::Right, port_type: None },
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

        // Resolve each case's port up front, defaulting an unset "port" to its
        // 1-based position (case_1, case_2, ...) instead of a fixed literal —
        // a fixed literal collapses every unconfigured case onto the same
        // port. An explicit "port" is checked against this node's real port
        // set (case_1..case_8, default) so a typo or out-of-range case number
        // fails loudly instead of routing to a port no edge can ever be wired
        // to. Validated before matching so the result doesn't depend on
        // which case happens to match at runtime (Rule 6/S4-1: silent
        // misrouting is worse than a loud config error).
        let valid_ports: HashSet<String> = self.ports().outputs.into_iter().map(|p| p.id).collect();

        let mut resolved_ports: Vec<String> = Vec::with_capacity(cases.len());
        let mut seen_ports: HashSet<String> = HashSet::with_capacity(cases.len());
        for (i, case) in cases.iter().enumerate() {
            let port_value = &case["port"];
            let port = if port_value.is_null() {
                // Key absent or explicitly null — genuinely unset, auto-default
                // to this case's 1-based position.
                let default_idx = i + 1;
                if default_idx > 8 {
                    return NodeOutput::failure(NodeError::unrecoverable(
                        "TOO_MANY_CASES",
                        format!(
                            "case at position {} has no 'port' set and there is no default port beyond 'case_8' (only 8 case ports exist) — set 'port' explicitly for cases beyond the 8th",
                            default_idx
                        ),
                    ));
                }
                format!("case_{}", default_idx)
            } else if let Some(p) = port_value.as_str() {
                if !valid_ports.contains(p) {
                    return NodeOutput::failure(NodeError::unrecoverable(
                        "INVALID_CASE_PORT",
                        format!(
                            "case at position {} sets 'port' to '{}', which is not one of this node's ports (case_1..case_8, default)",
                            i + 1, p
                        ),
                    ));
                }
                p.to_string()
            } else {
                // T1-1d: a non-string, non-null value (bare number, boolean,
                // array, object) reaches here. .as_str() alone can't tell
                // this apart from the key being absent, so it must be
                // checked explicitly — otherwise it silently falls through
                // to the auto-default path above instead of being rejected.
                return NodeOutput::failure(NodeError::unrecoverable(
                    "INVALID_CASE_PORT",
                    format!(
                        "case at position {} sets 'port' to a non-string value ({}), which is not a valid port — use one of case_1..case_8, default",
                        i + 1, port_value
                    ),
                ));
            };
            if !seen_ports.insert(port.clone()) {
                return NodeOutput::failure(NodeError::unrecoverable(
                    "DUPLICATE_CASE_PORT",
                    format!("multiple cases resolve to port '{}' — each case must route to a distinct port", port),
                ));
            }
            resolved_ports.push(port);
        }

        // default_port is checked against the same valid_ports set the loop
        // above already validates each case's explicit port against (T1-1c
        // — identical defect shape to the case["port"] check one field
        // over: an unvalidated arbitrary string here would route the
        // no-match fallback to a port no edge can ever be wired to,
        // previously caught only by the executor's generic drop-warning).
        // Validated up front, before the match loop, so the result doesn't
        // depend on whether any case ends up matching at runtime.
        //
        // Same absent/null-vs-non-string distinction as the case["port"]
        // check above (T1-1e, mirroring T1-1d): a non-string, non-null
        // value must be rejected explicitly rather than silently treated
        // as unset.
        let default_port_value = &input.input["default_port"];
        let default_port = if default_port_value.is_null() {
            "default".to_string()
        } else if let Some(p) = default_port_value.as_str() {
            if !valid_ports.contains(p) {
                return NodeOutput::failure(NodeError::unrecoverable(
                    "INVALID_DEFAULT_PORT",
                    format!(
                        "default_port is set to '{}', which is not one of this node's ports (case_1..case_8, default)",
                        p
                    ),
                ));
            }
            p.to_string()
        } else {
            return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_DEFAULT_PORT",
                format!(
                    "default_port is set to a non-string value ({}), which is not a valid port — use one of case_1..case_8, default",
                    default_port_value
                ),
            ));
        };

        // T1-1i-followup: unify further — an explicit source_node string
        // that names no node in context used to silently resolve to
        // Value::Null (T1-1g's original behavior), inconsistent with
        // transform.rs's SOURCE_NOT_FOUND for the identical shape. A typo'd
        // or stale source_node should fail loudly like every other
        // unresolvable-reference case in this fix, not silently degrade to
        // "no match, fall to default".
        let source_node_value = &input.input["source_node"];
        let data: Value = if source_node_value.is_null() {
            return NodeOutput::failure(NodeError::unrecoverable(
                "SOURCE_NODE_REQUIRED",
                "source_node is required. Leaving it blank previously merged every upstream node's output into one object, which was ambiguous whenever more than one predecessor set the same field. Set source_node to the specific node whose output you want to switch on.",
            ));
        } else if let Some(source_node) = source_node_value.as_str() {
            match input.context.node_outputs.get(source_node) {
                Some(v) => v.clone(),
                None => return NodeOutput::failure(NodeError::unrecoverable(
                    "SOURCE_NOT_FOUND",
                    format!("Node '{}' has no output in context", source_node),
                )),
            }
        } else {
            return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_SOURCE_NODE",
                format!(
                    "source_node is set to a non-string value ({}), which is not a valid node ID",
                    source_node_value
                ),
            ));
        };

        let value = traverse_dotpath(&data, &field);
        let value_str = match &value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b)   => b.to_string(),
            Value::Null      => "null".to_string(),
            other            => other.to_string(),
        };

        for (i, case) in cases.iter().enumerate() {
            let match_value = &case["match"];
            let match_val = if match_value.is_null() {
                // Key absent or explicit null — unchanged from pre-existing
                // behavior (this case simply won't match unless the field's
                // own resolved value is also the literal empty string).
                // Deliberately out of T1-1f's scope: a missing "match" key
                // has no equivalent to port's "auto-default to position",
                // and changing its meaning wasn't part of what was found.
                String::new()
            } else if let Some(s) = match_value.as_str() {
                s.to_string()
            } else {
                // T1-1f: a non-string, non-null match value (bare number or
                // boolean) is now coerced the same way value_str itself is
                // coerced below, so {"match": 5} correctly matches a field
                // that resolves to the number 5. Previously this silently
                // collapsed to "" via .as_str().unwrap_or(""), making the
                // case unreachable except when the field resolves to "".
                match match_value {
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b)   => b.to_string(),
                    other            => other.to_string(),
                }
            };
            if value_str == match_val {
                let port = &resolved_ports[i];
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// T1-1i: source_node is now required, so the shared helper takes it
    /// explicitly. Tests that error out before source_node resolution pass
    /// a placeholder ("src") since it's never read.
    fn make_input(field: &str, cases_json: &str, source_node: &str, outputs: HashMap<String, Value>) -> NodeInput {
        NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({ "field": field, "cases": cases_json, "source_node": source_node }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: Arc::new(outputs),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn routes_to_matching_case() {
        let mut outputs = HashMap::new();
        outputs.insert("prev".to_string(), json!({ "status": "ok" }));
        let input = NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({
                "field": "status",
                "cases": r#"[{"match":"ok","port":"case_1"},{"match":"error","port":"case_2"}]"#,
                "source_node": "prev"
            }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: Arc::new(outputs),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        };
        let out = SwitchNode.execute(input).await;
        assert!(out.success);
        let o = out.output.as_ref().unwrap();
        assert_eq!(o["matched_case"], "ok");
        assert_eq!(o["port"], "case_1");
    }

    #[tokio::test]
    async fn routes_to_default_on_no_match() {
        // source_node resolves fine, its field just doesn't match any case
        // — a genuine no-match, distinct from an unresolvable source_node
        // (see nonexistent_string_source_node_returns_error below, T1-1i
        // followup).
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "status": "nope" }));
        let out = SwitchNode.execute(make_input(
            "status",
            r#"[{"match":"ok","port":"case_1"}]"#,
            "src",
            outputs,
        )).await;
        assert!(out.success);
        let o = out.output.as_ref().unwrap();
        assert_eq!(o["matched_case"], serde_json::Value::Null);
        assert_eq!(o["port"], "default");
    }

    #[tokio::test]
    async fn missing_field_returns_error() {
        let input = NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({ "cases": r#"[{"match":"ok","port":"case_1"}]"# }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: Arc::new(HashMap::new()),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        };
        let out = SwitchNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "MISSING_FIELD");
    }

    #[tokio::test]
    async fn invalid_cases_json_returns_error() {
        let out = SwitchNode.execute(make_input("status", "not-json", "src", HashMap::new())).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_CASES");
    }

    #[tokio::test]
    async fn unconfigured_cases_get_distinct_index_based_default_ports() {
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "status": "b" }));
        let out = SwitchNode.execute(make_input(
            "status",
            r#"[{"match":"a"},{"match":"b"},{"match":"c","port":"case_3"}]"#,
            "src",
            outputs,
        )).await;
        assert!(out.success);
        let o = out.output.as_ref().unwrap();
        // Before the fix both unconfigured cases ("a", "b") defaulted to the
        // same literal "case_1" — matching "b" would have wrongly reported
        // "case_1" instead of its own distinct port.
        assert_eq!(o["matched_case"], "b");
        assert_eq!(o["port"], "case_2");
    }

    #[tokio::test]
    async fn more_than_eight_unconfigured_cases_returns_error() {
        let cases_json = (1..=9)
            .map(|i| format!(r#"{{"match":"v{}"}}"#, i))
            .collect::<Vec<_>>()
            .join(",");
        let out = SwitchNode.execute(make_input(
            "status",
            &format!("[{}]", cases_json),
            "src",
            HashMap::new(),
        )).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "TOO_MANY_CASES");
    }

    #[tokio::test]
    async fn default_port_colliding_with_explicit_port_returns_error() {
        let out = SwitchNode.execute(make_input(
            "status",
            r#"[{"match":"a"},{"match":"b","port":"case_1"}]"#,
            "src",
            HashMap::new(),
        )).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "DUPLICATE_CASE_PORT");
    }

    #[tokio::test]
    async fn explicit_case_port_outside_valid_range_returns_error() {
        // "case_9" doesn't exist — SwitchNode::ports() only defines case_1..case_8.
        let out = SwitchNode.execute(make_input(
            "status",
            r#"[{"match":"a","port":"case_9"}]"#,
            "src",
            HashMap::new(),
        )).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_CASE_PORT");
    }

    #[tokio::test]
    async fn explicit_case_port_typo_returns_error() {
        // Casing typo ("Case_1" vs "case_1") — must not silently pass through
        // to a port no edge can ever be wired to on the canvas.
        let out = SwitchNode.execute(make_input(
            "status",
            r#"[{"match":"a","port":"Case_1"}]"#,
            "src",
            HashMap::new(),
        )).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_CASE_PORT");
    }

    #[tokio::test]
    async fn explicit_case_port_set_to_default_is_valid() {
        // "default" is a real port (SwitchNode::ports() defines it), so a case
        // may explicitly route there — this must not be rejected.
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "status": "a" }));
        let out = SwitchNode.execute(make_input(
            "status",
            r#"[{"match":"a","port":"default"}]"#,
            "src",
            outputs,
        )).await;
        assert!(out.success);
        let o = out.output.as_ref().unwrap();
        assert_eq!(o["matched_case"], "a");
        assert_eq!(o["port"], "default");
    }

    #[tokio::test]
    async fn default_port_outside_valid_range_returns_error() {
        // "nope" isn't case_1..case_8/"default" — must fail loudly instead of
        // silently routing the no-match fallback to a port no edge can ever
        // be wired to on the canvas.
        let input = NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({
                "field": "status",
                "cases": r#"[{"match":"a","port":"case_1"}]"#,
                "default_port": "nope"
            }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: Arc::new(HashMap::new()),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        };
        let out = SwitchNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_DEFAULT_PORT");
    }

    #[tokio::test]
    async fn explicit_default_port_set_to_valid_case_port_is_honored() {
        // default_port may point at any real port, not just the literal
        // "default" — e.g. deliberately routing unmatched values to case_3.
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({}));
        let input = NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({
                "field": "status",
                "cases": r#"[{"match":"a","port":"case_1"}]"#,
                "default_port": "case_3",
                "source_node": "src"
            }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: Arc::new(outputs),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        };
        let out = SwitchNode.execute(input).await;
        assert!(out.success);
        let o = out.output.as_ref().unwrap();
        assert_eq!(o["matched_case"], serde_json::Value::Null);
        assert_eq!(o["port"], "case_3");
    }

    #[tokio::test]
    async fn non_string_case_port_returns_error() {
        // T1-1d: a bare number for "port" must not silently fall through to
        // the positional auto-default the way an absent key correctly does.
        let out = SwitchNode.execute(make_input(
            "status",
            r#"[{"match":"a","port":5}]"#,
            "src",
            HashMap::new(),
        )).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_CASE_PORT");
    }

    #[tokio::test]
    async fn explicit_null_case_port_still_gets_auto_default() {
        // Explicit JSON null is treated the same as an absent key (both are
        // "genuinely unset"), unlike a non-string value — must not be
        // rejected by the T1-1d fix above.
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "status": "a" }));
        let out = SwitchNode.execute(make_input(
            "status",
            r#"[{"match":"a","port":null}]"#,
            "src",
            outputs,
        )).await;
        assert!(out.success);
        let o = out.output.as_ref().unwrap();
        assert_eq!(o["matched_case"], "a");
        assert_eq!(o["port"], "case_1");
    }

    #[tokio::test]
    async fn non_string_default_port_returns_error() {
        // T1-1e: mirror of T1-1d for default_port — a bare number must not
        // silently fall through to the literal "default" fallback.
        let input = NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({
                "field": "status",
                "cases": r#"[{"match":"a","port":"case_1"}]"#,
                "default_port": 5
            }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: Arc::new(HashMap::new()),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        };
        let out = SwitchNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_DEFAULT_PORT");
    }

    #[tokio::test]
    async fn numeric_case_match_value_is_coerced_and_matches() {
        // T1-1f: {"match": 5} (a JSON number) must match a field that
        // resolves to the number 5, the same way {"match": "5"} would —
        // previously this silently coerced to "" and could never match.
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "status": 5 }));
        let out = SwitchNode.execute(make_input(
            "status",
            r#"[{"match":5,"port":"case_1"}]"#,
            "src",
            outputs,
        )).await;
        assert!(out.success);
        let o = out.output.as_ref().unwrap();
        assert_eq!(o["matched_case"], "5");
        assert_eq!(o["port"], "case_1");
    }

    #[tokio::test]
    async fn absent_case_match_key_is_unaffected_by_t1_1f() {
        // A case with no "match" key at all is untouched by the T1-1f fix
        // — it stays inert (never matches a non-empty field value), exactly
        // as before. Locks the deliberate absent/null-vs-non-string
        // boundary rather than leaving it unverified.
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "status": "x" }));
        let out = SwitchNode.execute(make_input(
            "status",
            r#"[{"port":"case_1"}]"#,
            "src",
            outputs,
        )).await;
        assert!(out.success);
        let o = out.output.as_ref().unwrap();
        assert_eq!(o["matched_case"], serde_json::Value::Null);
        assert_eq!(o["port"], "default");
    }

    #[tokio::test]
    async fn non_string_source_node_returns_error() {
        // T1-1g (unchanged by T1-1i): a bare number for "source_node" is a
        // malformed value, not "unset" — source_node is schema-typed as a
        // string, so this must be rejected with a distinct code from the
        // is_null() reject path (SOURCE_NODE_REQUIRED, see below).
        let input = NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({
                "field": "status",
                "cases": r#"[{"match":"ok","port":"case_1"}]"#,
                "source_node": 5
            }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: Arc::new(HashMap::new()),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        };
        let out = SwitchNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_SOURCE_NODE");
    }

    #[tokio::test]
    async fn explicit_null_source_node_is_rejected() {
        // T1-1i: unify to reject — explicit JSON null used to be treated as
        // "genuinely unset" and fall through to the all-outputs merge. That
        // fallback is removed; null is now rejected exactly like an absent
        // key (see missing_source_node_key_is_rejected below).
        let mut outputs = HashMap::new();
        outputs.insert("prev".to_string(), json!({ "status": "ok" }));
        let input = NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({
                "field": "status",
                "cases": r#"[{"match":"ok","port":"case_1"}]"#,
                "source_node": null
            }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: Arc::new(outputs),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        };
        let out = SwitchNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SOURCE_NODE_REQUIRED");
    }

    #[tokio::test]
    async fn missing_source_node_key_is_rejected() {
        // T1-1i: the key omitted entirely (the shape the canvas most
        // commonly sends for an unconfigured field) must be rejected the
        // same way an explicit null is — serde_json::Value indexing yields
        // Value::Null for a missing key, which is_null() already covers,
        // but this locks that specific real-world shape with its own test.
        let mut outputs = HashMap::new();
        outputs.insert("prev".to_string(), json!({ "status": "ok" }));
        let input = NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({
                "field": "status",
                "cases": r#"[{"match":"ok","port":"case_1"}]"#
            }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: Arc::new(outputs),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        };
        let out = SwitchNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SOURCE_NODE_REQUIRED");
    }

    #[tokio::test]
    async fn matching_string_source_node_uses_that_node_only() {
        // the one gap in T1-1g's own test coverage — a source_node
        // set to a string that DOES match an existing node must use ONLY
        // that node's output as `data`, not an error (reserved for a
        // non-matching string, see nonexistent_string_source_node_returns_
        // error below, T1-1i followup). (Since T1-1i, null/absent is
        // rejected outright rather than falling back to a merge — see
        // missing_source_node_key_is_rejected.)
        let mut outputs = HashMap::new();
        outputs.insert("prev".to_string(), json!({ "status": "ok" }));
        outputs.insert("other".to_string(), json!({ "status": "should_not_be_used" }));
        let input = NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({
                "field": "status",
                "cases": r#"[{"match":"ok","port":"case_1"}]"#,
                "source_node": "prev"
            }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: Arc::new(outputs),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        };
        let out = SwitchNode.execute(input).await;
        assert!(out.success);
        let o = out.output.as_ref().unwrap();
        assert_eq!(o["matched_case"], "ok");
        assert_eq!(o["port"], "case_1");
        assert_eq!(o["data"], json!({ "status": "ok" }));
    }

    #[tokio::test]
    async fn nonexistent_string_source_node_returns_error() {
        // T1-1i followup: a syntactically-valid string that names no node in
        // context now errors (SOURCE_NOT_FOUND), matching transform.rs.
        // Previously resolved to Value::Null and fell through to the
        // default port, silently masking a typo'd/stale source_node.
        let input = NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({
                "field": "status",
                "cases": r#"[{"match":"ok","port":"case_1"}]"#,
                "source_node": "ghost"
            }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: Arc::new(HashMap::new()),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        };
        let out = SwitchNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SOURCE_NOT_FOUND");
    }
}

