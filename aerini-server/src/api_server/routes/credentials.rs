//! Credential CRUD routes. Credentials are server-wide, so a token restricted
//! to specific workflows may not use these routes.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use aerini_engine::{error::EngineError, executor::SecretString, store::CreateCredentialRequest};
use serde::{Deserialize, Deserializer};
use serde_json::json;
use std::sync::Arc;

use crate::token_store::TokenRecord;
use super::state::{ApiState, internal_error, require_read, require_unrestricted, require_write};

#[derive(Deserialize)]
pub struct CredBody {
    pub id: String,
    pub name: String,
    #[serde(deserialize_with = "secret_string")]
    pub value: SecretString,
    #[serde(default = "default_cred_type")]
    pub cred_type: String,
    #[serde(default)]
    pub update: bool,
}

fn default_cred_type() -> String { "api_key".to_string() }

fn secret_string<'de, D: Deserializer<'de>>(d: D) -> Result<SecretString, D::Error> {
    String::deserialize(d).map(SecretString::new)
}

/// Maps a store failure to its HTTP response. Validation messages never carry
/// a submitted value; anything unexpected is logged and hidden.
fn store_error(context: &str, e: EngineError) -> Response {
    match e {
        EngineError::CredentialInvalid(msg) => {
            (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
        }
        EngineError::CredentialExists(_) => (
            StatusCode::CONFLICT,
            Json(json!({"error": "credential already exists; send \"update\": true to replace it"})),
        ).into_response(),
        EngineError::CredentialNotFound(_) => {
            (StatusCode::NOT_FOUND, Json(json!({"error": "credential not found"}))).into_response()
        }
        other => internal_error(context, other).into_response(),
    }
}

pub async fn save_cred(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Json(mut b):       Json<CredBody>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    if let Err(e) = require_unrestricted(&s, &caller).await { return e.into_response(); }
    let creds = Arc::clone(&s.creds);
    let update = b.update;
    let req = CreateCredentialRequest {
        id: b.id, name: b.name, value: std::mem::take(&mut *b.value), cred_type: b.cred_type,
        provider: None, model: None, base_url: None,
    };
    let saved = tokio::task::spawn_blocking(move || {
        if update { creds.update(&req) } else { creds.create(&req) }
    }).await;
    match saved {
        Ok(Ok(()))  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Ok(Err(e))  => store_error("save_cred", e),
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
    match tokio::task::spawn_blocking(move || creds.delete(&id)).await {
        Ok(Ok(()))  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Ok(Err(e))  => store_error("delete_cred", e),
        Err(e)      => internal_error("delete_cred: task failed", e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_errors_map_to_their_http_statuses() {
        let status = |e: EngineError| store_error("t", e).status();
        assert_eq!(status(EngineError::CredentialInvalid("bad".into())), StatusCode::BAD_REQUEST);
        assert_eq!(status(EngineError::CredentialExists("k".into())), StatusCode::CONFLICT);
        assert_eq!(status(EngineError::CredentialNotFound("k".into())), StatusCode::NOT_FOUND);
        assert_eq!(status(EngineError::Database("locked".into())), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn update_flag_defaults_to_false() {
        let b: CredBody = serde_json::from_str(r#"{"id":"k","name":"Key","value":"v"}"#).unwrap();
        assert!(!b.update);
        assert_eq!(b.value.as_str(), "v");
        assert_eq!(b.cred_type, "api_key");
    }
}
