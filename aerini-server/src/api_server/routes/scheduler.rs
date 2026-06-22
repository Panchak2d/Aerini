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
use super::state::{ApiState, require_read, require_write};
use super::workflows::PaginationParams;

pub async fn list_scheduler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Query(p):          Query<PaginationParams>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }
    let limit = p.limit.min(500);
    match tokio::task::spawn_blocking(move || s.scheduler.list_jobs_paginated(p.offset, limit)).await {
        Ok(Ok((items, total))) => {
            (StatusCode::OK, Json(json!({"items": items, "total": total, "limit": limit, "offset": p.offset}))).into_response()
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
    match tokio::task::spawn_blocking(move || s.scheduler.stop_job(&id)).await {
        Ok(Ok(()))  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Ok(Err(e))  => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)      => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}
