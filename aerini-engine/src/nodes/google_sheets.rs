use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;
use crate::nodes::oauth_listener;

pub struct GoogleSheetsNode;

#[async_trait]
impl Node for GoogleSheetsNode {
    fn type_id(&self) -> &'static str { "google_sheets" }
    fn display_name(&self) -> &'static str { "Google Sheets" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Read from or write to a Google Sheets spreadsheet using the Sheets API." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action", "spreadsheet_id", "range"],
            "properties": {
                "action":         { "type": "string", "enum": ["append_row", "get_values"], "description": "Operation to perform" },
                "spreadsheet_id": { "type": "string", "description": "Google Sheets spreadsheet ID (from the URL)" },
                "range":          { "type": "string", "description": "A1 notation range (e.g. Sheet1!A1:D1)" },
                "values":         { "description": "Row data for append_row — array of arrays, e.g. [[\"a\",\"b\"]]" },
                "client_id":      { "type": "string", "description": "Google OAuth client ID — enables automatic token refresh (recommended). Entered directly here; not currently offered as a saved-credential picker field." },
                "client_secret":  { "type": "string", "description": "Google OAuth client secret — enables automatic token refresh (recommended). Entered directly here; not currently offered as a saved-credential picker field." },
                "api_key":        { "type": "string", "description": "Google OAuth 2.0 access token, pasted directly. Does not auto-refresh (expires after ~1 hour). Only used when client_id/client_secret are not set." }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "values":          { "description": "Cell values returned by get_values" },
                "updatedRange":    { "type": "string" },
                "updatedRows":     { "type": "number" },
                "updatedColumns":  { "type": "number" },
                "updatedCells":    { "type": "number" }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let access_token = match Self::resolve_access_token(&input.input).await {
            Ok(t) => t,
            Err(e) => return NodeOutput::failure(e),
        };

        let action         = input.input["action"].as_str().unwrap_or("get_values");
        let spreadsheet_id = match input.input["spreadsheet_id"].as_str().filter(|s| !s.is_empty()) {
            Some(id) => id.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_SPREADSHEET_ID", "spreadsheet_id is required")),
        };
        let range = match input.input["range"].as_str().filter(|s| !s.is_empty()) {
            Some(r) => r.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_RANGE", "range is required (e.g. Sheet1!A1:D1)")),
        };

        match action {
            "append_row" => {
                let values = match input.input.get("values") {
                    Some(v) if v.is_array() => v.clone(),
                    _ => return NodeOutput::failure(NodeError::unrecoverable(
                        "MISSING_VALUES",
                        "values must be an array of arrays for append_row (e.g. [[\"col1\",\"col2\"]])",
                    )),
                };

                let url = {
                    let mut u = url::Url::parse("https://sheets.googleapis.com/v4/spreadsheets/").expect("hardcoded valid https URL");
                    u.path_segments_mut().expect("https URL is never cannot-be-a-base")
                        .push(&spreadsheet_id)
                        .push("values")
                        .push(&format!("{}:append", range));
                    u.set_query(Some("valueInputOption=RAW"));
                    u.to_string()
                };

                let body = json!({ "values": values });

                match super::shared_http_client()
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", access_token))
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        match super::util::read_json_response_capped(resp).await {
                            Ok(v) if status == 200 => {
                                let updated_range = v["updates"]["updatedRange"]
                                    .as_str()
                                    .unwrap_or(&range)
                                    .to_string();
                                let updated_cells = v["updates"]["updatedCells"].as_u64().unwrap_or(0);
                                NodeOutput::success_with_logs(
                                    v["updates"].clone(),
                                    vec![format!("Appended {} cell(s) to {}", updated_cells, updated_range)],
                                )
                            }
                            Ok(v) => {
                                let msg = v["error"]["message"].as_str().unwrap_or("unknown error").to_string();
                                NodeOutput::failure(super::util::provider_error(status, "SHEETS_ERROR", format!("HTTP {}: {}", status, msg)))
                            }
                            Err(e) => NodeOutput::failure(super::util::provider_error(status, "PARSE_ERROR", e)),
                        }
                    }
                    Err(e) => {
                        super::util::http_err_output(&e)
                    }
                }
            }
            "get_values" => {
                let url = {
                    let mut u = url::Url::parse("https://sheets.googleapis.com/v4/spreadsheets/").expect("hardcoded valid https URL");
                    u.path_segments_mut().expect("https URL is never cannot-be-a-base")
                        .push(&spreadsheet_id)
                        .push("values")
                        .push(&range);
                    u.to_string()
                };

                match super::shared_http_client()
                    .get(&url)
                    .header("Authorization", format!("Bearer {}", access_token))
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        match super::util::read_json_response_capped(resp).await {
                            Ok(v) if status == 200 => {
                                let row_count = v["values"].as_array().map(|a| a.len()).unwrap_or(0);
                                NodeOutput::success_with_logs(
                                    v,
                                    vec![format!("Retrieved {} row(s) from {}", row_count, range)],
                                )
                            }
                            Ok(v) => {
                                let msg = v["error"]["message"].as_str().unwrap_or("unknown error").to_string();
                                NodeOutput::failure(super::util::provider_error(status, "SHEETS_ERROR", format!("HTTP {}: {}", status, msg)))
                            }
                            Err(e) => NodeOutput::failure(super::util::provider_error(status, "PARSE_ERROR", e)),
                        }
                    }
                    Err(e) => {
                        super::util::http_err_output(&e)
                    }
                }
            }
            other => NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_ACTION",
                format!("Unknown action '{}'. Valid values: append_row, get_values", other),
            )),
        }
    }
}

impl GoogleSheetsNode {
    /// Resolves a valid Google OAuth access token for Sheets access.
    ///
    /// Prefers `client_id`/`client_secret` (OAuth app credentials): the token is
    /// then obtained via `oauth_listener::get_tokens`, which transparently stores,
    /// checks expiry, and refreshes under a per-credential mutex — the same
    /// mechanism `SocialUploadNode` already uses for YouTube/Instagram/TikTok.
    /// Falls back to a raw, caller-supplied `api_key` (this
    /// node's original behavior) when `client_id`/`client_secret` are absent, so
    /// existing workflows configured with a manually-pasted access token keep
    /// working unchanged — no breaking change.
    async fn resolve_access_token(cfg: &Value) -> Result<String, NodeError> {
        let client_id = cfg["client_id"].as_str().filter(|s| !s.is_empty());
        let client_secret = cfg["client_secret"].as_str().filter(|s| !s.is_empty());

        match (client_id, client_secret) {
            (Some(id), Some(secret)) => {
                oauth_listener::get_tokens("google_sheets", id, secret)
                    .await
                    .map(|t| t.access_token)
            }
            (None, None) => cfg["api_key"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| NodeError::unrecoverable(
                    "MISSING_TOKEN",
                    "Google OAuth credentials required — set client_id/client_secret (auto-refreshing, recommended) or a raw api_key access token.",
                )),
            // Exactly one of client_id/client_secret set: a real misconfiguration
            // (e.g. secret not pasted yet) — must not silently fall back to a
            // possibly-stale api_key and mask it.
            _ => Err(NodeError::unrecoverable(
                "INCOMPLETE_OAUTH_CONFIG",
                "client_id and client_secret must both be set to enable auto-refresh — set both, or neither (to use api_key instead).",
            )),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────
//
// The client_id+client_secret path is a direct passthrough to
// `oauth_listener::get_tokens`, which already owns its own keychain/refresh/
// full-flow logic and is not itself re-tested here — verified by manual
// trace only, since exercising it would require mocking a real OS keychain.
// What's cheaply testable without any mocking — which of the two paths
// `resolve_access_token` picks, and its two failure cases — is covered
// below.
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_access_token_falls_back_to_api_key_when_no_oauth_creds() {
        let cfg = json!({ "api_key": "raw-token-123" });
        let token = GoogleSheetsNode::resolve_access_token(&cfg).await.unwrap();
        assert_eq!(token, "raw-token-123");
    }

    #[tokio::test]
    async fn resolve_access_token_ignores_empty_string_oauth_creds() {
        // Empty client_id/client_secret must not be treated as "present" —
        // otherwise a config with blank fields would try (and fail) the OAuth
        // path instead of correctly falling back to api_key.
        let cfg = json!({ "client_id": "", "client_secret": "", "api_key": "raw-token-123" });
        let token = GoogleSheetsNode::resolve_access_token(&cfg).await.unwrap();
        assert_eq!(token, "raw-token-123");
    }

    #[tokio::test]
    async fn resolve_access_token_errors_when_nothing_provided() {
        let cfg = json!({});
        let err = GoogleSheetsNode::resolve_access_token(&cfg).await.unwrap_err();
        assert_eq!(err.code, "MISSING_TOKEN");
        assert!(!err.recoverable);
    }

    #[tokio::test]
    async fn resolve_access_token_errors_when_only_client_id_given() {
        // Partial OAuth config (id without secret) must error loudly, not
        // silently fall back to api_key — that would mask a real misconfiguration
        // (e.g. secret not pasted yet) behind a possibly-stale manual token.
        let cfg = json!({ "client_id": "abc", "api_key": "raw-token-123" });
        let err = GoogleSheetsNode::resolve_access_token(&cfg).await.unwrap_err();
        assert_eq!(err.code, "INCOMPLETE_OAUTH_CONFIG");
    }
}

