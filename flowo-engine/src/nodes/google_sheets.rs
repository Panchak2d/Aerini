use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

pub struct GoogleSheetsNode;

#[async_trait]
impl Node for GoogleSheetsNode {
    fn type_id(&self) -> &'static str { "google_sheets" }
    fn display_name(&self) -> &'static str { "Google Sheets" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action", "spreadsheet_id", "range"],
            "properties": {
                "action":         { "type": "string", "enum": ["append_row", "get_values"], "description": "Operation to perform" },
                "spreadsheet_id": { "type": "string", "description": "Google Sheets spreadsheet ID (from the URL)" },
                "range":          { "type": "string", "description": "A1 notation range (e.g. Sheet1!A1:D1)" },
                "values":         { "description": "Row data for append_row — array of arrays, e.g. [[\"a\",\"b\"]]" },
                "api_key":        { "type": "string", "description": "Google OAuth 2.0 access token" }
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
        let api_key = match input.input["api_key"].as_str().filter(|s| !s.is_empty()) {
            Some(k) => k.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_TOKEN",
                "Google OAuth access token is required — store it as a credential named 'api_key'",
            )),
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
                    let mut u = url::Url::parse("https://sheets.googleapis.com/v4/spreadsheets/").unwrap();
                    u.path_segments_mut().unwrap()
                        .push(&spreadsheet_id)
                        .push("values")
                        .push(&format!("{}:append", range));
                    u.set_query(Some("valueInputOption=RAW"));
                    u.to_string()
                };

                let body = json!({ "values": values });

                match super::shared_http_client()
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", api_key))
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        match resp.json::<Value>().await {
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
                                NodeOutput::failure(NodeError::unrecoverable("SHEETS_ERROR", format!("HTTP {}: {}", status, msg)))
                            }
                            Err(e) => NodeOutput::failure(NodeError::unrecoverable("PARSE_ERROR", e.to_string())),
                        }
                    }
                    Err(e) => {
                        super::util::http_err_output(&e)
                    }
                }
            }
            "get_values" => {
                let url = {
                    let mut u = url::Url::parse("https://sheets.googleapis.com/v4/spreadsheets/").unwrap();
                    u.path_segments_mut().unwrap()
                        .push(&spreadsheet_id)
                        .push("values")
                        .push(&range);
                    u.to_string()
                };

                match super::shared_http_client()
                    .get(&url)
                    .header("Authorization", format!("Bearer {}", api_key))
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        match resp.json::<Value>().await {
                            Ok(v) if status == 200 => {
                                let row_count = v["values"].as_array().map(|a| a.len()).unwrap_or(0);
                                NodeOutput::success_with_logs(
                                    v,
                                    vec![format!("Retrieved {} row(s) from {}", row_count, range)],
                                )
                            }
                            Ok(v) => {
                                let msg = v["error"]["message"].as_str().unwrap_or("unknown error").to_string();
                                NodeOutput::failure(NodeError::unrecoverable("SHEETS_ERROR", format!("HTTP {}: {}", status, msg)))
                            }
                            Err(e) => NodeOutput::failure(NodeError::unrecoverable("PARSE_ERROR", e.to_string())),
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
