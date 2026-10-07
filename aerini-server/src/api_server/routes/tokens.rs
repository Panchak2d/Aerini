//! Token management routes (admin scope required).

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::token_store::{GrantOutcome, RevokeOutcome, TokenRecord, UngrantOutcome};
use super::state::{ApiState, internal_error, require_admin};

pub async fn list_tokens_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    match s.with_token_store(|store| store.list_tokens()).await {
        Ok(tokens) => (StatusCode::OK, Json(json!(tokens))).into_response(),
        Err(e)     => internal_error("list_tokens_handler", e).into_response(),
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
const MAX_WORKFLOW_IDS: usize = 500;
const MAX_WORKFLOW_ID_CHARS: usize = 256;

/// Validates a create-token request, returning the `400` message on failure.
fn validate_create_token(b: &CreateTokenBody) -> Result<(), String> {
    let label_chars = b.label.chars().count();
    if b.label.trim().is_empty() || label_chars > MAX_LABEL_CHARS {
        return Err(format!("label must be 1–{MAX_LABEL_CHARS} characters"));
    }
    if b.scopes.is_empty() {
        return Err("scopes must not be empty. Allowed: read, write, admin".to_string());
    }
    let invalid: Vec<&str> = b.scopes.iter()
        .map(|s| s.as_str())
        .filter(|s| !VALID_SCOPES.contains(s))
        .collect();
    if !invalid.is_empty() {
        return Err(format!("Invalid scopes: {:?}. Allowed: read, write, admin", invalid));
    }
    if !b.workflow_ids.is_empty() && b.scopes.iter().any(|s| s == "admin") {
        return Err("admin tokens ignore the workflow ACL; drop `admin` to restrict the token, or drop `workflow_ids`".to_string());
    }
    if b.workflow_ids.len() > MAX_WORKFLOW_IDS {
        return Err(format!("workflow_ids must have at most {MAX_WORKFLOW_IDS} entries"));
    }
    if b.workflow_ids.iter().any(|w| w.trim().is_empty() || w.chars().count() > MAX_WORKFLOW_ID_CHARS) {
        return Err(format!("each workflow id must be 1–{MAX_WORKFLOW_ID_CHARS} characters"));
    }
    match b.expires_in_secs {
        Some(0) => return Err("expires_in_secs must be greater than 0".to_string()),
        Some(secs) => {
            let representable = i64::try_from(secs).ok()
                .and_then(chrono::TimeDelta::try_seconds)
                .and_then(|d| chrono::Utc::now().checked_add_signed(d));
            if representable.is_none() {
                return Err("expires_in_secs is too large".to_string());
            }
        }
        None => {}
    }
    Ok(())
}

pub async fn create_token_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Json(b):           Json<CreateTokenBody>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    if let Err(msg) = validate_create_token(&b) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
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
        Err(e)   => return internal_error("create_token_handler", e).into_response(),
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
        Ok(RevokeOutcome::Revoked | RevokeOutcome::AlreadyRevoked) =>
            (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Ok(RevokeOutcome::UnknownToken) =>
            (StatusCode::NOT_FOUND, Json(json!({"error": "token not found"}))).into_response(),
        Err(e) => internal_error("revoke_token_handler", e).into_response(),
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
        Err(e) => internal_error("list_token_workflows_handler", e).into_response(),
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
    if workflow_id.trim().is_empty() || workflow_id.chars().count() > MAX_WORKFLOW_ID_CHARS {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": format!("workflow id must be 1–{MAX_WORKFLOW_ID_CHARS} characters")
        }))).into_response();
    }
    match s.with_token_store(move |store| store.acl_grant_checked(&target, &wf)).await {
        Ok(GrantOutcome::Granted) => (StatusCode::CREATED, Json(json!({
            "ok":          true,
            "token_id":    token_id,
            "workflow_id": workflow_id,
            "note":        "Token is now restricted to its granted workflow(s)."
        }))).into_response(),
        Ok(GrantOutcome::UnknownToken) =>
            (StatusCode::NOT_FOUND, Json(json!({"error": "token not found"}))).into_response(),
        Ok(GrantOutcome::AdminToken) => (StatusCode::BAD_REQUEST, Json(json!({
            "error": "admin tokens ignore the workflow ACL; a grant would have no effect"
        }))).into_response(),
        Err(e) => internal_error("grant_token_workflow_handler", e).into_response(),
    }
}

/// DELETE /api/tokens/:id/workflows/:wf_id — revoke access to a workflow.
/// Refuses (409) to remove a token's only grant, since that would leave it
/// unrestricted; revoke the token itself to cut it off.
pub async fn revoke_token_workflow_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path((token_id, workflow_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    let result = s.with_token_store(move |store| {
        store.acl_revoke_unless_last(&token_id, &workflow_id)
    }).await;
    match result {
        Ok(UngrantOutcome::Removed | UngrantOutcome::NotGranted) =>
            (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Ok(UngrantOutcome::LastGrant) => (StatusCode::CONFLICT, Json(json!({
            "error": "cannot remove the token's only workflow grant: the token would become unrestricted. Revoke the token to cut it off, or create a new token without workflow_ids for an unrestricted one"
        }))).into_response(),
        Err(e) => internal_error("revoke_token_workflow_handler", e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> CreateTokenBody {
        CreateTokenBody {
            label: "ci".to_string(),
            scopes: vec!["read".to_string()],
            expires_in_secs: None,
            workflow_ids: vec![],
        }
    }

    #[test]
    fn valid_create_body_passes() {
        assert!(validate_create_token(&body()).is_ok());
    }

    #[test]
    fn blank_or_overlong_label_is_rejected() {
        let mut b = body();
        b.label = "   ".to_string();
        assert!(validate_create_token(&b).is_err());
        b.label = "é".repeat(MAX_LABEL_CHARS);
        assert!(validate_create_token(&b).is_ok());
        b.label = "é".repeat(MAX_LABEL_CHARS + 1);
        assert!(validate_create_token(&b).is_err());
    }

    #[test]
    fn empty_scopes_are_rejected() {
        let mut b = body();
        b.scopes.clear();
        assert!(validate_create_token(&b).is_err());
    }

    #[test]
    fn admin_with_workflow_ids_is_rejected() {
        let mut b = body();
        b.scopes = vec!["admin".to_string()];
        b.workflow_ids = vec!["wf_a".to_string()];
        assert!(validate_create_token(&b).is_err());
        b.workflow_ids.clear();
        assert!(validate_create_token(&b).is_ok());
    }

    #[test]
    fn workflow_id_limits_are_enforced() {
        let mut b = body();
        b.workflow_ids = vec!["w".to_string(); MAX_WORKFLOW_IDS];
        assert!(validate_create_token(&b).is_ok());
        b.workflow_ids.push("w".to_string());
        assert!(validate_create_token(&b).is_err());
        b.workflow_ids = vec!["w".repeat(MAX_WORKFLOW_ID_CHARS + 1)];
        assert!(validate_create_token(&b).is_err());
        b.workflow_ids = vec![" ".to_string()];
        assert!(validate_create_token(&b).is_err());
    }

    #[test]
    fn zero_and_unrepresentable_expiry_are_rejected() {
        let mut b = body();
        b.expires_in_secs = Some(0);
        assert!(validate_create_token(&b).is_err());
        b.expires_in_secs = Some(u64::MAX);
        assert!(validate_create_token(&b).is_err());
        b.expires_in_secs = Some(3600);
        assert!(validate_create_token(&b).is_ok());
    }
}
