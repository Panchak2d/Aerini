//! Workflow CRUD, run, and SSE routes.

use axum::{
    extract::{Extension, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Sse},
    Json,
};
use aerini_engine::{db::SaveOutcome, model::Workflow};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::OwnedSemaphorePermit;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use tokio_util::sync::CancellationToken;

use crate::token_store::TokenRecord;
use super::state::{ApiState, require_read, require_write};

pub async fn health() -> Json<Value> {
    let node_bundled = match tokio::task::spawn_blocking(
        aerini_engine::nodes::code_node::bundled_node_health_check,
    ).await {
        Ok(Ok(version)) => json!({"status": "ok", "version": version}),
        Ok(Err(e))      => json!({"status": "error", "message": e}),
        Err(e)          => json!({"status": "error", "message": e.to_string()}),
    };
    Json(json!({"status":"ok","version":aerini_engine::ENGINE_VERSION,"node_bundled":node_bundled}))
}

#[derive(Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize { 100 }

#[derive(Deserialize, Default)]
pub struct SseParams {
    /// Optional workflow ID to filter SSE events. Only events for this workflow
    /// will be forwarded. Subject to token ACL — will 403 if the token's ACL
    /// does not include the requested workflow.
    pub workflow_id: Option<String>,
}

pub async fn list_workflows(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Query(p):          Query<PaginationParams>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }
    let limit = p.limit.min(500);
    match tokio::task::spawn_blocking(move || s.db.list_paginated(limit, p.offset)).await {
        Ok(Ok((items, total))) => {
            (StatusCode::OK, Json(json!({"items": items, "total": total, "limit": limit, "offset": p.offset}))).into_response()
        },
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

pub async fn get_workflow(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }
    match tokio::task::spawn_blocking(move || s.db.load_with_row_version(&id)).await {
        Ok(Ok(Some((wf, row_version)))) => {
            let mut resp = (StatusCode::OK, Json(json!(wf.to_json_pretty().unwrap_or_default()))).into_response();
            set_etag(&mut resp, row_version);
            resp
        }
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({"error":"Not found"}))).into_response(),
        Ok(Err(e))   => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)       => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

/// Sets the `ETag` response header to the given `row_version`, quoted per
/// HTTP's ETag syntax. `row_version` is always a plain non-negative counter,
/// so formatting can't produce characters `HeaderValue` would reject —
/// this only returns without setting the header if that invariant is ever
/// violated, rather than panicking on a response header.
fn set_etag(resp: &mut axum::response::Response, row_version: i64) {
    if let Ok(v) = HeaderValue::from_str(&format!("\"{row_version}\"")) {
        resp.headers_mut().insert(header::ETAG, v);
    }
}

/// Parses an `If-Match` header value into the row_version it asserts.
/// Accepts a bare integer (`5`) or a quoted ETag (`"5"`) — the two forms
/// that round-trip against the `ETag` this same API emits, so a client that
/// sends back exactly what it received always parses cleanly. Any other
/// form (wildcard, weak ETag, multiple values) is rejected rather than
/// guessed at.
fn parse_if_match(raw: &str) -> Option<i64> {
    let trimmed  = raw.trim();
    let unquoted = trimmed.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(trimmed);
    unquoted.parse::<i64>().ok()
}

#[derive(Deserialize)]
pub struct SaveWorkflowBody { pub workflow_json: String }

pub async fn save_workflow(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    headers:           HeaderMap,
    Json(b):           Json<SaveWorkflowBody>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    let wf = match Workflow::from_json(&b.workflow_json) {
        Ok(w)  => w,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error":e.to_string()}))).into_response(),
    };

    // No If-Match header → unconditional save (last-write-wins). Only a
    // client that opts in by sending back the ETag it last read gets the
    // conflict protection — other callers are unaffected.
    let if_match = match headers.get(header::IF_MATCH) {
        None => None,
        Some(v) => match v.to_str().ok().and_then(parse_if_match) {
            Some(version) => Some(version),
            None => return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "malformed If-Match header — expected the row_version from a prior ETag"})),
            ).into_response(),
        },
    };

    match tokio::task::spawn_blocking(move || s.db.save_checked(&wf, if_match)).await {
        Ok(Ok(outcome)) => {
            let (status, row_version, body) = match outcome {
                SaveOutcome::Created { row_version } | SaveOutcome::Updated { row_version } =>
                    (StatusCode::OK, Some(row_version), json!({"ok": true, "row_version": row_version})),
                SaveOutcome::Conflict { current_row_version } => (
                    StatusCode::CONFLICT,
                    None,
                    json!({
                        "error": "workflow was modified since you last loaded it",
                        "current_row_version": current_row_version,
                    }),
                ),
                SaveOutcome::NotFound => (
                    StatusCode::PRECONDITION_FAILED,
                    None,
                    json!({"error": "workflow not found — If-Match cannot be satisfied"}),
                ),
            };
            let mut resp = (status, Json(body)).into_response();
            if let Some(v) = row_version { set_etag(&mut resp, v); }
            resp
        }
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

pub async fn delete_workflow(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    if let Err(e) = s.scheduler.stop_job(&id) {
        tracing::warn!(workflow_id = %id, error = %e, "delete_workflow: stop_job failed before delete");
    }

    // Acquire (or create) the per-workflow exec lock before deleting.
    // This serialises against a concurrent run_workflow: if a run is in
    // progress it completes first; if delete holds the lock, run_workflow
    // will find NOT_FOUND after the 5-second timeout.
    let lock = {
        let entry = s.exec_locks.entry(id.clone()).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())));
        Arc::clone(&*entry)
    };
    let _guard = lock.lock().await;

    let exec_locks  = Arc::clone(&s.exec_locks);
    let id_for_lock = id.clone();

    match tokio::task::spawn_blocking(move || {
        s.db.delete_runs_for_workflow(&id)?;
        s.db.delete_scheduled_job(&id)?;
        s.db.delete(&id)
    }).await {
        Ok(Ok(())) => {
            exec_locks.remove(&id_for_lock);
            (StatusCode::OK, Json(json!({"ok":true}))).into_response()
        },
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct RunBody { #[serde(default)] pub initial_variables: HashMap<String, Value> }

pub async fn run_workflow(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
    Json(b):           Json<RunBody>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    let wf = match tokio::task::spawn_blocking({
        let db = Arc::clone(&s.db);
        let id = id.clone();
        move || db.load(&id)
    }).await {
        Ok(Ok(Some(wf))) => wf,
        Ok(Ok(None))     => return (StatusCode::NOT_FOUND, Json(json!({"error":"Not found"}))).into_response(),
        Ok(Err(e))       => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)           => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    };

    {
        if s.exec_locks.len() > 1000 {
            let db = Arc::clone(&s.db);
            if let Ok(Ok(live_ids)) = tokio::task::spawn_blocking(move || db.list_ids()).await {
                s.exec_locks.retain(|k, _| live_ids.contains(k));
            }
        }
    }

    // Acquire the global run semaphore BEFORE the per-workflow exec lock.
    // If the order were reversed, callers blocked on the exec lock would each
    // hold a semaphore slot, starving unrelated workflows.
    //
    // acquire_owned() queues the request; the caller waits up to queue_timeout
    // for a free slot rather than being rejected immediately.
    let _run_permit = match tokio::time::timeout(
        s.queue_timeout,
        Arc::clone(&s.run_semaphore).acquire_owned(),
    ).await {
        Ok(Ok(permit)) => permit,
        Ok(Err(_)) => {
            // Semaphore closed — should not happen in normal operation.
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "run semaphore closed"})),
            ).into_response();
        }
        Err(_elapsed) => {
            let retry_after = s.queue_timeout.as_secs().to_string();
            let mut response = (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "server is at capacity, no execution slot became available within the queue timeout"})),
            ).into_response();
            if let Ok(v) = axum::http::HeaderValue::from_str(&retry_after) {
                response.headers_mut().insert(axum::http::header::RETRY_AFTER, v);
            }
            return response;
        }
    };

    let lock = {
        let entry = s.exec_locks.entry(id.clone()).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())));
        Arc::clone(&*entry)
    };
    let _guard = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        lock.lock(),
    ).await {
        Ok(guard) => guard,
        Err(_)    => return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": "workflow already running"})),
        ).into_response(),
    };

    let cancel = CancellationToken::new();
    let executor = s.base_executor.clone()
        .with_registry(s.registry.current())
        .with_caller_is_admin(caller.has_scope("admin"))
        .with_cancel_token(cancel);

    let run_result = executor.run(Arc::new(wf), b.initial_variables).await;

    {
        let id_check = id.clone();
        let db = Arc::clone(&s.db);
        if let Ok(Ok(None)) = tokio::task::spawn_blocking(move || db.load(&id_check)).await {
            s.exec_locks.remove(&id);
        }
    }

    match run_result {
        Ok(result) => (StatusCode::OK, Json(json!(result))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

pub async fn sse_events(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Query(q):          Query<SseParams>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) {
        return e.into_response();
    }

    let permit: OwnedSemaphorePermit = match Arc::clone(&s.sse_semaphore).try_acquire_owned() {
        Ok(p)  => p,
        Err(_) => return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": "too many active SSE connections"})),
        ).into_response(),
    };

    // Build the effective workflow filter for this connection:
    // 1. If caller is admin — no filter unless they explicitly requested one.
    // 2. Otherwise, compute the ACL-based filter (None = unrestricted).
    // 3. If caller passed ?workflow_id=X, further restrict to that single ID
    //    (only if their ACL allows it, or if they are unrestricted).
    let acl_filter: Option<std::collections::HashSet<String>> = match s.token_store.acl_filter(&caller) {
        Ok(f) => f,
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("ACL lookup failed: {}", e)})),
        ).into_response(),
    };

    // Merge explicit ?workflow_id param with ACL filter.
    // Result: the set of workflow IDs whose events will be forwarded to this client.
    // None = forward everything.
    let effective_filter: Option<std::collections::HashSet<String>> = match (&acl_filter, &q.workflow_id) {
        // No ACL restriction, no explicit param → forward all
        (None, None) => None,
        // No ACL restriction, explicit param → forward only that workflow
        (None, Some(wf)) => {
            let mut set = std::collections::HashSet::new();
            set.insert(wf.clone());
            Some(set)
        }
        // ACL restricts to a set, no explicit param → use ACL set
        (Some(acl), None) => Some(acl.clone()),
        // ACL restricts to a set, explicit param → intersection; deny if not in ACL
        (Some(acl), Some(wf)) => {
            if !acl.contains(wf) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"error": "token ACL does not permit access to that workflow's events"})),
                ).into_response();
            }
            let mut set = std::collections::HashSet::new();
            set.insert(wf.clone());
            Some(set)
        }
    };

    let rx     = s.sse_tx.subscribe();
    let stream = BroadcastStream::new(rx)
        .filter_map(move |msg| {
            let raw = msg.ok()?;
            // If there is an active filter, parse the event JSON and check workflow_id.
            if let Some(ref filter) = effective_filter {
                let parsed: Option<Value> = serde_json::from_str(&raw).ok();
                let wf_id = parsed
                    .as_ref()
                    .and_then(|v| v.get("payload"))
                    .and_then(|p| p.get("workflow_id"))
                    .and_then(|id| id.as_str())
                    .map(|s| s.to_string());
                match wf_id {
                    Some(id) if filter.contains(&id) => {}
                    // Event has no workflow_id (e.g. heartbeat) — forward to all
                    None => {}
                    // workflow_id present but not in filter — drop
                    _ => return None,
                }
            }
            Some(Ok::<axum::response::sse::Event, std::convert::Infallible>(
                axum::response::sse::Event::default().data(raw)
            ))
        });

    struct GuardedStream<S> {
        inner:   S,
        _permit: OwnedSemaphorePermit,
    }
    impl<S: futures_core::Stream + Unpin> futures_core::Stream for GuardedStream<S> {
        type Item = S::Item;
        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            std::pin::Pin::new(&mut self.inner).poll_next(cx)
        }
    }
    let guarded = GuardedStream { inner: stream, _permit: permit };

    Sse::new(guarded).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("ping"),
    ).into_response()
}

#[cfg(test)]
mod if_match_tests {
    use super::parse_if_match;

    #[test]
    fn parse_if_match_accepts_bare_integer() {
        assert_eq!(parse_if_match("5"), Some(5));
    }

    // Normal case for real clients: this is the exact form set_etag() emits,
    // so a client round-tripping the ETag it received must parse cleanly.
    #[test]
    fn parse_if_match_accepts_quoted_integer() {
        assert_eq!(parse_if_match("\"5\""), Some(5));
    }

    // Edge case: anything that isn't a plain (optionally quoted) integer —
    // wildcard, weak ETag, garbage — is rejected rather than guessed at.
    #[test]
    fn parse_if_match_rejects_non_numeric_value() {
        assert_eq!(parse_if_match("*"), None);
        assert_eq!(parse_if_match("W/\"5\""), None);
        assert_eq!(parse_if_match("not-a-version"), None);
    }
}
