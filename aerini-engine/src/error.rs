//! Engine error types.
//!
//! [`EngineError`] covers all failures that originate in the engine: invalid workflow
//! structure, graph errors, DB failures, and encryption errors. It derives `thiserror::Error`
//! and implements `From<EngineError> for String` so it can be returned from Tauri commands
//! (which require `String`-serialisable errors) without modification.
//!
//! [`NodeError`] is a plain struct (not `thiserror`) used by node implementations to
//! return execution failures. It carries an optional node ID so the executor can attach
//! context when logging. Nodes return `NodeOutput::error(NodeError { ... })` rather than
//! propagating `NodeError` via `?`.
//!
//! # Frozen variants
//!
//! All `EngineError` variants and their `#[error]`/`#[from]` annotations are frozen —
//! they are part of the error surface consumed by the Tauri commands. Adding variants
//! is fine; removing or renaming breaks callers.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("Workflow JSON is invalid: {0}")]
    InvalidWorkflowJson(#[from] serde_json::Error),

    #[error("Workflow schema violation: {0}")]
    SchemaViolation(String),

    #[error("Cycle detected involving node '{0}'")]
    CycleDetected(String),

    #[error("Node '{0}' referenced in edge does not exist")]
    UnknownNodeReference(String),

    #[error("Workflow has no entry nodes")]
    NoEntryNodes,

    #[error("Node '{0}' is unreachable from any entry node")]
    UnreachableNode(String),

    #[error("Node '{node_id}' input failed schema validation: {reason}")]
    InputValidationFailed { node_id: String, reason: String },

    #[error("Node '{node_id}' output failed schema validation: {reason}")]
    OutputValidationFailed { node_id: String, reason: String },

    #[error("Node '{node_id}' failed after {attempts} attempt(s): {reason}")]
    NodeExecutionFailed { node_id: String, attempts: u32, reason: String },

    #[error("Node type '{type_id}' is not registered")]
    NodeTypeNotRegistered { type_id: String },

    #[error("Credential '{0}' not found")]
    CredentialNotFound(String),

    #[error("Execution cancelled")]
    ExecutionCancelled,

    #[error("Workflow '{workflow_id}' exceeded maximum execution time of {limit_secs}s")]
    WorkflowTimeout { workflow_id: String, limit_secs: u64 },

    #[error("Database error: {0}")]
    Database(String),

    #[error("Encryption error: {0}")]
    Encryption(String),

    #[error("Internal executor error: {0}")]
    Internal(String),
}

/// Structured error returned by a node on failure.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeError {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

impl NodeError {
    pub fn recoverable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into(), recoverable: true }
    }
    pub fn unrecoverable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into(), recoverable: false }
    }
}

/// Tauri commands need String errors — this converts engine errors for IPC.
impl From<EngineError> for String {
    fn from(e: EngineError) -> String {
        e.to_string()
    }
}
