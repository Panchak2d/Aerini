use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::db::WorkflowDb;
use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};
use super::util::cfg_bool_opt;

fn describe_value(value: &Value) -> String {
    match value {
        Value::Null      => "null".to_string(),
        Value::Bool(_)   => "boolean".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(s) => format!("string, {} bytes", s.len()),
        Value::Array(a)  => format!("array, {} items", a.len()),
        Value::Object(o) => format!("object, {} keys", o.len()),
    }
}

pub struct SetVariableNode {
    /// None in serve mode (single-workflow daemon) — persist is a no-op there.
    pub db: Option<Arc<WorkflowDb>>,
}

#[async_trait]
impl Node for SetVariableNode {
    fn type_id(&self) -> &'static str { "set_variable" }
    fn display_name(&self) -> &'static str { "Set Variable" }
    fn node_type(&self) -> NodeType { NodeType::Utility }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Store a value in a named workflow variable accessible to downstream nodes via Get Variable or expressions." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["key"],
            "properties": {
                "key":     { "type": "string", "description": "Variable name" },
                "value":   { "description": "Value to store (any JSON type)" },
                "persist": {
                    "type": "boolean",
                    "description": "If true, persist value to DB so it survives between runs",
                    "default": false
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
            outputs: vec![PortDefinition {
                id: "output".to_string(),
                label: "Out".to_string(),
                position: PortPosition::Right,
                port_type: None,
                arity: PortArity::Single,
            }],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let key = match input.input["key"].as_str() {
            Some(k) if !k.is_empty() => k.to_string(),
            _ => return NodeOutput::failure(
                NodeError::unrecoverable("MISSING_KEY", "key field is required")
            ),
        };

        let value = input.input["value"].clone();
        let persist = match cfg_bool_opt(&input.input["persist"], "persist") {
            Ok(v) => v.unwrap_or(false),
            Err(e) => return NodeOutput::failure(e),
        };

        let mut logs = vec![format!("Set '{}' ({})", key, describe_value(&value))];

        if persist {
            match self.db {
                None => {
                    logs.push(format!(
                        "Warning: persist=true for '{}' has no effect in serve mode — \
                         no WorkflowDb is available. Variable set for this run only.",
                        key
                    ));
                }
                Some(ref db) => {
                    let db_arc = Arc::clone(db);
                    let wf_id = input.workflow_id.clone();
                    let key_clone = key.clone();
                    let val_clone = value.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        db_arc.set_variable(&wf_id, &key_clone, &val_clone)
                    }).await;
                    match result {
                        Ok(Ok(())) => logs.push(format!("Persisted '{}' to DB", key)),
                        Ok(Err(e)) => return NodeOutput::failure(NodeError::unrecoverable(
                            "DB_ERROR",
                            format!("Failed to persist variable '{}' to DB: {}", key, e),
                        )),
                        Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
                            "DB_TASK_PANIC",
                            format!("DB task panicked persisting '{}': {}", key, e),
                        )),
                    }
                }
            }
        }

        NodeOutput::success_with_logs(
            json!({
                "_variable_key":   key,
                "_variable_value": value,
                "key":             key,
                "value":           value
            }),
            logs,
        )
    }
}

pub struct GetVariableNode {
    /// None in serve mode (single-workflow daemon) — DB fallback is a no-op there.
    pub db: Option<Arc<WorkflowDb>>,
}

#[async_trait]
impl Node for GetVariableNode {
    fn type_id(&self) -> &'static str { "get_variable" }
    fn display_name(&self) -> &'static str { "Get Variable" }
    fn node_type(&self) -> NodeType { NodeType::Utility }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Read a named workflow variable set earlier in the run by a Set Variable node." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["key"],
            "properties": {
                "key":     { "type": "string", "description": "Variable name to retrieve" },
                "persist": {
                    "type": "boolean",
                    "description": "If true, fall back to DB when variable not found in this run",
                    "default": false
                },
                "default": {
                    "description": "Value to return when the variable is not found (in this run, or in the DB if persist is true)."
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
            outputs: vec![PortDefinition {
                id: "output".to_string(),
                label: "Out".to_string(),
                position: PortPosition::Right,
                port_type: None,
                arity: PortArity::Single,
            }],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let key = match input.input["key"].as_str() {
            Some(k) if !k.is_empty() => k.to_string(),
            _ => return NodeOutput::failure(
                NodeError::unrecoverable("MISSING_KEY", "key field is required")
            ),
        };

        let default = input.input.get("default").cloned();
        let persist = match cfg_bool_opt(&input.input["persist"], "persist") {
            Ok(v) => v.unwrap_or(false),
            Err(e) => return NodeOutput::failure(e),
        };

        // Check in-run execution context — always, regardless of persist flag
        if let Some(v) = input.context.variables.get(&key) {
            return NodeOutput::success_with_logs(
                json!({ "key": key, "value": v, "found": true }),
                vec![format!("Got '{}' ({})", key, describe_value(v))],
            );
        }

        if persist {
            match self.db {
                None => {}
                Some(ref db) => {
                    let db_arc = Arc::clone(db);
                    let wf_id = input.workflow_id.clone();
                    let key_clone = key.clone();
                    let db_result = tokio::task::spawn_blocking(move || {
                        db_arc.get_variable(&wf_id, &key_clone)
                    }).await;

                    match db_result {
                        Ok(Ok(Some(db_val))) => {
                            return NodeOutput::success_with_logs(
                                json!({ "key": key, "value": db_val, "found": true }),
                                vec![format!("Got '{}' from DB ({})", key, describe_value(&db_val))],
                            );
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(e)) => {
                            return NodeOutput::failure(NodeError::unrecoverable(
                                "DB_ERROR",
                                format!("Failed to read variable '{}' from DB: {}", key, e),
                            ));
                        }
                        Err(e) => {
                            return NodeOutput::failure(NodeError::unrecoverable(
                                "DB_TASK_PANIC",
                                format!("DB task panicked reading '{}': {}", key, e),
                            ));
                        }
                    }
                }
            }
        }

        match default {
            Some(ref d) if !d.is_null() => NodeOutput::success_with_logs(
                json!({ "key": key, "value": d, "found": false }),
                vec![format!(
                    "Variable '{}' not found in this run — using configured default ({})",
                    key, describe_value(d)
                )],
            ),
            _ => NodeOutput::success_with_logs(
                json!({ "key": key, "value": null, "found": false }),
                vec![format!(
                    "Variable '{}' not found in this run and no default configured — \
                     returning null. If you expected a value, ensure a SetVariable node \
                     ran earlier in this execution.",
                    key
                )],
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_set_input(key: &str, value: Value, persist: bool) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id: "set1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({ "key": key, "value": value, "persist": persist }),
            context: ExecutionContext::default(),
        }
    }

    fn make_get_input(key: &str, variables: HashMap<String, Value>) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id: "get1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({ "key": key }),
            context: ExecutionContext {
                variables,
                node_outputs: Arc::new(HashMap::new()),
                metadata: HashMap::new(),
                ..Default::default()
            },
        }
    }

    // ── SetVariableNode ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn set_returns_key_and_value_in_output() {
        let node = SetVariableNode { db: None };
        let out = node.execute(make_set_input("count", json!(42), false)).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["key"], json!("count"));
        assert_eq!(data["value"], json!(42));
        assert_eq!(data["_variable_key"], json!("count"));
        assert_eq!(data["_variable_value"], json!(42));
    }

    #[tokio::test]
    async fn set_missing_key_returns_failure() {
        let node = SetVariableNode { db: None };
        let input = NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id: "n".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "e".to_string(),
            input: json!({ "value": 1 }),
            context: ExecutionContext::default(),
        };
        let out = node.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "MISSING_KEY");
    }

    #[tokio::test]
    async fn set_empty_key_returns_failure() {
        let node = SetVariableNode { db: None };
        let out = node.execute(make_set_input("", json!(1), false)).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "MISSING_KEY");
    }

    #[tokio::test]
    async fn set_persist_true_no_db_adds_warning_log() {
        // serve mode: db=None, persist=true — must warn in logs, not error
        let node = SetVariableNode { db: None };
        let out = node.execute(make_set_input("x", json!("hello"), true)).await;
        assert!(out.success);
        let warn = out.logs.iter().any(|l| l.contains("no effect in serve mode") || l.contains("persist"));
        assert!(warn, "expected warning log about serve-mode persist, got: {:?}", out.logs);
    }

    #[tokio::test]
    async fn set_null_value_allowed() {
        let node = SetVariableNode { db: None };
        let out = node.execute(make_set_input("k", Value::Null, false)).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["value"], Value::Null);
    }

    // ── persistence failure and log hygiene ─────────────────────────────────

    #[tokio::test]
    async fn set_persist_returns_failure_when_db_write_fails_without_leaking_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vars.db");
        let db = Arc::new(crate::db::WorkflowDb::open(&path, 2).unwrap());
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("DROP TABLE workflow_variables", [])
            .unwrap();

        let node = SetVariableNode { db: Some(db) };
        let out = node.execute(make_set_input("tok", json!("s3cret-value"), true)).await;

        assert!(!out.success, "a failed persist must not report success");
        let err = out.error.unwrap();
        assert_eq!(err.code, "DB_ERROR");
        assert!(!err.message.contains("s3cret-value"), "message leaked the value: {}", err.message);
    }

    #[tokio::test]
    async fn set_and_get_logs_describe_the_value_without_containing_it() {
        let set = SetVariableNode { db: None }
            .execute(make_set_input("tok", json!("s3cret-value"), false))
            .await;
        assert!(set.success);
        assert!(set.logs.iter().all(|l| !l.contains("s3cret-value")), "set logs: {:?}", set.logs);

        let mut vars = HashMap::new();
        vars.insert("tok".to_string(), json!("s3cret-value"));
        let get = GetVariableNode { db: None }.execute(make_get_input("tok", vars)).await;
        assert!(get.success);
        assert!(get.logs.iter().all(|l| !l.contains("s3cret-value")), "get logs: {:?}", get.logs);
    }

    // ── GetVariableNode ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_reads_from_context_variables() {
        let node = GetVariableNode { db: None };
        let mut vars = HashMap::new();
        vars.insert("score".to_string(), json!(100));
        let out = node.execute(make_get_input("score", vars)).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["value"], json!(100));
        assert_eq!(data["found"], json!(true));
    }

    #[tokio::test]
    async fn get_missing_key_returns_null_found_false() {
        let node = GetVariableNode { db: None };
        let out = node.execute(make_get_input("absent", HashMap::new())).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["value"], Value::Null);
        assert_eq!(data["found"], json!(false));
    }

    #[tokio::test]
    async fn get_scope_isolation_different_keys() {
        let node = GetVariableNode { db: None };
        let mut vars = HashMap::new();
        vars.insert("a".to_string(), json!(1));
        vars.insert("b".to_string(), json!(2));
        let out_a = node.execute(make_get_input("a", vars.clone())).await;
        let out_b = node.execute(make_get_input("b", vars.clone())).await;
        assert_eq!(out_a.output.unwrap()["value"], json!(1));
        assert_eq!(out_b.output.unwrap()["value"], json!(2));
    }

    #[tokio::test]
    async fn get_overwritten_key_returns_latest() {
        // Simulate overwrite: last write wins (HashMap semantics)
        let node = GetVariableNode { db: None };
        let mut vars = HashMap::new();
        vars.insert("x".to_string(), json!(99)); // "overwritten" value
        let out = node.execute(make_get_input("x", vars)).await;
        assert_eq!(out.output.unwrap()["value"], json!(99));
    }

    #[tokio::test]
    async fn get_empty_key_returns_failure() {
        let node = GetVariableNode { db: None };
        let input = NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id: "n".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "e".to_string(),
            input: json!({ "key": "" }),
            context: ExecutionContext::default(),
        };
        let out = node.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "MISSING_KEY");
    }
}
