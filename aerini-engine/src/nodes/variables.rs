use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::db::WorkflowDb;
use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

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
            }],
            outputs: vec![PortDefinition {
                id: "output".to_string(),
                label: "Out".to_string(),
                position: PortPosition::Right,
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
        let persist = input.input["persist"].as_bool().unwrap_or(false);

        let mut logs = vec![format!("Set '{}' = {}", key, value)];

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
                        Ok(Err(e)) => logs.push(format!(
                            "Warning: failed to persist '{}' to DB: {}", key, e
                        )),
                        Err(e) => logs.push(format!(
                            "Warning: DB task panicked persisting '{}': {}", key, e
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
            outputs: vec![PortDefinition {
                id: "output".to_string(),
                label: "Out".to_string(),
                position: PortPosition::Right,
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
        let persist = input.input["persist"].as_bool().unwrap_or(false);

        // Check in-run execution context — always, regardless of persist flag
        if let Some(v) = input.context.variables.get(&key) {
            return NodeOutput::success_with_logs(
                json!({ "key": key, "value": v, "found": true }),
                vec![format!("Got '{}' = {}", key, v)],
            );
        }

        if persist {
            match self.db {
                None => {
                    // serve mode: warn, fall through to default
                }
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
                                vec![format!("Got '{}' from DB = {}", key, db_val)],
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
                    "Variable '{}' not found in this run — using configured default: {}",
                    key, d
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
