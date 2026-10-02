//! Credential CRUD routes. Credentials are server-wide, so a token restricted
//! to specific workflows may not use these routes.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use aerini_engine::store::CreateCredentialRequest;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::token_store::TokenRecord;
use super::state::{internal_error, ApiState, require_read, require_unrestricted, require_write};

#[derive(Deserialize)]
pub struct CredBody {
    pub id: String,
    pub name: String,
    pub value: String,
    #[serde(default = "default_cred_type")]
    pub cred_type: String,
}

fn default_cred_type() -> String { "api_key".to_string() }

const MAX_FIELD_CHARS: usize = 256;

fn invalid_cred_field(b: &CredBody) -> Option<&'static str> {
    let ok = |v: &str| !v.trim().is_empty() && v.chars().count() <= MAX_FIELD_CHARS;
    if !ok(&b.id)   { return Some("id must be 1–256 characters"); }
    if !ok(&b.name) { return Some("name must be 1–256 characters"); }
    if b.value.is_empty() { return Some("value must not be empty"); }
    None
}

pub async fn save_cred(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Json(b):           Json<CredBody>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    if let Err(e) = require_unrestricted(&s, &caller).await { return e.into_response(); }
    if let Some(msg) = invalid_cred_field(&b) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    }
    let creds = Arc::clone(&s.creds);
    let req = CreateCredentialRequest {
        id: b.id, name: b.name, value: b.value, cred_type: b.cred_type,
        provider: None, model: None, base_url: None,
    };
    match tokio::task::spawn_blocking(move || creds.store(&req).map_err(|e| e.to_string())).await {
        Ok(Ok(()))  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Ok(Err(e))  => internal_error("credential operation failed", e).into_response(),
        Err(e)      => internal_error("credential task failed", e).into_response(),
    }
}

pub async fn list_creds(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }
    if let Err(e) = require_unrestricted(&s, &caller).await { return e.into_response(); }
    let creds = Arc::clone(&s.creds);
    match tokio::task::spawn_blocking(move || creds.list().map_err(|e| e.to_string())).await {
        Ok(Ok(list)) => (StatusCode::OK, Json(json!(list))).into_response(),
        Ok(Err(e))   => internal_error("credential operation failed", e).into_response(),
        Err(e)       => internal_error("credential task failed", e).into_response(),
    }
}

pub async fn delete_cred(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    if let Err(e) = require_unrestricted(&s, &caller).await { return e.into_response(); }
    let creds = Arc::clone(&s.creds);
    match tokio::task::spawn_blocking(move || creds.delete(&id).map_err(|e| e.to_string())).await {
        Ok(Ok(()))  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Ok(Err(e))  => internal_error("credential operation failed", e).into_response(),
        Err(e)      => internal_error("credential task failed", e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(id: &str, name: &str, value: &str) -> CredBody {
        CredBody { id: id.into(), name: name.into(), value: value.into(), cred_type: "api_key".into() }
    }

    #[test]
    fn credential_fields_are_validated() {
        assert!(invalid_cred_field(&body("openai", "OpenAI", "sk-1")).is_none());
        assert!(invalid_cred_field(&body(" ", "OpenAI", "sk-1")).is_some());
        assert!(invalid_cred_field(&body("openai", "", "sk-1")).is_some());
        assert!(invalid_cred_field(&body("openai", "OpenAI", "")).is_some());
        assert!(invalid_cred_field(&body(&"a".repeat(257), "OpenAI", "sk-1")).is_some());
    }
}
