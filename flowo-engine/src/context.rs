//! Per-execution mutable state, shared across async node tasks.
//!
//! [`ExecutionState`] holds everything that changes during a single workflow run:
//! node outputs, node statuses, accumulated log entries, and workflow variables.
//!
//! [`SharedExecutionState`] wraps `ExecutionState` in `Arc<RwLock<...>>` so the executor
//! and event-emitting tasks can access it concurrently without data races.
//!
//! # `#[allow(dead_code)]` items
//!
//! Several items in this module are marked `#[allow(dead_code)]` intentionally:
//! `NodeStatus`, `ExecutionState`, and the helper methods `get_node_output`,
//! `node_succeeded`, `node_status`. These are part of the public inspection API used
//! by tooling and tests — zero direct call sites in the executor is correct.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::model::{ExecutionContext, NodeOutput};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum NodeStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeExecution {
    pub node_id: String,
    pub status: NodeStatus,
    pub attempts: u32,
    pub output: Option<NodeOutput>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionLogEntry {
    pub timestamp: DateTime<Utc>,
    pub node_id: Option<String>,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct ExecutionState {
    pub execution_id: String,
    pub workflow_id: String,
    pub started_at: DateTime<Utc>,
    node_outputs: Arc<HashMap<String, Value>>,
    node_statuses: HashMap<String, NodeExecution>,
    pub variables: HashMap<String, Value>,
    // Loop-internal state: __loop_*_index and __loop_*_result_* keys.
    // Kept separate from `variables` so user-defined variables cannot collide
    // with or observe loop internals.
    loop_state: HashMap<String, Value>,
    pub logs: Vec<ExecutionLogEntry>,
}

impl ExecutionState {
    pub fn new(workflow_id: impl Into<String>, variables: HashMap<String, Value>) -> Self {
        Self {
            execution_id: Uuid::new_v4().to_string(),
            workflow_id: workflow_id.into(),
            started_at: Utc::now(),
            node_outputs: Arc::new(HashMap::new()),
            node_statuses: HashMap::new(),
            variables,
            loop_state: HashMap::new(),
            logs: Vec::new(),
        }
    }

    pub fn mark_running(&mut self, node_id: &str) {
        self.node_statuses.insert(node_id.to_string(), NodeExecution {
            node_id: node_id.to_string(),
            status: NodeStatus::Running,
            attempts: 0,
            output: None,
            started_at: Some(Utc::now()),
            finished_at: None,
        });
        self.log(None, LogLevel::Info, format!("Node '{}' started", node_id));
    }

    pub fn mark_succeeded(&mut self, node_id: &str, output: NodeOutput) {
        let value = output.output.clone().unwrap_or(serde_json::Value::Null);
        // CoW: if no other Arc references exist (common case), mutates in place — O(1).
        // If a snapshot is still alive, clones the map before inserting — O(n).
        Arc::make_mut(&mut self.node_outputs).insert(node_id.to_string(), value);
        if let Some(r) = self.node_statuses.get_mut(node_id) {
            r.status = NodeStatus::Succeeded;
            r.output = Some(output);
            r.finished_at = Some(Utc::now());
        }
        self.log(None, LogLevel::Info, format!("Node '{}' succeeded", node_id));
    }

    pub fn mark_failed(&mut self, node_id: &str, output: NodeOutput, attempts: u32) {
        if let Some(r) = self.node_statuses.get_mut(node_id) {
            r.status = NodeStatus::Failed;
            r.output = Some(output.clone());
            r.finished_at = Some(Utc::now());
            r.attempts = attempts;
        }
        let msg = output.error.as_ref().map(|e| e.message.clone()).unwrap_or_default();
        self.log(None, LogLevel::Error, format!("Node '{}' failed: {}", node_id, msg));
    }

    pub fn mark_skipped(&mut self, node_id: &str) {
        self.node_statuses.insert(node_id.to_string(), NodeExecution {
            node_id: node_id.to_string(),
            status: NodeStatus::Skipped,
            attempts: 0,
            output: None,
            started_at: None,
            finished_at: None,
        });
        self.log(None, LogLevel::Info, format!("Node '{}' skipped", node_id));
    }

    #[allow(dead_code)]
    pub fn get_node_output(&self, node_id: &str) -> Option<&Value> {
        self.node_outputs.get(node_id)
    }

    #[allow(dead_code)]
    pub fn node_succeeded(&self, node_id: &str) -> bool {
        self.node_statuses.get(node_id)
            .map(|r| r.status == NodeStatus::Succeeded)
            .unwrap_or(false)
    }

    #[allow(dead_code)]
    pub fn node_status(&self, node_id: &str) -> Option<&NodeStatus> {
        self.node_statuses.get(node_id).map(|r| &r.status)
    }

    pub fn set_variable(&mut self, key: impl Into<String>, value: Value) {
        self.variables.insert(key.into(), value);
    }

    /// Write a loop-internal variable (e.g. `__loop_{id}_index`).
    /// These keys are stored in a private map and exposed only through
    /// `snapshot().metadata`, not through `snapshot().variables`, so
    /// user-defined variables cannot collide with loop state.
    pub fn set_loop_var(&mut self, key: impl Into<String>, value: Value) {
        self.loop_state.insert(key.into(), value);
    }

    pub fn log(&mut self, node_id: Option<&str>, level: LogLevel, message: String) {
        self.logs.push(ExecutionLogEntry {
            timestamp: Utc::now(),
            node_id: node_id.map(|s| s.to_string()),
            level,
            message,
        });
    }

    pub fn snapshot(&self) -> ExecutionContext {
        let mut meta = HashMap::new();
        meta.insert("execution_id".to_string(), Value::String(self.execution_id.clone()));
        meta.insert("workflow_id".to_string(), Value::String(self.workflow_id.clone()));
        // Expose loop-internal state through metadata so loop_node.rs can read
        // iteration index and body results without them appearing in `variables`.
        for (k, v) in &self.loop_state {
            meta.insert(k.clone(), v.clone());
        }
        ExecutionContext {
            variables: self.variables.clone(),
            node_outputs: Arc::clone(&self.node_outputs),
            metadata: meta,
        }
    }
}

pub type SharedExecutionState = Arc<RwLock<ExecutionState>>;

pub fn new_shared_state(
    workflow_id: impl Into<String>,
    variables: HashMap<String, Value>,
) -> SharedExecutionState {
    Arc::new(RwLock::new(ExecutionState::new(workflow_id, variables)))
}
