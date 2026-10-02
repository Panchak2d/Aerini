//! Token management routes (admin scope required).

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::token_store::{AclGrant, AclRevoke, TokenRecord};
use super::state::{internal_error, ApiState, require_admin};

pub async fn list_tokens_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    match s.with_token_store(|store| store.list_tokens()).await {
        Ok(tokens) => (StatusCode::OK, Json(json!(tokens))).into_response(),
        Err(e)     => internal_error("list tokens failed", e).into_response(),
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
const MAX_LABEL_CHARS: usize = 256;
const MAX_WORKFLOW_ID_CHARS: usize = 256;
const MAX_WORKFLOW_GRANTS: usize = 500;

fn bad_request(msg: impl Into<String>) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg.into()}))).into_response()
}

/// An `admin` token ignores the workflow ACL, so granting one workflows would
/// report a restriction that is never enforced.
fn admin_with_grants(scopes: &[String], workflow_ids: &[String]) -> bool {
    !workflow_ids.is_empty() && scopes.iter().any(|sc| sc == "admin")
}

/// Why `secs` cannot be a token lifetime, if it cannot: zero expires the token
/// before it can be used, and a huge value overflows the expiry date.
fn expiry_error(secs: u64) -> Option<&'static str> {
    if secs == 0 {
        return Some("expires_in_secs must be at least 1");
    }
    let representable = i64::try_from(secs).ok()
        .and_then(chrono::TimeDelta::try_seconds)
        .and_then(|d| chrono::Utc::now().checked_add_signed(d));
    representable.is_none().then_some("expires_in_secs is too large")
}

fn valid_workflow_id(id: &str) -> bool {
    !id.trim().is_empty() && id.chars().count() <= MAX_WORKFLOW_ID_CHARS
}

pub async fn create_token_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Json(b):           Json<CreateTokenBody>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    let label = b.label.trim().to_string();
    if label.is_empty() || label.chars().count() > MAX_LABEL_CHARS {
        return bad_request(format!("label must be 1–{MAX_LABEL_CHARS} characters"));
    }
    let invalid: Vec<&str> = b.scopes.iter()
        .map(|s| s.as_str())
        .filter(|s| !VALID_SCOPES.contains(s))
        .collect();
    if !invalid.is_empty() {
        return bad_request(format!("Invalid scopes: {:?}. Allowed: read, write, admin", invalid));
    }
    let mut scopes: Vec<String> = Vec::with_capacity(b.scopes.len());
    for sc in &b.scopes {
        if !scopes.contains(sc) { scopes.push(sc.clone()); }
    }
    if scopes.is_empty() {
        return bad_request("scopes must contain at least one of: read, write, admin");
    }
    if admin_with_grants(&scopes, &b.workflow_ids) {
        return bad_request("admin tokens are never restricted to specific workflows; drop `admin` from scopes or clear `workflow_ids`");
    }
    if b.workflow_ids.len() > MAX_WORKFLOW_GRANTS {
        return bad_request(format!("workflow_ids may contain at most {MAX_WORKFLOW_GRANTS} entries"));
    }
    if !b.workflow_ids.iter().all(|id| valid_workflow_id(id)) {
        return bad_request(format!("each workflow id must be 1–{MAX_WORKFLOW_ID_CHARS} characters"));
    }
    if let Some(msg) = b.expires_in_secs.and_then(expiry_error) {
        return bad_request(msg);
    }
    let workflow_ids = b.workflow_ids.clone();
    let expires      = b.expires_in_secs;
    let (label_in, scopes_in) = (label.clone(), scopes.clone());
    let created = s.with_token_store(move |store| {
        let scopes_ref: Vec<&str> = scopes_in.iter().map(|sc| sc.as_str()).collect();
        store.create_token_with_workflows(&label_in, &scopes_ref, expires, &workflow_ids)
    }).await;
    let (token_id, raw) = match created {
        Ok(pair) => pair,
        Err(e)   => return internal_error("create token failed", e).into_response(),
    };
    (StatusCode::CREATED, Json(json!({
        "token":       raw,
        "token_id":    token_id,
        "label":       label,
        "scopes":      scopes,
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
        Ok(true)  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({"error": "token not found"}))).into_response(),
        Err(e)    => internal_error("revoke token failed", e).into_response(),
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
        Err(e) => internal_error("list token workflows failed", e).into_response(),
    }
}

/// POST /api/tokens/:id/workflows/:wf_id — grant access to a workflow.
/// `404` when no active token has that id, so a mistyped id is not mistaken
/// for a token that is now restricted; `400` for an admin token, which the
/// ACL never restricts.
pub async fn grant_token_workflow_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path((token_id, workflow_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    if !valid_workflow_id(&workflow_id) {
        return bad_request(format!("workflow id must be 1–{MAX_WORKFLOW_ID_CHARS} characters"));
    }
    let target = token_id.clone();
    let wf     = workflow_id.clone();
    match s.with_token_store(move |store| store.acl_grant(&target, &wf)).await {
        Ok(AclGrant::Granted) => (StatusCode::CREATED, Json(json!({
            "ok":          true,
            "token_id":    token_id,
            "workflow_id": workflow_id,
            "note":        "Token is now restricted to its granted workflow(s)."
        }))).into_response(),
        Ok(AclGrant::NoActiveToken) => (StatusCode::NOT_FOUND,
            Json(json!({"error": "no active token with that id"}))).into_response(),
        Ok(AclGrant::AdminToken) => bad_request("admin tokens are never restricted to specific workflows"),
        Err(e) => internal_error("grant token workflow failed", e).into_response(),
    }
}

/// DELETE /api/tokens/:id/workflows/:wf_id — revoke access to a workflow.
/// `409` when it is the token's only grant: a token with no grants is
/// unrestricted, so removing it would widen access. Revoke the token instead.
/// Revoking a grant the token does not hold is a no-op `200`.
pub async fn revoke_token_workflow_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path((token_id, workflow_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    match s.with_token_store(move |store| store.acl_revoke(&token_id, &workflow_id)).await {
        Ok(AclRevoke::Revoked | AclRevoke::NotGranted) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Ok(AclRevoke::LastGrant) => (StatusCode::CONFLICT, Json(json!({
            "error": "this is the token's last workflow grant; removing it would make the token unrestricted. Revoke the token instead (DELETE /api/tokens/:id)"
        }))).into_response(),
        Err(e) => internal_error("revoke token workflow failed", e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::{admin_with_grants, expiry_error, valid_workflow_id, MAX_WORKFLOW_ID_CHARS};

    #[test]
    fn admin_scope_cannot_be_combined_with_workflow_grants() {
        let ids = vec!["wf_1".to_string()];
        assert!(admin_with_grants(&["admin".to_string()], &ids));
        assert!(!admin_with_grants(&["admin".to_string()], &[]));
        assert!(!admin_with_grants(&["read".to_string(), "write".to_string()], &ids));
    }

    #[test]
    fn workflow_id_must_be_non_blank_and_bounded() {
        assert!(valid_workflow_id("wf_1"));
        assert!(!valid_workflow_id(""));
        assert!(!valid_workflow_id("   "));
        assert!(valid_workflow_id(&"\u{e9}".repeat(MAX_WORKFLOW_ID_CHARS)));
        assert!(!valid_workflow_id(&"a".repeat(MAX_WORKFLOW_ID_CHARS + 1)));
    }

    #[test]
    fn token_lifetime_must_be_positive_and_representable() {
        assert!(expiry_error(3600).is_none());
        assert!(expiry_error(0).is_some());
        assert!(expiry_error(u64::MAX).is_some());
    }
}
