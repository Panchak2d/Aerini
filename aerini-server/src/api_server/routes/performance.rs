//! Server REST for the Performance Monitor 

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::token_store::TokenRecord;
use super::state::{ApiState, require_read, require_write};

/// Single-workflow ACL gate — verbatim copy of
/// `scheduler::require_workflow_acl`. `Ok(())` if `caller` is unrestricted
/// (admin, or no ACL rows) or `workflow_id` is explicitly granted.
fn require_workflow_acl(
    s:           &ApiState,
    caller:      &TokenRecord,
    workflow_id: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match s.token_store.acl_filter(caller) {
        Ok(None) => Ok(()),
        Ok(Some(allowed)) if allowed.contains(workflow_id) => Ok(()),
        Ok(Some(_)) => Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "token ACL does not permit access to this workflow"})),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("ACL lookup failed: {}", e)})),
        )),
    }
}

/// Fetch-then-check ACL gate for the `{run_id}` routes — see module doc
/// comment's 404-vs-403 decision for why this returns a bare `bool` rather
/// than committing to a status code itself (both call sites map `false` to
/// `404`, never `403`).
fn acl_permits(s: &ApiState, caller: &TokenRecord, workflow_id: &str) -> Result<bool, String> {
    match s.token_store.acl_filter(caller) {
        Ok(None)          => Ok(true),
        Ok(Some(allowed)) => Ok(allowed.contains(workflow_id)),
        Err(e)            => Err(e.to_string()),
    }
}

pub async fn get_live(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) {
        return e.into_response();
    }

    let acl_filter = match s.token_store.acl_filter(&caller) {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("ACL lookup failed: {}", e)})),
            )
                .into_response()
        }
    };

    // In-memory DashMap iteration, same as `all_live_snapshots`'s own doc
    // comment / `mem_tracking::snapshot()`'s precedent — no `spawn_blocking`.
    let mut items = aerini_engine::perf_monitor::all_live_snapshots();
    if let Some(allowed) = acl_filter {
        items.retain(|r| allowed.contains(&r.workflow_id));
    }

    (StatusCode::OK, Json(json!(items))).into_response()
}

#[derive(Deserialize)]
pub struct ListReportsParams {
    pub workflow_id: String,
    #[serde(default = "default_reports_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_reports_limit() -> i64 { 100 }

pub async fn list_reports(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Query(p):          Query<ListReportsParams>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }
    if let Err(e) = require_workflow_acl(&s, &caller, &p.workflow_id) { return e.into_response(); }

    let db          = Arc::clone(&s.db);
    let workflow_id = p.workflow_id.clone();
    let limit       = p.limit;
    let offset      = p.offset;

    match tokio::task::spawn_blocking(move || db.list_performance_reports(&workflow_id, offset, limit)).await {
        Ok(Ok(items)) => {
            // Mirrors `list_performance_reports`'s own clamp so `has_more`
            // reflects the page size the DB layer actually used, not the
            // raw (possibly out-of-range) query param.
            let effective_limit = limit.clamp(1, 1000);
            let has_more = items.len() as i64 == effective_limit;
            (StatusCode::OK, Json(json!({
                "items": items, "limit": effective_limit, "offset": offset.max(0), "has_more": has_more
            }))).into_response()
        }
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

pub async fn get_report(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(run_id):      Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }

    let db      = Arc::clone(&s.db);
    let lookup  = run_id.clone();
    match tokio::task::spawn_blocking(move || db.get_performance_report(&lookup)).await {
        Ok(Ok(Some(record))) => match acl_permits(&s, &caller, &record.report.workflow_id) {
            Ok(true)  => (StatusCode::OK, Json(json!(record))).into_response(),
            Ok(false) => (StatusCode::NOT_FOUND, Json(json!({"error": "Not found"}))).into_response(),
            Err(e)    => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("ACL lookup failed: {}", e)}))).into_response(),
        },
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({"error": "Not found"}))).into_response(),
        Ok(Err(e))   => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
        Err(e)       => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

pub async fn delete_report(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(run_id):      Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }

    let db     = Arc::clone(&s.db);
    let lookup = run_id.clone();
    let record = match tokio::task::spawn_blocking(move || db.get_performance_report(&lookup)).await {
        Ok(Ok(Some(record))) => record,
        Ok(Ok(None)) => return (StatusCode::NOT_FOUND, Json(json!({"error": "Not found"}))).into_response(),
        Ok(Err(e))   => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
        Err(e)       => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };

    match acl_permits(&s, &caller, &record.report.workflow_id) {
        Ok(true)  => {}
        Ok(false) => return (StatusCode::NOT_FOUND, Json(json!({"error": "Not found"}))).into_response(),
        Err(e)    => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("ACL lookup failed: {}", e)}))).into_response(),
    }

    // Benign TOCTOU: if another request deletes the same row between the
    // fetch above and this call, `delete_performance_report` is a no-op on
    // a missing row (`DELETE ... WHERE id = ?1` matches zero rows, still
    // `Ok(())` — same idempotent contract `clear_recent` documents for its
    // own "nothing stored" case) — not an error either caller needs to see.
    let db2    = Arc::clone(&s.db);
    let delete = run_id.clone();
    match tokio::task::spawn_blocking(move || db2.delete_performance_report(&delete)).await {
        Ok(Ok(()))  => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Ok(Err(e))  => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
        Err(e)      => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct ClearReportsParams {
    pub workflow_id: String,
}

pub async fn clear_reports(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Query(p):          Query<ClearReportsParams>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    if let Err(e) = require_workflow_acl(&s, &caller, &p.workflow_id) { return e.into_response(); }

    let db          = Arc::clone(&s.db);
    let workflow_id = p.workflow_id.clone();
    match tokio::task::spawn_blocking(move || db.clear_performance_reports(&workflow_id)).await {
        Ok(Ok(()))  => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Ok(Err(e))  => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
        Err(e)      => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aerini_engine::{
        db::WorkflowDb,
        executor::{CredentialResolver, WorkflowExecutor},
        node::NodeRegistry,
        nodes::register_builtins,
        perf_monitor::{monitor_run, HistorySample, PerfStatus, PerformanceReport},
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
    use tower::ServiceExt;

    // Same technique as `memory::tests::TEST_SERIAL` (cited there in full):
    // `get_live`'s "empty" assertion and the live-ACL test below both touch
    // the process-global `perf_monitor::LIVE` registry inside this same
    // test binary, so they're serialized against each other. Only these two
    // tests in this module register live entries.
    static TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aerini_api_performance_test_{}_{}", tag, std::process::id()));
        let _ = std::fs::create_dir_all(&p);
        p
    }

    /// Verbatim copy of `memory::tests::make_state` (cited there in full) —
    /// builds a real `ApiState` backed by temp-directory SQLite stores.
    fn make_state(tag: &str) -> ApiState {
        let dir = temp_dir(tag);

        let db = Arc::new(WorkflowDb::open(&dir.join("aerini.db"), 4).expect("open workflow db"));
        let creds = Arc::new(
            CredentialStore::open(&dir.join("creds.db"), KeySource::File(dir.join("aerini.key")))
                .expect("open credential store"),
        );

        let mut registry = NodeRegistry::new();
        register_builtins(&mut registry, &dir, Some(Arc::clone(&db)));
        let registry = Arc::new(registry);

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
            Arc::clone(&registry),
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
            sse_tx,
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

    /// Mounts all five `/api/performance/*` routes behind the real,
    /// production `auth_middleware` — not a reimplementation of auth.
    fn router(state: ApiState) -> Router {
        Router::new()
            .route("/api/performance/live", get(super::get_live))
            .route("/api/performance/reports", get(super::list_reports).delete(super::clear_reports))
            .route("/api/performance/reports/{run_id}", get(super::get_report).delete(super::delete_report))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                crate::api_server::auth_middleware,
            ))
            .with_state(state)
    }

    fn sample_report(workflow_id: &str) -> PerformanceReport {
        PerformanceReport {
            workflow_id: workflow_id.to_string(),
            status: PerfStatus::Success,
            started_at_ms: 1_000,
            finished_at_ms: 2_000,
            duration_ms: 1_000,
            sampling_interval_ms: 150,
            baseline_bytes: 100,
            final_bytes: 200,
            peak_bytes: 300,
            peak_at_ms: 1_500,
            minimum_bytes: 100,
            average_bytes: 180,
            delta_bytes: 100,
            sample_count: 3,
            history: vec![HistorySample { at_ms: 1_100, bytes: 150 }],
        }
    }

    #[tokio::test]
    async fn requires_auth_and_read_scope_for_live() {
        let _serial = TEST_SERIAL.lock().await;
        let state = make_state("auth");
        let app = router(state.clone());

        let res = app
            .clone()
            .oneshot(Request::builder().uri("/api/performance/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let write_only = state.token_store.create_token("write-only", &["write"], None).expect("create token");
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/performance/live")
                    .header("authorization", format!("Bearer {}", write_only))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let reader = state.token_store.create_token("reader", &["read"], None).expect("create token");
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/performance/live")
                    .header("authorization", format!("Bearer {}", reader))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let items: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(items.as_array().expect("must be a JSON array").is_empty());
    }

    #[tokio::test]
    async fn live_acl_restricted_token_only_sees_its_granted_workflow() {
        let _serial = TEST_SERIAL.lock().await;
        let state = make_state("live-acl");
        let app = router(state.clone());

        let visible_wf = format!("wf_visible_{}", std::process::id());
        let hidden_wf = format!("wf_hidden_{}", std::process::id());

        let restricted = state.token_store.create_token("restricted", &["read"], None).expect("create token");
        let restricted_record = state.token_store.verify_token(&restricted).expect("verify");
        state.token_store.acl_grant(&restricted_record.token_id, &visible_wf).expect("grant");

        let visible_for_run = visible_wf.clone();
        let hidden_for_run = hidden_wf.clone();

        monitor_run(hidden_for_run, async move {
            monitor_run(visible_for_run, async move {
                let req = Request::builder()
                    .uri("/api/performance/live")
                    .header("authorization", format!("Bearer {}", restricted))
                    .body(Body::empty())
                    .unwrap();
                let res = app.oneshot(req).await.unwrap();
                assert_eq!(res.status(), StatusCode::OK);
                let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
                let items: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let ids: Vec<&str> = items
                    .as_array()
                    .expect("must be a JSON array")
                    .iter()
                    .filter_map(|r| r.get("workflow_id").and_then(|v| v.as_str()))
                    .collect();
                assert!(ids.contains(&visible_wf.as_str()), "granted workflow must be visible: {:?}", ids);
                assert!(!ids.contains(&hidden_wf.as_str()), "ungranted workflow must be filtered out: {:?}", ids);
            }, |_: &()| PerfStatus::Success)
            .await;
        }, |_: &()| PerfStatus::Success)
        .await;
    }

    #[tokio::test]
    async fn list_reports_acl_gate() {
        let state = make_state("list-acl");
        let app = router(state.clone());

        let visible_wf = format!("wf_visible_{}", std::process::id());
        let hidden_wf = format!("wf_hidden_{}", std::process::id());
        state.db.save_performance_report("run_visible", &sample_report(&visible_wf)).expect("save");
        state.db.save_performance_report("run_hidden", &sample_report(&hidden_wf)).expect("save");

        let restricted = state.token_store.create_token("restricted", &["read"], None).expect("create token");
        let restricted_record = state.token_store.verify_token(&restricted).expect("verify");
        state.token_store.acl_grant(&restricted_record.token_id, &visible_wf).expect("grant");

        // Granted workflow -> 200, one item.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/performance/reports?workflow_id={}", visible_wf))
                    .header("authorization", format!("Bearer {}", restricted))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["items"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["has_more"], false);

        // Ungranted workflow -> 403, not a leaked 200-with-empty-items.
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/performance/reports?workflow_id={}", hidden_wf))
                    .header("authorization", format!("Bearer {}", restricted))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn get_and_delete_report_hide_non_granted_as_404() {
        let state = make_state("run-acl");
        let app = router(state.clone());

        let visible_wf = format!("wf_visible_{}", std::process::id());
        let hidden_wf = format!("wf_hidden_{}", std::process::id());
        state.db.save_performance_report("run_visible", &sample_report(&visible_wf)).expect("save");
        state.db.save_performance_report("run_hidden", &sample_report(&hidden_wf)).expect("save");

        let restricted = state.token_store.create_token("restricted", &["read", "write"], None).expect("create token");
        let restricted_record = state.token_store.verify_token(&restricted).expect("verify");
        state.token_store.acl_grant(&restricted_record.token_id, &visible_wf).expect("grant");

        // Granted run_id -> 200.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/performance/reports/run_visible")
                    .header("authorization", format!("Bearer {}", restricted))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // Ungranted run_id -> 404, not 403 (see module doc comment).
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/performance/reports/run_hidden")
                    .header("authorization", format!("Bearer {}", restricted))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        // Truly nonexistent run_id -> also 404, indistinguishable from the
        // ACL-denied case above (that's the point of the decision).
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/performance/reports/run_does_not_exist")
                    .header("authorization", format!("Bearer {}", restricted))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        // DELETE on the ungranted run_id -> 404, and it must NOT be deleted.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/performance/reports/run_hidden")
                    .header("authorization", format!("Bearer {}", restricted))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert!(state.db.get_performance_report("run_hidden").unwrap().is_some(), "ungranted report must survive a denied delete");

        // DELETE on the granted run_id -> 200, and it IS deleted.
        let res = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/performance/reports/run_visible")
                    .header("authorization", format!("Bearer {}", restricted))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(state.db.get_performance_report("run_visible").unwrap().is_none(), "granted report must be gone after delete");
    }

    #[tokio::test]
    async fn write_scope_required_for_mutations() {
        let state = make_state("write-scope");
        let app = router(state.clone());

        let wf = format!("wf_{}", std::process::id());
        state.db.save_performance_report("run_x", &sample_report(&wf)).expect("save");

        let reader = state.token_store.create_token("reader", &["read"], None).expect("create token");

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/performance/reports/run_x")
                    .header("authorization", format!("Bearer {}", reader))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let res = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/performance/reports?workflow_id={}", wf))
                    .header("authorization", format!("Bearer {}", reader))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn clear_reports_acl_gate_and_effect() {
        let state = make_state("clear-acl");
        let app = router(state.clone());

        let visible_wf = format!("wf_visible_{}", std::process::id());
        let hidden_wf = format!("wf_hidden_{}", std::process::id());
        state.db.save_performance_report("run_visible", &sample_report(&visible_wf)).expect("save");
        state.db.save_performance_report("run_hidden", &sample_report(&hidden_wf)).expect("save");

        let restricted = state.token_store.create_token("restricted", &["read", "write"], None).expect("create token");
        let restricted_record = state.token_store.verify_token(&restricted).expect("verify");
        state.token_store.acl_grant(&restricted_record.token_id, &visible_wf).expect("grant");

        // Ungranted workflow_id -> 403, row untouched.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/performance/reports?workflow_id={}", hidden_wf))
                    .header("authorization", format!("Bearer {}", restricted))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert!(state.db.get_performance_report("run_hidden").unwrap().is_some());

        // Granted workflow_id -> 200, its row(s) cleared.
        let res = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/performance/reports?workflow_id={}", visible_wf))
                    .header("authorization", format!("Bearer {}", restricted))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(state.db.get_performance_report("run_visible").unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_required_workflow_id_query_param_is_400() {
        let state = make_state("missing-param");
        let app = router(state.clone());
        let reader = state.token_store.create_token("reader", &["read"], None).expect("create token");

        // VERIFIED (axum 0.8 docs, docs.rs/axum/latest/axum/extract/struct.Query.html):
        // `Query<T>` rejects an unparseable/incomplete query string with 400.
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/performance/reports")
                    .header("authorization", format!("Bearer {}", reader))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }
}
