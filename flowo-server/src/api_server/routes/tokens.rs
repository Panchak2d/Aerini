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
    match s.token_store.list_tokens() {
        Ok(tokens) => (StatusCode::OK, Json(json!(tokens))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct CreateTokenBody {
    pub label:  String,
    #[serde(default = "default_token_scopes")]
    pub scopes: Vec<String>,
    pub expires_in_secs: Option<u64>,
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
    let invalid: Vec<&str> = b.scopes.iter()
        .map(|s| s.as_str())
        .filter(|s| !VALID_SCOPES.contains(s))
        .collect();
    if !invalid.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": format!("Invalid scopes: {:?}. Allowed: read, write, admin", invalid)
        }))).into_response();
    }
    let scopes_ref: Vec<&str> = b.scopes.iter().map(|s| s.as_str()).collect();
    match s.token_store.create_token(&b.label, &scopes_ref, b.expires_in_secs) {
        Ok(raw) => (StatusCode::CREATED, Json(json!({
            "token":      raw,
            "label":      b.label,
            "scopes":     b.scopes,
            "expires_in_secs": b.expires_in_secs,
            "note":       "Save this token — it will not be shown again."
        }))).into_response(),
        Err(e)  => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

pub async fn revoke_token_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    match s.token_store.revoke_token(&id) {
        Ok(()) => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}
