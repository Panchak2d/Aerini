//! `ApiState` shared across all route handlers.

use flowo_engine::{
    db::WorkflowDb,
    executor::WorkflowExecutor,
    node::NodeRegistry,
    scheduler::SchedulerDaemon,
    store::CredentialStore,
};
use dashmap::DashMap;
use tokio::sync::{broadcast, Semaphore};
use std::sync::Arc;

use crate::token_store::{TokenRecord, TokenStore};

/// Maximum concurrent SSE connections across all tokens.
pub const SSE_MAX_CONNECTIONS: usize = 64;

#[derive(Clone)]
pub struct ApiState {
    pub db:               Arc<WorkflowDb>,
    pub scheduler:        Arc<SchedulerDaemon>,
    pub creds:            Arc<CredentialStore>,
    pub registry:         Arc<NodeRegistry>,
    pub sse_tx:           broadcast::Sender<String>,
    pub token_store:      Arc<TokenStore>,
    pub exec_locks:       Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pub env_allowlist:    Option<Arc<std::collections::HashSet<String>>>,
    pub file_sandbox_dir: Option<Arc<std::path::PathBuf>>,
    pub shell_exec_disabled:  bool,
    pub code_exec_disabled:   bool,
    pub parallel_execution:   bool,
    pub max_concurrent_nodes: usize,
    pub server_max_duration_secs: Option<u64>,
    pub base_executor: WorkflowExecutor,
    pub sse_semaphore: Arc<Semaphore>,
}

pub fn require_admin(record: &TokenRecord) -> Result<(), (axum::http::StatusCode, axum::Json<serde_json::Value>)> {
    if record.has_scope("admin") {
        Ok(())
    } else {
        Err((
            axum::http::StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({"error":"admin scope required for token management"})),
        ))
    }
}

pub fn require_read(record: &TokenRecord) -> Result<(), (axum::http::StatusCode, axum::Json<serde_json::Value>)> {
    if record.has_scope("read") {
        Ok(())
    } else {
        Err((axum::http::StatusCode::FORBIDDEN, axum::Json(serde_json::json!({"error":"read scope required"}))))
    }
}

pub fn require_write(record: &TokenRecord) -> Result<(), (axum::http::StatusCode, axum::Json<serde_json::Value>)> {
    if record.has_scope("write") {
        Ok(())
    } else {
        Err((axum::http::StatusCode::FORBIDDEN, axum::Json(serde_json::json!({"error":"write scope required"}))))
    }
}
