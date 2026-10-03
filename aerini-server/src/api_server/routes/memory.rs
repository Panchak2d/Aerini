//! Live per-workflow/per-node memory breakdown

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::json;

use crate::token_store::TokenRecord;
use super::state::{internal_error, ApiState, require_read};

pub async fn get_memory(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) {
        return e.into_response();
    }

    // Per-workflow ACL. None = unrestricted
    // (admin, or a token with no ACL rows).
    let acl_filter = match s.acl_filter(&caller).await {
        Ok(f) => f,
        Err(e) => return internal_error("ACL lookup failed", e).into_response(),
    };

    // `snapshot()` is a DashMap iteration over a small, in-memory, process-
    // global registry ("cheap to call repeatedly")
    // not a blocking I/O call, so unlike the DB-backed handlers in this
    // module family it does not need `spawn_blocking`.
    let mut items = aerini_engine::mem_tracking::snapshot();
    if let Some(allowed) = acl_filter {
        items.retain(|run| allowed.contains(&run.workflow_id));
    }

    (StatusCode::OK, Json(json!(items))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aerini_engine::{
        db::WorkflowDb,
        executor::{CredentialResolver, WorkflowExecutor},
        mem_tracking::{run_tracked, GroupMeta},
        node::{NodeRegistry, Reloadable},
        nodes::register_builtins,
        plugin_loader::PluginLoadReport,
        scheduler::{SchedulerDaemon, SchedulerDb},
        store::{CredentialStore, KeySource, StoreCredentialResolver},
        EventSink,
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        middleware,
        routing::get,
        Router,
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    static TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aerini_api_memory_test_{}_{}", tag, std::process::id()));
        let _ = std::fs::create_dir_all(&p);
        p
    }

    /// Builds a real `ApiState` backed by temp-directory SQLite stores —
    /// mirrors `api_server::run()`'s own construction (this module's parent,
    /// `mod.rs`), minus the parts (TCP bind, CORS, rate limiter, scheduler
    /// ticking) irrelevant to exercising one route handler behind the real
    /// `auth_middleware`.
    fn make_state(tag: &str) -> ApiState {
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

        // Fixed, non-secret key — this store never holds a real token.
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

    /// Mounts only `/api/memory` behind the real, production `auth_middleware`
    /// (`crate::api_server::auth_middleware` — private to `api_server`,
    /// visible here because `routes::memory::tests` is one of its
    /// descendants; written as an absolute path rather than `super::super`
    /// to avoid miscounting how many modules deep `tests` itself sits) — not
    /// a reimplementation of auth for the test.
    fn router(state: ApiState) -> Router {
        Router::new()
            .route("/api/memory", get(super::get_memory))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                crate::api_server::auth_middleware,
            ))
            .with_state(state)
    }

    #[tokio::test]
    async fn requires_auth_and_read_scope() {
        let _serial = TEST_SERIAL.lock().await;
        let state = make_state("auth");
        let app = router(state.clone());

        // No Authorization header at all -> auth_middleware rejects (401).
        let res = app
            .clone()
            .oneshot(Request::builder().uri("/api/memory").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Valid token, but no 'read' scope -> require_read rejects (403).
        let write_only = state
            .token_store
            .create_token("write-only", &["write"], None)
            .expect("create token");
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/memory")
                    .header("authorization", format!("Bearer {}", write_only))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // Valid read-scoped token -> 200, and no live runs -> empty array.
        let reader = state
            .token_store
            .create_token("reader", &["read"], None)
            .expect("create token");
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/memory")
                    .header("authorization", format!("Bearer {}", reader))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let items: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            items.as_array().expect("response must be a JSON array").is_empty(),
            "no workflow is running in this test — snapshot should be empty"
        );
    }

    #[tokio::test]
    async fn acl_restricted_token_only_sees_its_granted_workflow() {
        let _serial = TEST_SERIAL.lock().await;
        let state = make_state("acl");
        let app = router(state.clone());

        let visible_wf = format!("wf_visible_{}", std::process::id());
        let hidden_wf = format!("wf_hidden_{}", std::process::id());

        let restricted = state
            .token_store
            .create_token("restricted", &["read"], None)
            .expect("create token");
        let restricted_record = state
            .token_store
            .verify_token(&restricted)
            .expect("token should verify right after creation");
        state
            .token_store
            .acl_grant(&restricted_record.token_id, &visible_wf)
            .expect("grant");

        let visible_for_group = visible_wf.clone();
        let hidden_for_group = hidden_wf.clone();
        let visible_for_assert = visible_wf.clone();
        let hidden_for_assert = hidden_wf.clone();

        // Register two live "Run" groups — one the token is ACL'd to, one it
        // isn't — and issue the request while both are still alive (a
        // TrackedGroup only exists for the lifetime of the future it wraps).
        run_tracked(GroupMeta::run(hidden_for_group), async move {
            run_tracked(GroupMeta::run(visible_for_group), async move {
                let req = Request::builder()
                    .uri("/api/memory")
                    .header("authorization", format!("Bearer {}", restricted))
                    .body(Body::empty())
                    .unwrap();
                let res = app.oneshot(req).await.unwrap();
                assert_eq!(res.status(), StatusCode::OK);
                let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
                let items: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let ids: Vec<&str> = items
                    .as_array()
                    .expect("response must be a JSON array")
                    .iter()
                    .filter_map(|r| r.get("workflow_id").and_then(|v| v.as_str()))
                    .collect();
                assert!(
                    ids.contains(&visible_for_assert.as_str()),
                    "granted workflow must be visible: {:?}",
                    ids
                );
                assert!(
                    !ids.contains(&hidden_for_assert.as_str()),
                    "ungranted workflow must be filtered out: {:?}",
                    ids
                );
            })
            .await;
        })
        .await;
    }
}
