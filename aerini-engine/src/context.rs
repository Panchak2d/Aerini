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
    // Plain HashMap, not DashMap: every mutation path here takes `&mut self`,
    // reachable only through SharedExecutionState's outer `Arc<RwLock<...>>`
    // write lock — access is already fully serialized before it ever reaches
    // this field, so DashMap's fine-grained bucket locking is never actually
    // exercised. It previously cost real allocation/indirection overhead per
    // entry for concurrency this field can't observe (S5-2).
    node_outputs: HashMap<String, Value>,
    // Cached clone of `node_outputs`, rebuilt once inside mark_succeeded (the
    // only site that mutates node_outputs) rather than on every snapshot()
    // call. snapshot() is called under a shared *read* lock (build_input:
    // `state.read().await`), so in parallel mode several nodes can become
    // ready and each call snapshot() before the next mark_succeeded — every
    // one of those used to independently deep-clone the same, unchanged map,
    // expensive when node outputs embed large payloads (files, images, HTTP
    // bodies). Caching turns every snapshot() call after the first such call
    // into an O(1) Arc clone instead of an O(n) deep clone of the map
    // contents. Sequential mode is unaffected either way (already ~1
    // snapshot() per mark_succeeded, so no reduction, but no regression).
    node_outputs_snapshot: Arc<HashMap<String, Value>>,
    node_statuses: HashMap<String, NodeExecution>,
    pub variables: HashMap<String, Value>,
    // Loop-internal state: __loop_*_index keys.
    // Kept separate from `variables` so user-defined variables cannot collide
    // with or observe loop internals.
    loop_state: HashMap<String, Value>,
    // Per-loop accumulated body results, stored outside loop_state so they are
    // NOT included in snapshot().metadata. snapshot() clones loop_state on every
    // node execution — storing O(n) per-iteration results there caused O(k·n²)
    // clone cost across an n-iteration loop with k body nodes. The executor reads
    // these directly via take_loop_results() and injects them into the done output.
    loop_results: HashMap<String, Vec<Value>>,
    pub logs: Vec<ExecutionLogEntry>,
    // Node ids in completion order — root cause fix for the S3-2/S3-3/S4-6/S4-7
    // family. Mutated only via mark_succeeded's move-to-end logic (see below)
    // so a loop body node re-completing every iteration cannot grow this
    // unboundedly — bounded by unique node count, not iteration count.
    // T2-2 (this field) landed in Batch J; T2-5 (Batch K) has since migrated
    // the five consumer nodes (output_node.rs, json_node.rs, text_splitter.rs,
    // merge.rs, loop_node.rs) onto it via nodes/util.rs::ordered_node_outputs.
    execution_order: Vec<String>,
}

impl ExecutionState {
    pub fn new(workflow_id: impl Into<String>, variables: HashMap<String, Value>) -> Self {
        Self {
            execution_id: Uuid::new_v4().to_string(),
            workflow_id: workflow_id.into(),
            started_at: Utc::now(),
            node_outputs: HashMap::new(),
            node_outputs_snapshot: Arc::new(HashMap::new()),
            node_statuses: HashMap::new(),
            variables,
            loop_state: HashMap::new(),
            loop_results: HashMap::new(),
            logs: Vec::new(),
            execution_order: Vec::new(),
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
        self.node_outputs.insert(node_id.to_string(), value);
        // Rebuild the cached snapshot here (the one place node_outputs
        // mutates), not in snapshot() itself — see node_outputs_snapshot's
        // doc comment above.
        self.node_outputs_snapshot = Arc::new(self.node_outputs.clone());
        // Move-to-end, not append: a loop body node calls mark_succeeded once per
        // iteration (see executor/loop_executor.rs:337), not once per execution —
        // an unconditional push would grow execution_order without bound across
        // iterations and reproduce the exact O(k·n²) snapshot-clone cost the
        // loop_results field above was specifically designed to avoid. Removing
        // any prior entry before re-pushing keeps length bounded by unique node
        // count and keeps `.last()` correctly naming the most recently completed
        // node, including across loop iterations.
        if let Some(pos) = self.execution_order.iter().position(|id| id == node_id) {
            self.execution_order.remove(pos);
        }
        self.execution_order.push(node_id.to_string());
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
    pub fn get_node_output(&self, node_id: &str) -> Option<Value> {
        self.node_outputs.get(node_id).cloned()
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

    /// Append one body-node result for the given loop node.
    /// Stored in a dedicated Vec outside loop_state so it is never included in
    /// snapshot().metadata — avoiding the O(k·n²) clone cost that results from
    /// snapshot() deep-cloning loop_state on every node execution in every iteration.
    pub fn push_loop_result(&mut self, loop_id: &str, val: Value) {
        self.loop_results.entry(loop_id.to_string()).or_default().push(val);
    }

    /// Consume and return all accumulated body results for a loop node.
    /// Called once by the executor on the "done" iteration to build all_results.
    pub fn take_loop_results(&mut self, loop_id: &str) -> Vec<Value> {
        self.loop_results.remove(loop_id).unwrap_or_default()
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
        // the iteration index without it appearing in `variables`.
        for (k, v) in &self.loop_state {
            meta.insert(k.clone(), v.clone());
        }
        ExecutionContext {
            variables: self.variables.clone(),
            // O(1) Arc clone — node_outputs_snapshot is rebuilt in mark_succeeded,
            // not here. See that field's doc comment.
            node_outputs: self.node_outputs_snapshot.clone(),
            metadata: meta,
            execution_order: Arc::new(self.execution_order.clone()),
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

#[cfg(test)]
mod execution_order_tests {
    use super::*;
    use crate::model::NodeOutput;
    use serde_json::json;

    #[test]
    fn records_completion_order_for_distinct_nodes() {
        let mut s = ExecutionState::new("wf", HashMap::new());
        s.mark_succeeded("a", NodeOutput::success(json!(1)));
        s.mark_succeeded("b", NodeOutput::success(json!(2)));
        s.mark_succeeded("c", NodeOutput::success(json!(3)));
        assert_eq!(s.execution_order, vec!["a", "b", "c"]);
    }

    #[test]
    fn snapshot_exposes_execution_order() {
        let mut s = ExecutionState::new("wf", HashMap::new());
        s.mark_succeeded("a", NodeOutput::success(json!(1)));
        s.mark_succeeded("b", NodeOutput::success(json!(2)));
        let ctx = s.snapshot();
        assert_eq!(*ctx.execution_order, vec!["a".to_string(), "b".to_string()]);
    }

    /// Loop-body node re-completing across iterations must move to the end,
    /// not append a duplicate — this is the exact call pattern
    /// executor/loop_executor.rs:337 produces (one mark_succeeded per iteration
    /// for the same body_id).
    #[test]
    fn reexecuting_node_moves_to_end_not_duplicated() {
        let mut s = ExecutionState::new("wf", HashMap::new());
        s.mark_succeeded("a", NodeOutput::success(json!(1)));
        s.mark_succeeded("body", NodeOutput::success(json!("iter0")));
        s.mark_succeeded("z", NodeOutput::success(json!(9)));
        s.mark_succeeded("body", NodeOutput::success(json!("iter1")));
        assert_eq!(s.execution_order, vec!["a", "z", "body"]);
        assert_eq!(s.execution_order.len(), 3);
    }

    /// Bounded-growth guarantee: an n-iteration loop body must not grow
    /// execution_order past the unique node count, regardless of n — this is
    /// the specific regression this batch's design decision (move-to-end,
    /// not append) exists to prevent.
    #[test]
    fn many_reexecutions_of_same_node_stay_bounded() {
        let mut s = ExecutionState::new("wf", HashMap::new());
        for i in 0..500 {
            s.mark_succeeded("body", NodeOutput::success(json!(i)));
        }
        assert_eq!(s.execution_order.len(), 1);
        assert_eq!(s.execution_order, vec!["body"]);
    }

    #[test]
    fn last_reflects_most_recently_completed_node_across_reexecution() {
        let mut s = ExecutionState::new("wf", HashMap::new());
        s.mark_succeeded("output_node", NodeOutput::success(json!("first")));
        s.mark_succeeded("body", NodeOutput::success(json!("iter0")));
        s.mark_succeeded("body", NodeOutput::success(json!("iter1")));
        assert_eq!(s.execution_order.last().map(String::as_str), Some("body"));
    }

    // ── node_outputs snapshot caching (memory-efficiency batch) ─────────────

    #[test]
    fn repeated_snapshot_calls_reuse_same_arc_when_unmutated() {
        let mut s = ExecutionState::new("wf", HashMap::new());
        s.mark_succeeded("a", NodeOutput::success(json!(1)));
        let first = s.snapshot().node_outputs;
        let second = s.snapshot().node_outputs;
        assert!(
            Arc::ptr_eq(&first, &second),
            "snapshot() must reuse the cached Arc when node_outputs hasn't changed"
        );
    }

    #[test]
    fn snapshot_arc_changes_after_mark_succeeded() {
        let mut s = ExecutionState::new("wf", HashMap::new());
        s.mark_succeeded("a", NodeOutput::success(json!(1)));
        let before = s.snapshot().node_outputs;
        s.mark_succeeded("b", NodeOutput::success(json!(2)));
        let after = s.snapshot().node_outputs;
        assert!(
            !Arc::ptr_eq(&before, &after),
            "snapshot() must rebuild the cached Arc once node_outputs changes"
        );
        assert_eq!(after.len(), 2);
        assert_eq!(before.len(), 1);
    }

    #[test]
    fn snapshot_node_outputs_content_matches_mark_succeeded_calls() {
        let mut s = ExecutionState::new("wf", HashMap::new());
        s.mark_succeeded("a", NodeOutput::success(json!({"x": 1})));
        s.mark_succeeded("b", NodeOutput::success(json!({"y": 2})));
        let ctx = s.snapshot();
        assert_eq!(ctx.node_outputs.get("a"), Some(&json!({"x": 1})));
        assert_eq!(ctx.node_outputs.get("b"), Some(&json!({"y": 2})));
    }
}
