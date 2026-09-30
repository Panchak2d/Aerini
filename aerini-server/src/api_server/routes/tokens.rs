//! Token management routes (admin scope required).

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::token_store::TokenRecord;
use super::state::{ApiState, require_admin};

pub async fn list_tokens_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    match s.with_token_store(|store| store.list_tokens()).await {
        Ok(tokens) => (StatusCode::OK, Json(json!(tokens))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct CreateTokenBody {
    pub label:  String,
    #[serde(default = "default_token_scopes")]
    pub scopes: Vec<String>,
    pub expires_in_secs: Option<u64>,
    /// Optional: restrict this token to exactly these workflow ids (the
    /// per-token workflow ACL), granted atomically in this same call instead
    /// of requiring a separate `POST /api/tokens/:id/workflows/:wf_id` per
    /// id. Empty = unrestricted.
    #[serde(default)]
    pub workflow_ids: Vec<String>,
}

fn default_token_scopes() -> Vec<String> {
    vec!["read".to_string(), "write".to_string()]
}

const VALID_SCOPES: &[&str] = &["read", "write", "admin"];

pub async fn create_token_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Json(b):           Json<CreateTokenBody>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    if b.label.is_empty() || b.label.len() > 256 {
        return (StatusCode::BAD_REQUEST,
            Json(json!({"error": "label must be 1–256 characters"}))).into_response();
    }
    let invalid: Vec<&str> = b.scopes.iter()
        .map(|s| s.as_str())
        .filter(|s| !VALID_SCOPES.contains(s))
        .collect();
    if !invalid.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": format!("Invalid scopes: {:?}. Allowed: read, write, admin", invalid)
        }))).into_response();
    }
    if let Some(secs) = b.expires_in_secs {
        let representable = i64::try_from(secs).ok()
            .and_then(chrono::TimeDelta::try_seconds)
            .and_then(|d| chrono::Utc::now().checked_add_signed(d));
        if representable.is_none() {
            return (StatusCode::BAD_REQUEST,
                Json(json!({"error": "expires_in_secs is too large"}))).into_response();
        }
    }
    let label        = b.label.clone();
    let scopes       = b.scopes.clone();
    let workflow_ids = b.workflow_ids.clone();
    let expires      = b.expires_in_secs;
    let created = s.with_token_store(move |store| {
        let scopes_ref: Vec<&str> = scopes.iter().map(|sc| sc.as_str()).collect();
        store.create_token_with_workflows(&label, &scopes_ref, expires, &workflow_ids)
    }).await;
    let (token_id, raw) = match created {
        Ok(pair) => pair,
        Err(e)   => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
    };
    (StatusCode::CREATED, Json(json!({
        "token":       raw,
        "token_id":    token_id,
        "label":       b.label,
        "scopes":      b.scopes,
        "expires_in_secs": b.expires_in_secs,
        "workflow_ids": b.workflow_ids,
        "note":        "Save this token — it will not be shown again."
    }))).into_response()
}

pub async fn revoke_token_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    if caller.token_id == id {
        return (StatusCode::BAD_REQUEST,
            Json(json!({"error": "Cannot revoke the token you are currently using"}))).into_response();
    }
    match s.with_token_store(move |store| store.revoke_token(&id)).await {
        Ok(()) => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
    }
}

// ── Per-workflow ACL handlers ────────────────────────────────────────────────

/// GET /api/tokens/:id/workflows — list workflow IDs the token is restricted to.
/// Empty list = unrestricted.
pub async fn list_token_workflows_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(token_id):    Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    let target = token_id.clone();
    match s.with_token_store(move |store| store.acl_list(&target)).await {
        Ok(ids) => {
            let note = if ids.is_empty() {
                "No ACL entries — token is not restricted to specific workflows."
            } else {
                "Token is restricted to these workflow IDs only."
            };
            (StatusCode::OK, Json(json!({
                "token_id":     token_id,
                "workflow_ids": ids,
                "note":         note
            }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
    }
}

/// POST /api/tokens/:id/workflows/:wf_id — grant access to a workflow.
pub async fn grant_token_workflow_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path((token_id, workflow_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    let target = token_id.clone();
    let wf     = workflow_id.clone();
    match s.with_token_store(move |store| store.acl_grant(&target, &wf)).await {
        Ok(()) => (StatusCode::CREATED, Json(json!({
            "ok":          true,
            "token_id":    token_id,
            "workflow_id": workflow_id,
            "note":        "Token is now restricted to its granted workflow(s)."
        }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
    }
}

/// DELETE /api/tokens/:id/workflows/:wf_id — revoke access to a workflow.
/// Revoking a token's last grant leaves it unrestricted, which the response
/// says explicitly.
pub async fn revoke_token_workflow_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path((token_id, workflow_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    let result = s.with_token_store(move |store| {
        store.acl_revoke(&token_id, &workflow_id)?;
        store.acl_list(&token_id)
    }).await;
    match result {
        Ok(remaining) => {
            let mut body = json!({"ok": true});
            if remaining.is_empty() {
                body["note"] = json!("Token has no remaining workflow grants and is now unrestricted.");
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
    }
}
