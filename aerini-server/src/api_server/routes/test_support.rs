//! Shared fixtures for route-handler tests: a real `ApiState` over temp-dir
//! stores, and a router that puts the production `auth_middleware` in front
//! of whatever routes a test mounts.

use aerini_engine::{
    db::WorkflowDb,
    executor::{CredentialResolver, WorkflowExecutor},
    node::{NodeRegistry, Reloadable},
    nodes::register_builtins,
    plugin_loader::PluginLoadReport,
    scheduler::{SchedulerDaemon, SchedulerDb},
    store::{CredentialStore, KeySource, StoreCredentialResolver},
    EventSink,
};
use axum::{middleware, Router};
use std::sync::Arc;

use super::state::ApiState;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("aerini_api_routes_test_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).expect("create test dir");
    p
}

/// `tag` must be unique per test: it names the temp directory, and tests in
/// one process run concurrently. Shell, Code and Database nodes are disabled,
/// as on a default `aerini-server api`.
pub(crate) fn make_state(tag: &str) -> ApiState {
    let dir = temp_dir(tag);

    let db = Arc::new(WorkflowDb::open(&dir.join("aerini.db"), 4).expect("open workflow db"));
    let creds = Arc::new(
        CredentialStore::open(&dir.join("creds.db"), KeySource::File(dir.join("aerini.key")))
            .expect("open credential store"),
    );

    let mut registry = NodeRegistry::new();
    register_builtins(&mut registry, &dir, Some(Arc::clone(&db)));
    let registry = Arc::new(Reloadable::new(registry));

    let (sse_tx, _) = tokio::sync::broadcast::channel::<String>(16);
    let sink = Arc::new(crate::event_bridge::BroadcastEventSink { tx: sse_tx.clone() });
    let resolver = Arc::new(StoreCredentialResolver { store: Arc::clone(&creds) });

    let scheduler = Arc::new(SchedulerDaemon::new(
        Arc::clone(&db) as Arc<dyn SchedulerDb>,
        Arc::clone(&registry),
        Arc::clone(&resolver) as Arc<dyn CredentialResolver>,
        Arc::clone(&sink) as Arc<dyn EventSink>,
    ));

    let base_executor = WorkflowExecutor::new(
        registry.current(),
        Arc::clone(&resolver) as Arc<dyn CredentialResolver>,
    );

    let token_store = Arc::new(
        crate::token_store::TokenStore::open(&dir.join("tokens.db"), [7u8; 32])
            .expect("open token store"),
    );

    ApiState {
        db,
        scheduler,
        creds,
        registry,
        reload_lock: Arc::new(tokio::sync::Mutex::new(())),
        last_load_report: Arc::new(tokio::sync::RwLock::new(PluginLoadReport::default())),
        data_dir: Arc::new(dir),
        plugin_dir: None,
        sse_tx,
        shutdown: tokio_util::sync::CancellationToken::new(),
        token_store,
        exec_locks: Arc::new(dashmap::DashMap::new()),
        env_allowlist: None,
        file_sandbox_dir: None,
        shell_exec_disabled: true,
        code_exec_disabled: true,
        database_exec_disabled: true,
        parallel_execution: false,
        max_concurrent_nodes: 1,
        server_max_duration_secs: None,
        base_executor,
        sse_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
        run_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
        queue_timeout: std::time::Duration::from_secs(1),
    }
}

/// Mints a token with exactly `scopes` and returns its raw bearer value.
pub(crate) fn token(state: &ApiState, scopes: &[&str]) -> String {
    state
        .token_store
        .create_token("test", scopes, None)
        .expect("create token")
}

/// Wraps `routes` in the production `auth_middleware` and binds `state`.
pub(crate) fn with_auth(routes: Router<ApiState>, state: ApiState) -> Router {
    routes
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::api_server::auth_middleware,
        ))
        .with_state(state)
}
