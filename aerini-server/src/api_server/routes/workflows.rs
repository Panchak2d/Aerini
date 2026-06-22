//! Workflow CRUD, run, and SSE routes.

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Sse},
    Json,
};
use aerini_engine::model::Workflow;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::OwnedSemaphorePermit;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use tokio_util::sync::CancellationToken;

use crate::token_store::TokenRecord;
use super::state::{ApiState, require_read, require_write};

pub async fn health() -> Json<Value> {
    Json(json!({"status":"ok","version":aerini_engine::ENGINE_VERSION}))
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
    match tokio::task::spawn_blocking(move || s.db.load(&id)).await {
        Ok(Ok(Some(wf))) => (StatusCode::OK, Json(json!(wf.to_json_pretty().unwrap_or_default()))).into_response(),
        Ok(Ok(None))     => (StatusCode::NOT_FOUND, Json(json!({"error":"Not found"}))).into_response(),
        Ok(Err(e))       => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)           => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct SaveWorkflowBody { pub workflow_json: String }

pub async fn save_workflow(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Json(b):           Json<SaveWorkflowBody>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    let wf = match Workflow::from_json(&b.workflow_json) {
        Ok(w)  => w,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error":e.to_string()}))).into_response(),
    };
    match tokio::task::spawn_blocking(move || s.db.save(&wf)).await {
        Ok(Ok(())) => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
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
    let _ = s.scheduler.stop_job(&id);

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
