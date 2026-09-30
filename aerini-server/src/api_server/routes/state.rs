//! `ApiState` shared across all route handlers.

use aerini_engine::{
    db::WorkflowDb,
    executor::WorkflowExecutor,
    node::{NodeRegistry, Reloadable},
    plugin_loader::PluginLoadReport,
    scheduler::SchedulerDaemon,
    store::CredentialStore,
};
use axum::{http::StatusCode, Json};
use dashmap::DashMap;
use tokio::sync::{broadcast, Semaphore};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

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
    /// Cancelled when shutdown begins; long-lived streams end on it.
    pub shutdown:         CancellationToken,
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

pub type ApiError = (StatusCode, Json<serde_json::Value>);

impl ApiState {
    /// Verifies a bearer token on the blocking pool: the lookup is a locked
    /// SQLite read. An empty token can never match a stored hash, so it is
    /// rejected without touching the database. A failed task denies access.
    pub async fn verify_token(&self, raw: &str) -> Option<TokenRecord> {
        if raw.is_empty() {
            return None;
        }
        let store = Arc::clone(&self.token_store);
        let raw = raw.to_owned();
        deny_on_join_error(tokio::task::spawn_blocking(move || store.verify_token(&raw)).await)
    }

    /// Runs a token-store operation on the blocking pool. Store errors and
    /// task failures are both returned as their display text.
    pub async fn with_token_store<T, F>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&TokenStore) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let store = Arc::clone(&self.token_store);
        tokio::task::spawn_blocking(move || f(store.as_ref()).map_err(|e| e.to_string()))
            .await
            .map_err(|e| e.to_string())?
    }

    /// `None` = the caller may see every workflow; `Some(set)` = only those.
    /// See [`TokenStore::acl_filter`].
    pub async fn acl_filter(&self, caller: &TokenRecord) -> Result<Option<HashSet<String>>, String> {
        let record = caller.clone();
        self.with_token_store(move |store| store.acl_filter(&record)).await
    }
}

fn deny_on_join_error(joined: Result<Option<TokenRecord>, tokio::task::JoinError>) -> Option<TokenRecord> {
    joined.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "token verification task failed, denying access");
        None
    })
}

pub fn acl_allows(filter: &Option<HashSet<String>>, workflow_id: &str) -> bool {
    match filter {
        None          => true,
        Some(allowed) => allowed.contains(workflow_id),
    }
}

fn acl_lookup_failed(e: &str) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": format!("ACL lookup failed: {}", e)})),
    )
}

/// Whether `caller` may act on `workflow_id`: admin and tokens with no grants
/// always may; a token with grants may only for a granted workflow.
pub async fn acl_permits(s: &ApiState, caller: &TokenRecord, workflow_id: &str) -> Result<bool, String> {
    s.acl_filter(caller).await.map(|filter| acl_allows(&filter, workflow_id))
}

/// Pre-check for routes addressed by workflow id. The answer depends only on
/// the caller's own grants, never on whether the workflow exists, so a `403`
/// reveals nothing about other workflows.
pub async fn require_workflow_acl(
    s:           &ApiState,
    caller:      &TokenRecord,
    workflow_id: &str,
) -> Result<(), ApiError> {
    match acl_permits(s, caller, workflow_id).await {
        Ok(true)  => Ok(()),
        Ok(false) => Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "token ACL does not permit access to this workflow"})),
        )),
        Err(e) => Err(acl_lookup_failed(&e)),
    }
}

/// For server-wide routes that belong to no single workflow: a token
/// restricted to specific workflows may not use them.
pub async fn require_unrestricted(s: &ApiState, caller: &TokenRecord) -> Result<(), ApiError> {
    match s.acl_filter(caller).await {
        Ok(None)    => Ok(()),
        Ok(Some(_)) => Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "token is restricted to specific workflows and cannot use server-wide routes"})),
        )),
        Err(e) => Err(acl_lookup_failed(&e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> TokenRecord {
        TokenRecord {
            token_id:   "t".to_string(),
            label:      "l".to_string(),
            scopes:     vec!["read".to_string()],
            created_at: "c".to_string(),
            expires_at: None,
        }
    }

    #[test]
    fn unrestricted_filter_allows_any_workflow() {
        assert!(acl_allows(&None, "wf_a"));
    }

    #[test]
    fn restricted_filter_allows_only_granted_workflows() {
        let filter = Some(HashSet::from(["wf_a".to_string()]));
        assert!(acl_allows(&filter, "wf_a"));
        assert!(!acl_allows(&filter, "wf_b"));
    }

    #[tokio::test]
    async fn verification_task_panic_denies_access() {
        let joined = tokio::task::spawn_blocking(|| -> Option<TokenRecord> {
            panic!("simulated verification panic")
        })
        .await;
        assert!(joined.is_err());
        assert!(deny_on_join_error(joined).is_none());
    }

    #[tokio::test]
    async fn successful_verification_result_is_returned_unchanged() {
        let joined = tokio::task::spawn_blocking(|| Some(record())).await;
        assert_eq!(deny_on_join_error(joined).map(|r| r.token_id), Some("t".to_string()));
    }
}
