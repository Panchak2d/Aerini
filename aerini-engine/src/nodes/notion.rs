use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

const NOTION_VERSION: &str = "2022-06-28";

/// A Notion ID is a UUID: 32 hex digits, optionally hyphenated 8-4-4-4-12.
fn is_valid_notion_id(s: &str) -> bool {
    let hex = |p: &str| p.bytes().all(|c| c.is_ascii_hexdigit());
    match s.len() {
        32 => hex(s),
        36 => {
            let parts: Vec<&str> = s.split('-').collect();
            parts.len() == 5
                && [8, 4, 4, 4, 12].iter().zip(&parts).all(|(n, p)| p.len() == *n && hex(p))
        }
        _ => false,
    }
}

pub struct NotionNode;

#[async_trait]
impl Node for NotionNode {
    fn type_id(&self) -> &'static str { "notion" }
    fn display_name(&self) -> &'static str { "Notion" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Create or update pages and database entries in Notion." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action":      { "type": "string", "enum": ["create_page", "update_page"], "description": "Operation to perform" },
                "database_id": { "type": "string", "description": "Notion database ID — required for create_page" },
                "page_id":     { "type": "string", "description": "Notion page ID — required for update_page" },
                "title":       { "type": "string", "description": "Page title (used in the Name / title property)" },
                "properties":  { "description": "Additional Notion page properties as a JSON object" },
                "api_key":     { "type": "string", "description": "Notion integration token (secret_...)" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id":       { "type": "string", "description": "Notion page ID" },
                "url":      { "type": "string", "description": "Public URL of the page" },
                "object":   { "type": "string" },
                "archived": { "type": "boolean" }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let api_key = match input.input["api_key"].as_str().filter(|s| !s.is_empty()) {
            Some(k) => k.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_TOKEN",
                "Notion integration token is required — add it via the credential store",
            )),
        };

        let action = input.input["action"].as_str().unwrap_or("create_page");

        match action {
            "create_page" => {
                let database_id = match input.input["database_id"].as_str().filter(|s| !s.is_empty()) {
                    Some(id) => id.to_string(),
                    None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_DATABASE_ID", "database_id is required for create_page")),
                };

                // Build properties — start with title if provided.
                let mut properties = match input.input.get("properties") {
                    Some(Value::Object(m)) => m.clone(),
                    _ => serde_json::Map::new(),
                };

                if let Some(title) = input.input["title"].as_str().filter(|s| !s.is_empty()) {
                    properties.entry("Name".to_string()).or_insert_with(|| {
                        json!({
                            "title": [{ "text": { "content": title } }]
                        })
                    });
                }

                let body = json!({
                    "parent": { "database_id": database_id },
                    "properties": Value::Object(properties)
                });

                self.send_request("POST", "https://api.notion.com/v1/pages", &api_key, body, "create_page").await
            }
            "update_page" => {
                let page_id = match input.input["page_id"].as_str().filter(|s| !s.is_empty()) {
                    Some(id) => id.to_string(),
                    None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_PAGE_ID", "page_id is required for update_page")),
                };

                if !is_valid_notion_id(&page_id) {
                    return NodeOutput::failure(NodeError::unrecoverable(
                        "INVALID_PAGE_ID",
                        "page_id must be a Notion page ID (32 hex digits, with or without hyphens)",
                    ));
                }

                let mut properties = match input.input.get("properties") {
                    Some(Value::Object(m)) => m.clone(),
                    _ => serde_json::Map::new(),
                };

                if let Some(title) = input.input["title"].as_str().filter(|s| !s.is_empty()) {
                    properties.entry("Name".to_string()).or_insert_with(|| {
                        json!({
                            "title": [{ "text": { "content": title } }]
                        })
                    });
                }

                let url = format!("https://api.notion.com/v1/pages/{}", page_id);
                let body = json!({ "properties": Value::Object(properties) });

                self.send_request("PATCH", &url, &api_key, body, "update_page").await
            }
            other => NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_ACTION",
                format!("Unknown action '{}'. Valid values: create_page, update_page", other),
            )),
        }
    }
}

impl NotionNode {
    async fn send_request(&self, method: &str, url: &str, api_key: &str, body: Value, action: &str) -> NodeOutput {
        let req = if method == "POST" {
            super::shared_http_client().post(url)
        } else {
            super::shared_http_client().patch(url)
        };

        match req
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Notion-Version", NOTION_VERSION)
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                match super::util::read_json_response_capped(resp).await {
                    Ok(v) => {
                        if status == 200 || status == 201 {
                            let page_id = v["id"].as_str().unwrap_or("").to_string();
                            NodeOutput::success_with_logs(
                                v,
                                vec![format!("Notion {} succeeded — page ID: {}", action, page_id)],
                            )
                        } else {
                            let msg = v["message"].as_str().unwrap_or("unknown error").to_string();
                            NodeOutput::failure(super::util::provider_error(status, "NOTION_ERROR", format!("HTTP {}: {}", status, msg)))
                        }
                    }
                    Err(e) => NodeOutput::failure(super::util::provider_error(status, "PARSE_ERROR", e)),
                }
            }
            Err(e) => {
                super::util::http_err_output(&e)
            }
        }
    }
}


#[cfg(test)]
mod page_id_tests {
    use super::is_valid_notion_id;

    #[test]
    fn page_id_must_be_a_uuid() {
        assert!(is_valid_notion_id("0123456789abcdef0123456789abcdef"));
        assert!(is_valid_notion_id("01234567-89ab-cdef-0123-456789abcdef"));
        assert!(!is_valid_notion_id("../users"));
        assert!(!is_valid_notion_id("0123456789abcdef0123456789abcdeg"));
        assert!(!is_valid_notion_id("0123456789abcdef0123456789abcdef/x"));
    }
}
