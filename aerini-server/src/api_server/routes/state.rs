//! `ApiState` shared across all route handlers.

use aerini_engine::{
    db::WorkflowDb,
    executor::WorkflowExecutor,
    node::{NodeRegistry, Reloadable},
    plugin_loader::PluginLoadReport,
    scheduler::SchedulerDaemon,
    store::CredentialStore,
};
use dashmap::DashMap;
use tokio::sync::{broadcast, Semaphore};
use std::{sync::Arc, time::Duration};

use crate::token_store::{TokenRecord, TokenStore};

/// Maximum concurrent SSE connections across all tokens.
pub const SSE_MAX_CONNECTIONS: usize = 64;

#[allow(dead_code)]
#[derive(Clone)]
pub struct ApiState {
    pub db:               Arc<WorkflowDb>,
    pub scheduler:        Arc<SchedulerDaemon>,
    pub creds:            Arc<CredentialStore>,
    pub registry:         Arc<Reloadable<NodeRegistry>>,
    /// Held for the full scan+compile+swap sequence of a `/plugins/reload`
    /// request, so two reloads that finish out of order (e.g. a slower
    /// install started before a faster one) can't have the earlier-started,
    /// later-finishing one clobber the newer registry with stale data. The
    /// swap inside `Reloadable::reload` itself is already atomic; this lock
    /// orders *whole reloads* against each other, which the swap alone
    /// doesn't guarantee.
    pub reload_lock:      Arc<tokio::sync::Mutex<()>>,
    /// Snapshot of the most recent plugin load — startup, or the last
    /// `/api/plugins/reload` call, whichever happened later.
    pub last_load_report: Arc<tokio::sync::RwLock<PluginLoadReport>>,
    pub data_dir:         Arc<std::path::PathBuf>,
    pub plugin_dir:       Option<Arc<std::path::PathBuf>>,
    pub sse_tx:           broadcast::Sender<String>,
    pub token_store:      Arc<TokenStore>,
    pub exec_locks:       Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pub env_allowlist:    Option<Arc<std::collections::HashSet<String>>>,
    pub file_sandbox_dir: Option<Arc<std::path::PathBuf>>,
    pub shell_exec_disabled:  bool,
    pub code_exec_disabled:   bool,
    pub database_exec_disabled: bool,
    pub parallel_execution:   bool,
    pub max_concurrent_nodes: usize,
    pub server_max_duration_secs: Option<u64>,
    pub base_executor: WorkflowExecutor,
    pub sse_semaphore: Arc<Semaphore>,
    pub run_semaphore: Arc<Semaphore>,
    pub queue_timeout: Duration,
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
