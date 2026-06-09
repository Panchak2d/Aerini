//! Core data model — serde types shared across the engine, Tauri shell, and server binary.
//!
//! All field names in these types are part of the **wire contract** between Rust and the
//! TypeScript frontend. Do not rename fields without a coordinated frontend update.
//!
//! # Key types
//!
//! - [`Workflow`] — top-level document stored in SQLite and exchanged over IPC.
//! - [`WorkflowNode`] — a single node on the canvas; holds `params` as freeform JSON.
//! - [`WorkflowEdge`] — a directed connection between two nodes; may carry `condition`,
//!   `on_success`, and `on_failure` routing targets.
//! - [`ExecutionContext`] — read-only snapshot passed to nodes at execute time.
//! - [`RetryPolicy`] — per-node retry configuration; default is 1 attempt (no retry).
//!
//! # RetryPolicy default
//!
//! The default is intentionally 1 attempt (no automatic retry). An earlier default of 3
//! caused nodes with side effects (email, HTTP POST) to silently execute 3× on failure.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

// ── Node type category ────────────────────────────────────────────────────────

/// Palette category for a node type.
///
/// Controls which section of the node picker the node appears in.
/// Has no effect on execution behavior — the executor treats all categories identically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    Action,
    Ai,
    Logic,
    Utility,
}

// ── Retry policy ──────────────────────────────────────────────────────────────

/// Per-node retry configuration.
///
/// The executor re-attempts a node that returns a *recoverable* failure up to
/// `max_attempts` times. Nodes returning an unrecoverable error are never retried
/// regardless of this setting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Total attempts allowed, including the first. `1` means no automatic retry.
    pub max_attempts: u32,
    /// Fixed wait between attempts in milliseconds.
    pub backoff_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        // 1 attempt = no automatic retry. Users opt into retries via the Retry UI.
        // Default of 3 caused all nodes to silently retry 3× on failure, including
        // nodes with side effects (email, HTTP POST). See BUG-01 audit finding.
        Self { max_attempts: 1, backoff_ms: 500 }
    }
}

// ── Canvas position (stored with node, ignored by executor) ──────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CanvasPosition {
    pub x: f64,
    pub y: f64,
}

// ── Workflow node ─────────────────────────────────────────────────────────────

/// A single node instance on the workflow canvas.
///
/// `node_type_id` determines which registered [`crate::node::Node`] implementation is
/// invoked. `config` holds the user-provided parameter values as freeform JSON;
/// the node's `execute()` method deserializes the fields it needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    /// Unique ID within this workflow (e.g. "node_abc123").
    pub id: String,

    /// Which registered node IMPLEMENTATION to use (e.g. "http_request").
    /// FIXES the Phase 1 limitation — multiple nodes can share the same type.
    pub node_type_id: String,

    /// UI category for display.
    pub node_type: NodeType,

    /// Human label shown on the canvas block.
    pub name: String,

    /// Static config for this node instance (filled in via the config panel).
    #[serde(default)]
    pub config: Value,

    /// Named credential references (e.g. { "api_key": "openai_prod" }).
    /// Values are credential IDs — the actual secrets are looked up at runtime.
    #[serde(default)]
    pub credentials: HashMap<String, String>,

    /// JSON Schema the node's input must match.
    pub input_schema: Value,

    /// JSON Schema the node's output must match.
    pub output_schema: Value,

    #[serde(default)]
    pub retry: RetryPolicy,

    /// Reserved for future use — not read by the executor in the current version.
    #[serde(default)]
    pub fallback_node: Option<String>,

    /// When true, the executor skips this node and activates its successors
    /// as if it ran successfully with empty output. Existing workflows without
    /// this field deserialize with disabled = false (fully backwards compatible).
    #[serde(default)]
    pub disabled: bool,

    /// Visual position on the canvas — not used by the executor.
    #[serde(default)]
    pub position: CanvasPosition,
}

// ── Workflow edge ─────────────────────────────────────────────────────────────

/// A directed connection between two nodes on the workflow canvas.
///
/// The executor uses `from_port` to determine which edge to follow after a node
/// produces output. Multi-output nodes (e.g. `if_condition`, `switch`) write a
/// `branch` key to their output; the executor matches that value against `from_port`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEdge {
    /// Unique ID for this edge (used by canvas to track connectors).
    pub id: String,
    pub from_node: String,
    /// Output port name on the source node (e.g. "output", "on_true").
    pub from_port: String,
    pub to_node: String,
    /// Input port name on the target node (e.g. "input").
    pub to_port: String,
    /// Optional expression evaluated by the frontend canvas to control edge visibility.
    /// Not evaluated by the executor — routing is determined by `from_port` alone.
    #[serde(default)]
    pub condition: Option<String>,
    /// Alternate target node activated when the source node succeeds.
    /// Used by the graph builder to register additional reachability edges.
    #[serde(default)]
    pub on_success: Option<String>,
    /// Target node activated when the source node fails after all retry attempts.
    /// When set, the executor routes the failure here instead of aborting the run.
    #[serde(default)]
    pub on_failure: Option<String>,
}

// ── Workflow metadata ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowMetadata {
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Default for WorkflowMetadata {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            author: "user".to_string(),
            created_at: now,
            updated_at: now,
            version: "1.0.0".to_string(),
            tags: vec![],
        }
    }
}

// ── Root workflow document ────────────────────────────────────────────────────

use crate::migration::CURRENT_VERSION;

fn default_schema_version() -> String { CURRENT_VERSION.to_string() }

/// The root workflow document.
///
/// Stored as JSON in SQLite and exchanged over Tauri IPC and the server API.
/// Use [`Workflow::from_json`] to deserialize — it applies any pending schema
/// migrations before deserializing. Field names are part of the wire contract
/// with the TypeScript frontend; do not rename without a coordinated frontend update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    #[serde(default)]
    pub metadata: WorkflowMetadata,
    /// Maximum wall-clock time for the entire workflow execution.
    /// `None` = no limit (backward-compatible default).
    /// The executor clamps values to [10, 86400] seconds.
    #[serde(default)]
    pub max_duration_secs: Option<u64>,
    /// When true, independent branches run concurrently via the parallel executor.
    /// Default: false — existing workflows are unaffected (sequential-safe default).
    /// Enabling this for workflows with ordered side-effects (Stripe → Slack → Email)
    /// may cause non-deterministic execution order; opt in only where branches are
    /// truly independent.
    #[serde(default)]
    pub parallel_execution: bool,
    /// Maximum concurrent node tasks when `parallel_execution` is true.
    /// `None` = use executor default (8). Reduce if hitting external rate limits.
    #[serde(default)]
    pub max_concurrent_nodes: Option<usize>,
}

impl Workflow {
    #[allow(dead_code)]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema_version: CURRENT_VERSION.to_string(),
            id: id.into(),
            name: name.into(),
            description: String::new(),
            nodes: vec![],
            edges: vec![],
            metadata: WorkflowMetadata::default(),
            max_duration_secs: None,
            parallel_execution: false,
            max_concurrent_nodes: None,
        }
    }

    /// Parse a workflow from JSON, applying any pending schema migrations first.
    ///
    /// The raw JSON is parsed into a `serde_json::Value`, then
    /// [`crate::migration::MigrationEngine`] upgrades the data to the current
    /// schema version in-place, then the result is deserialized into `Workflow`.
    ///
    /// For current-version workflows (`schema_version == "1.0"`) this is a
    /// transparent no-op — no extra cost beyond the normal deserialization.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let mut raw: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| format!("JSON parse error: {}", e))?;
        crate::migration::MigrationEngine::new()
            .apply(&mut raw)
            .map_err(|e| format!("Schema migration error: {}", e))?;
        serde_json::from_value(raw)
            .map_err(|e| format!("Workflow deserialization error: {}", e))
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn node(&self, id: &str) -> Option<&WorkflowNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

// ── Runtime models (not stored in JSON) ──────────────────────────────────────

/// The fully resolved input passed to [`crate::node::Node::execute`].
///
/// Built by the executor immediately before calling `execute()`. The `input` field
/// contains the node's config values with all `{{...}}` expressions evaluated and
/// credential values already substituted — the node sees plaintext secrets directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInput {
    pub node_id: String,
    pub workflow_id: String,
    pub execution_id: String,
    /// Merged config + resolved credential values.
    pub input: Value,
    pub context: ExecutionContext,
}

/// Read-only snapshot of workflow execution state at the moment a node is about to run.
///
/// Passed inside [`NodeInput`]. Nodes use this to read outputs from upstream nodes
/// (via `node_outputs`) and workflow-level variables (via `variables`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub variables: HashMap<String, Value>,
    /// Arc-wrapped so snapshot() in the executor is O(1) (refcount bump only).
    /// Serializes identically to HashMap<String, Value> — no wire-format change.
    pub node_outputs: Arc<HashMap<String, Value>>,
    pub metadata: HashMap<String, Value>,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            variables:    HashMap::new(),
            node_outputs: Arc::new(HashMap::new()),
            metadata:     HashMap::new(),
        }
    }
}

/// The result returned by [`crate::node::Node::execute`].
///
/// Use the constructors ([`NodeOutput::success`], [`NodeOutput::failure`], etc.)
/// rather than constructing this struct directly. When `success` is `true`, the
/// `output` value is stored in the execution context and made available to downstream
/// nodes via `{{$nodes.node_id.field}}` expressions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeOutput {
    pub success: bool,
    pub output: Option<Value>,
    pub error: Option<crate::error::NodeError>,
    #[serde(default)]
    pub logs: Vec<String>,
}

impl NodeOutput {
    pub fn success(output: Value) -> Self {
        Self { success: true, output: Some(output), error: None, logs: vec![] }
    }
    /// Construct a success result and attach additional log lines to the run history.
    ///
    /// Use instead of [`NodeOutput::success`] when the node has diagnostic output
    /// worth preserving on the happy path (HTTP response headers, row counts, etc.).
    pub fn success_with_logs(output: Value, logs: Vec<String>) -> Self {
        Self { success: true, output: Some(output), error: None, logs }
    }
    pub fn failure(error: crate::error::NodeError) -> Self {
        Self { success: false, output: None, error: Some(error), logs: vec![] }
    }
    /// Construct a failure result and attach additional log lines to the run history.
    ///
    /// Use instead of [`NodeOutput::failure`] when the node captured stderr or other
    /// diagnostics before the error occurred.
    pub fn failure_with_logs(error: crate::error::NodeError, logs: Vec<String>) -> Self {
        Self { success: false, output: None, error: Some(error), logs }
    }
}
