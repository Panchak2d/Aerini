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
use super::state::{ApiState, require_read, require_unrestricted, require_write};

#[derive(Deserialize)]
pub struct CredBody {
    pub id: String,
    pub name: String,
    pub value: String,
    #[serde(default = "default_cred_type")]
    pub cred_type: String,
}

fn default_cred_type() -> String { "api_key".to_string() }

pub async fn save_cred(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Json(b):           Json<CredBody>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    if let Err(e) = require_unrestricted(&s, &caller).await { return e.into_response(); }
    let creds = Arc::clone(&s.creds);
    let req = CreateCredentialRequest {
        id: b.id, name: b.name, value: b.value, cred_type: b.cred_type,
        provider: None, model: None, base_url: None,
    };
    match tokio::task::spawn_blocking(move || creds.store(&req).map_err(|e| e.to_string())).await {
        Ok(Ok(()))  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Ok(Err(e))  => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)      => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
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
        Ok(Err(e))   => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)       => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
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
        Ok(Err(e))  => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)      => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}
