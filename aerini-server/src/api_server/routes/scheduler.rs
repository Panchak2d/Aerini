//! Scheduler routes: list, start, stop.

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::token_store::TokenRecord;
use super::state::{ApiState, require_read, require_workflow_acl, require_write};
use super::workflows::PaginationParams;

pub async fn list_scheduler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Query(p):          Query<PaginationParams>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }

    // Per-workflow ACL: a read-scoped token only sees job rows for
    // workflows its ACL grants cover. None = unrestricted (admin, or no
    // ACL rows).
    let acl_filter = match s.acl_filter(&caller).await {
        Ok(f)  => f,
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("ACL lookup failed: {}", e)})),
        ).into_response(),
    };

    let limit  = p.limit.min(500);
    let offset = p.offset;

    let result = match acl_filter {
        // Unrestricted token — unchanged, DB-level pagination.
        None => tokio::task::spawn_blocking(move || s.scheduler.list_jobs_paginated(offset, limit)).await,
        // ACL-restricted token — filter the full set before paginating, so
        // `total` and the offset/limit window only ever reflect workflows
        // this token is allowed to see.
        Some(allowed) => tokio::task::spawn_blocking(move || {
            let filtered: Vec<_> = s.scheduler.list_jobs()?
                .into_iter()
                .filter(|row| allowed.contains(&row.workflow_id))
                .collect();
            let total = filtered.len();
            let page: Vec<_> = filtered.into_iter().skip(offset).take(limit).collect();
            Ok::<_, String>((page, total))
        }).await,
    };

    match result {
        Ok(Ok((items, total))) => {
            // Never return a webhook trigger's plaintext secret over the API
            // — the scheduler's own listener
            // still holds the real secret internally for HMAC comparison.
            let items: Vec<_> = items.iter().map(|row| row.redacted()).collect();
            (StatusCode::OK, Json(json!({"items": items, "total": total, "limit": limit, "offset": offset}))).into_response()
        },
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct StartBody { #[serde(default)] pub always_on: bool, pub port_override: Option<u16> }

pub async fn start_job(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
    Json(b):           Json<StartBody>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    if let Err(e) = require_workflow_acl(&s, &caller, &id).await { return e.into_response(); }
    match tokio::task::spawn_blocking(move || s.scheduler.start_job(&id, b.port_override, Some(b.always_on))).await {
        Ok(Ok(()))  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Ok(Err(e))  => (StatusCode::BAD_REQUEST, Json(json!({"error":format!("{:?}",e)}))).into_response(),
        Err(e)      => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

pub async fn stop_job(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    if let Err(e) = require_workflow_acl(&s, &caller, &id).await { return e.into_response(); }
    match tokio::task::spawn_blocking(move || s.scheduler.stop_job(&id)).await {
        Ok(Ok(()))  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Ok(Err(e))  => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)      => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}
