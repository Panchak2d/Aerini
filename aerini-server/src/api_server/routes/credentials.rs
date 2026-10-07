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
use super::state::{ApiState, internal_error, require_read, require_unrestricted, require_write};

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

/// Validates a save-credential request, returning the `400` message on failure.
fn validate_cred(b: &CredBody) -> Result<(), String> {
    for (field, v) in [("id", &b.id), ("name", &b.name)] {
        if v.trim().is_empty() || v.chars().count() > MAX_FIELD_CHARS {
            return Err(format!("{field} must be 1–{MAX_FIELD_CHARS} characters"));
        }
    }
    if b.value.is_empty() {
        return Err("value must not be empty".to_string());
    }
    Ok(())
}

pub async fn save_cred(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Json(b):           Json<CredBody>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    if let Err(e) = require_unrestricted(&s, &caller).await { return e.into_response(); }
    if let Err(msg) = validate_cred(&b) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    }
    let creds = Arc::clone(&s.creds);
    let req = CreateCredentialRequest {
        id: b.id, name: b.name, value: b.value, cred_type: b.cred_type,
        provider: None, model: None, base_url: None,
    };
    match tokio::task::spawn_blocking(move || creds.store(&req).map_err(|e| e.to_string())).await {
        Ok(Ok(()))  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Ok(Err(e))  => internal_error("save_cred", e).into_response(),
        Err(e)      => internal_error("save_cred: task failed", e).into_response(),
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
        Ok(Err(e))   => internal_error("list_creds", e).into_response(),
        Err(e)       => internal_error("list_creds: task failed", e).into_response(),
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
        Ok(Err(e))  => internal_error("delete_cred", e).into_response(),
        Err(e)      => internal_error("delete_cred: task failed", e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> CredBody {
        CredBody { id: "k".into(), name: "Key".into(), value: "secret".into(), cred_type: "api_key".into() }
    }

    #[test]
    fn valid_credential_passes() {
        assert!(validate_cred(&body()).is_ok());
    }

    #[test]
    fn blank_id_or_name_is_rejected() {
        let mut b = body();
        b.id = " ".into();
        assert!(validate_cred(&b).is_err());
        let mut b = body();
        b.name = String::new();
        assert!(validate_cred(&b).is_err());
    }

    #[test]
    fn empty_value_is_rejected() {
        let mut b = body();
        b.value = String::new();
        assert!(validate_cred(&b).is_err());
    }

    #[test]
    fn id_and_name_are_capped_at_256_chars() {
        let mut b = body();
        b.id = "a".repeat(MAX_FIELD_CHARS);
        assert!(validate_cred(&b).is_ok());
        b.id = "a".repeat(MAX_FIELD_CHARS + 1);
        assert!(validate_cred(&b).is_err());
        let mut b = body();
        b.name = "n".repeat(MAX_FIELD_CHARS + 1);
        assert!(validate_cred(&b).is_err());
    }
}
