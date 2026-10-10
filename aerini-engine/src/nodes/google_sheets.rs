use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;
use crate::nodes::oauth_listener;

pub struct GoogleSheetsNode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action { AppendRow, GetValues }

#[derive(Debug)]
struct Request {
    action: Action,
    spreadsheet_id: String,
    range: String,
    values: Option<Value>,
    value_input: &'static str,
}

fn value_input_option(v: &Value) -> Result<&'static str, NodeError> {
    let bad = || NodeError::unrecoverable("INVALID_CONFIG", "value_input must be RAW or USER_ENTERED");
    match v {
        Value::Null => Ok("RAW"),
        Value::String(s) => match s.trim().to_ascii_uppercase().as_str() {
            "" | "RAW" => Ok("RAW"),
            "USER_ENTERED" => Ok("USER_ENTERED"),
            _ => Err(bad()),
        },
        _ => Err(bad()),
    }
}

fn id_error() -> NodeError {
    NodeError::unrecoverable(
        "INVALID_SPREADSHEET_ID",
        "spreadsheet_id must be the ID from the sheet's address (the part after /spreadsheets/d/) or the full https://docs.google.com/spreadsheets/d/... address",
    )
}

fn spreadsheet_id_from(raw: &str) -> Result<String, NodeError> {
    let id = if raw.contains("://") {
        let url = url::Url::parse(raw).map_err(|_| id_error())?;
        if url.scheme() != "https" || url.host_str() != Some("docs.google.com") {
            return Err(id_error());
        }
        let segs: Vec<&str> = url.path_segments().map(|s| s.collect()).unwrap_or_default();
        if segs.first() != Some(&"spreadsheets") {
            return Err(id_error());
        }
        segs.iter().position(|s| *s == "d")
            .and_then(|i| segs.get(i + 1))
            .map(|s| s.to_string())
            .ok_or_else(id_error)?
    } else {
        raw.to_string()
    };
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(id_error());
    }
    Ok(id)
}

fn values_shape_error(detail: String) -> NodeError {
    NodeError::unrecoverable(
        "INVALID_VALUES_SHAPE",
        format!("{detail}; values must be rows of cells, e.g. [[\"a\",\"b\"],[\"c\",\"d\"]]"),
    )
}

/// Accepts rows as a JSON array or as JSON text (what the UI stores).
fn parse_rows(raw: Option<&Value>) -> Result<Value, NodeError> {
    let missing = || NodeError::unrecoverable(
        "MISSING_VALUES",
        "values is required for append_row (e.g. [[\"col1\",\"col2\"]])",
    );
    let parsed;
    let v = match raw {
        None | Some(Value::Null) => return Err(missing()),
        Some(Value::String(s)) => {
            if s.trim().is_empty() {
                return Err(missing());
            }
            parsed = serde_json::from_str::<Value>(s).map_err(|e| NodeError::unrecoverable(
                "INVALID_VALUES_JSON",
                format!("values is not valid JSON ({e}); expected rows such as [[\"a\",\"b\"]]"),
            ))?;
            &parsed
        }
        Some(other) => other,
    };
    let rows = match v {
        Value::Array(rows) if !rows.is_empty() => rows,
        Value::Array(_) => return Err(values_shape_error("values has no rows".into())),
        _ => return Err(values_shape_error("values is not an array".into())),
    };
    for (i, row) in rows.iter().enumerate() {
        let n = i + 1;
        match row {
            Value::Array(cells) if cells.is_empty() => {
                return Err(values_shape_error(format!("row {n} has no cells")));
            }
            Value::Array(cells) => {
                if cells.iter().any(|c| c.is_array() || c.is_object()) {
                    return Err(values_shape_error(format!("row {n} has a cell that is an array or object")));
                }
            }
            _ => return Err(values_shape_error(format!("row {n} is not an array"))),
        }
    }
    Ok(v.clone())
}

/// Checks everything the node can check without the network, before any sign-in.
fn validate(cfg: &Value) -> Result<Request, NodeError> {
    let action = match cfg["action"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some("append_row") => Action::AppendRow,
        Some("get_values") => Action::GetValues,
        None => return Err(NodeError::unrecoverable(
            "MISSING_ACTION",
            "action is required. Valid values: append_row, get_values",
        )),
        Some(other) => return Err(NodeError::unrecoverable(
            "INVALID_ACTION",
            format!("Unknown action '{}'. Valid values: append_row, get_values", other),
        )),
    };
    let raw_id = cfg["spreadsheet_id"].as_str().map(str::trim).filter(|s| !s.is_empty())
        .ok_or_else(|| NodeError::unrecoverable("MISSING_SPREADSHEET_ID", "spreadsheet_id is required"))?;
    let spreadsheet_id = spreadsheet_id_from(raw_id)?;
    let range = cfg["range"].as_str().map(str::trim).filter(|s| !s.is_empty())
        .ok_or_else(|| NodeError::unrecoverable("MISSING_RANGE", "range is required (e.g. Sheet1!A1:D1)"))?
        .to_string();
    let (values, value_input) = match action {
        Action::AppendRow => (Some(parse_rows(cfg.get("values"))?), value_input_option(&cfg["value_input"])?),
        Action::GetValues => (None, "RAW"),
    };
    Ok(Request { action, spreadsheet_id, range, values, value_input })
}

fn append_result(v: Value, range: &str) -> NodeOutput {
    match v.get("updates").filter(|u| u.is_object()) {
        Some(updates) => {
            let cells = updates["updatedCells"].as_u64().unwrap_or(0);
            let updated_range = updates["updatedRange"].as_str().unwrap_or(range).to_string();
            NodeOutput::success_with_logs(
                updates.clone(),
                vec![format!("Appended {} cell(s) to {}", cells, updated_range)],
            )
        }
        None => NodeOutput::success_with_logs(
            v,
            vec![format!("Appended to {}; Google returned no update summary", range)],
        ),
    }
}

fn get_values_result(mut v: Value, range: &str) -> NodeOutput {
    if !v["values"].is_array() {
        if let Some(obj) = v.as_object_mut() {
            obj.insert("values".to_string(), json!([]));
        }
    }
    let rows = v["values"].as_array().map(|a| a.len()).unwrap_or(0);
    NodeOutput::success_with_logs(v, vec![format!("Retrieved {} row(s) from {}", rows, range)])
}

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
                "spreadsheet_id": { "type": "string", "description": "Google Sheets spreadsheet ID (from the URL); the full sheet address also works" },
                "range":          { "type": "string", "description": "A1 notation range (e.g. Sheet1!A1:D1)" },
                "values":         { "x-aerini-multiline": true, "description": "Row data for append_row — rows of cells as JSON, e.g. [[\"a\",\"b\"]]. Stored as typed (RAW) unless value_input is USER_ENTERED." },
                "value_input":    { "type": "string", "enum": ["RAW", "USER_ENTERED"], "description": "append_row only. RAW (default) stores values as typed. USER_ENTERED parses them as if typed in the sheet: formulas run, dates and numbers are converted. Do not use USER_ENTERED with data you do not control: a value starting with = becomes a formula." },
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
        let req = match validate(&input.input) {
            Ok(r) => r,
            Err(e) => return NodeOutput::failure(e),
        };
        let access_token = match Self::resolve_access_token(&input.input).await {
            Ok(t) => t,
            Err(e) => return NodeOutput::failure(e),
        };

        match req.action {
            Action::AppendRow => {
                let url = {
                    let mut u = url::Url::parse("https://sheets.googleapis.com/v4/spreadsheets/").expect("hardcoded valid https URL");
                    u.path_segments_mut().expect("https URL is never cannot-be-a-base")
                        .push(&req.spreadsheet_id)
                        .push("values")
                        .push(&format!("{}:append", req.range));
                    u.set_query(Some(format!("valueInputOption={}&insertDataOption=INSERT_ROWS", req.value_input).as_str()));
                    u.to_string()
                };

                let body = json!({ "values": req.values });

                match super::shared_http_client()
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", access_token))
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        if status == 401 {
                            Self::discard_rejected_tokens(&input.input).await;
                        }
                        match super::util::read_json_response_capped(resp).await {
                            Ok(v) if status == 200 => append_result(v, &req.range),
                            Ok(v) => {
                                let msg = v["error"]["message"].as_str().unwrap_or("unknown error").to_string();
                                NodeOutput::failure(super::util::provider_error(status, "SHEETS_ERROR", format!("HTTP {}: {}", status, msg)))
                            }
                            Err(e) => NodeOutput::failure(super::util::provider_error(status, "PARSE_ERROR", e)),
                        }
                    }
                    Err(e) => {
                        super::util::http_err_output(super::util::Replay::Never, &e)
                    }
                }
            }
            Action::GetValues => {
                let url = {
                    let mut u = url::Url::parse("https://sheets.googleapis.com/v4/spreadsheets/").expect("hardcoded valid https URL");
                    u.path_segments_mut().expect("https URL is never cannot-be-a-base")
                        .push(&req.spreadsheet_id)
                        .push("values")
                        .push(&req.range);
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
                        if status == 401 {
                            Self::discard_rejected_tokens(&input.input).await;
                        }
                        match super::util::read_json_response_capped(resp).await {
                            Ok(v) if status == 200 => get_values_result(v, &req.range),
                            Ok(v) => {
                                let msg = v["error"]["message"].as_str().unwrap_or("unknown error").to_string();
                                NodeOutput::failure(super::util::provider_error_for(super::util::Replay::Safe, status, "SHEETS_ERROR", format!("HTTP {}: {}", status, msg)))
                            }
                            Err(e) => NodeOutput::failure(super::util::provider_error_for(super::util::Replay::Safe, status, "PARSE_ERROR", e)),
                        }
                    }
                    Err(e) => {
                        super::util::http_err_output(super::util::Replay::Safe, &e)
                    }
                }
            }
        }
    }
}

impl GoogleSheetsNode {
    /// A 401 means the stored access token was rejected. Dropping the stored
    /// tokens makes the next run sign in again instead of failing the same way
    /// until the keychain entry is removed by hand. A pasted `api_key` has
    /// nothing stored.
    async fn discard_rejected_tokens(cfg: &Value) {
        let id = cfg["client_id"].as_str().filter(|s| !s.is_empty());
        let secret = cfg["client_secret"].as_str().filter(|s| !s.is_empty());
        if let (Some(id), Some(_)) = (id, secret) {
            oauth_listener::clear_tokens("google_sheets", id).await;
        }
    }

    /// Resolves a Google OAuth access token for Sheets access.
    ///
    /// With `client_id` and `client_secret` the token comes from
    /// `oauth_listener::get_tokens`, which stores it, checks expiry and
    /// refreshes it. Without them, a pasted `api_key` access token is used as is.
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

    #[test]
    fn value_input_defaults_to_raw_accepts_user_entered_in_any_case_and_refuses_the_rest() {
        assert_eq!(value_input_option(&json!(null)).unwrap(), "RAW");
        assert_eq!(value_input_option(&json!("")).unwrap(), "RAW");
        assert_eq!(value_input_option(&json!(" raw ")).unwrap(), "RAW");
        assert_eq!(value_input_option(&json!("user_entered")).unwrap(), "USER_ENTERED");
        assert_eq!(value_input_option(&json!("FORMULA")).unwrap_err().code, "INVALID_CONFIG");
        assert_eq!(value_input_option(&json!(true)).unwrap_err().code, "INVALID_CONFIG");
    }

    #[test]
    fn values_given_as_json_text_parse_into_the_same_rows_as_an_array() {
        let rows = json!([["a", 1, true, null], ["b"]]);
        assert_eq!(parse_rows(Some(&rows)).unwrap(), rows);
        let text = json!(r#" [["a", 1, true, null], ["b"]] "#);
        assert_eq!(parse_rows(Some(&text)).unwrap(), rows);
    }

    #[test]
    fn values_that_are_not_rows_fail_with_a_named_unrecoverable_error() {
        let cases: Vec<(Option<Value>, &str)> = vec![
            (None, "MISSING_VALUES"),
            (Some(json!(null)), "MISSING_VALUES"),
            (Some(json!("  ")), "MISSING_VALUES"),
            (Some(json!("[[\"a\"")), "INVALID_VALUES_JSON"),
            (Some(json!("not json")), "INVALID_VALUES_JSON"),
            (Some(json!(["a", "b"])), "INVALID_VALUES_SHAPE"),
            (Some(json!("[\"a\",\"b\"]")), "INVALID_VALUES_SHAPE"),
            (Some(json!([])), "INVALID_VALUES_SHAPE"),
            (Some(json!([[]])), "INVALID_VALUES_SHAPE"),
            (Some(json!([["a"], []])), "INVALID_VALUES_SHAPE"),
            (Some(json!([[{"x": 1}]])), "INVALID_VALUES_SHAPE"),
            (Some(json!({"a": 1})), "INVALID_VALUES_SHAPE"),
            (Some(json!(5)), "INVALID_VALUES_SHAPE"),
        ];
        for (raw, code) in cases {
            let err = parse_rows(raw.as_ref()).unwrap_err();
            assert_eq!(err.code, code, "input: {raw:?}");
            assert!(!err.recoverable, "input: {raw:?}");
        }
    }

    #[test]
    fn spreadsheet_id_accepts_a_pasted_sheet_address_and_rejects_anything_else() {
        let ok = [
            ("1AbC-d_9", "1AbC-d_9"),
            ("https://docs.google.com/spreadsheets/d/1AbC-d_9/edit#gid=0", "1AbC-d_9"),
            ("https://docs.google.com/spreadsheets/u/0/d/1AbC-d_9/edit", "1AbC-d_9"),
        ];
        for (raw, id) in ok {
            assert_eq!(spreadsheet_id_from(raw).unwrap(), id, "input: {raw}");
        }
        let bad = [
            "http://docs.google.com/spreadsheets/d/1AbC/edit",
            "https://evil.example/spreadsheets/d/1AbC/edit",
            "https://docs.google.com/document/d/1AbC/edit",
            "https://docs.google.com/spreadsheets/",
            "1AbC/../x",
            "1AbC?x=1",
        ];
        for raw in bad {
            assert_eq!(spreadsheet_id_from(raw).unwrap_err().code, "INVALID_SPREADSHEET_ID", "input: {raw}");
        }
    }

    #[tokio::test]
    async fn invalid_config_fails_before_any_sign_in() {
        use crate::model::ExecutionContext;

        async fn run(cfg: Value) -> NodeOutput {
            let mut input = cfg;
            input["client_id"] = json!("id");
            input["client_secret"] = json!("secret");
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                GoogleSheetsNode.execute(NodeInput {
                    resolved_credentials: std::collections::HashMap::new(),
                    cancel_token: None,
                    node_id: "n1".into(),
                    workflow_id: "w1".into(),
                    execution_id: "e1".into(),
                    input,
                    context: ExecutionContext::default(),
                }),
            )
            .await
            .expect("validation must answer without waiting on a sign-in")
        }

        let cases = [
            (json!({ "action": "bogus", "spreadsheet_id": "abc", "range": "Sheet1!A1" }), "INVALID_ACTION"),
            (json!({ "spreadsheet_id": "abc", "range": "Sheet1!A1" }), "MISSING_ACTION"),
            (json!({ "action": "get_values", "range": "Sheet1!A1" }), "MISSING_SPREADSHEET_ID"),
            (json!({ "action": "get_values", "spreadsheet_id": "abc" }), "MISSING_RANGE"),
            (json!({ "action": "append_row", "spreadsheet_id": "abc", "range": "Sheet1!A1", "values": ["a"] }), "INVALID_VALUES_SHAPE"),
        ];
        for (cfg, code) in cases {
            let out = run(cfg.clone()).await;
            assert_eq!(out.error.expect("must fail").code, code, "config: {cfg}");
        }
    }

    #[test]
    fn empty_range_and_missing_update_summary_still_give_a_usable_output() {
        let empty = get_values_result(json!({ "range": "Sheet1!A1:B2", "majorDimension": "ROWS" }), "Sheet1!A1:B2");
        assert!(empty.success);
        assert_eq!(empty.output.unwrap()["values"], json!([]));

        let appended = append_result(json!({ "spreadsheetId": "abc" }), "Sheet1!A1");
        assert!(appended.success);
        assert_eq!(appended.output.unwrap()["spreadsheetId"], json!("abc"));

        let with_updates = append_result(json!({ "updates": { "updatedRange": "Sheet1!A2:B2", "updatedCells": 2 } }), "Sheet1!A1");
        assert_eq!(with_updates.output.unwrap()["updatedCells"], json!(2));
    }
}
