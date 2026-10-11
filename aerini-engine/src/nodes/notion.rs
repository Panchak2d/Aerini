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

/// The only API version that accepts a `data_source_id` parent on Create a
/// page, which Notion documents as usable per call while everything else
/// stays on `NOTION_VERSION`.
const DATA_SOURCE_NOTION_VERSION: &str = "2025-09-03";

/// The database ID in `raw`: the ID itself, or the one in a pasted
/// `notion.so` / `notion.site` link (the last 32 hex digits of the last path
/// segment; the query string, such as `?v=<view id>`, is ignored). `None` when
/// `raw` is neither.
fn database_id_from_input(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if is_valid_notion_id(raw) {
        return Some(raw.to_string());
    }
    let scheme_len = ["https://", "http://"]
        .iter()
        .find(|p| raw.get(..p.len()).is_some_and(|head| head.eq_ignore_ascii_case(p)))
        .map(|p| p.len())?;
    let rest = &raw[scheme_len..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..authority_end].to_ascii_lowercase();
    let notion_host = ["notion.so", "notion.site"]
        .iter()
        .any(|d| host == *d || host.strip_suffix(d).is_some_and(|h| h.ends_with('.')));
    if !notion_host || host.contains(['@', ':']) {
        return None;
    }
    let path = rest[authority_end..].split(['?', '#']).next().unwrap_or("");
    let segment = path.split('/').rfind(|p| !p.is_empty())?;
    if is_valid_notion_id(segment) {
        return Some(segment.to_string());
    }
    let split_at = segment.len().checked_sub(32)?;
    let (head, tail) = (segment.get(..split_at)?, segment.get(split_at..)?);
    (is_valid_notion_id(tail) && (head.is_empty() || head.ends_with('-'))).then(|| tail.to_string())
}

const DEFAULT_TITLE_PROPERTY: &str = "Name";

struct Request {
    method: &'static str,
    url: String,
    body: Value,
    action: &'static str,
    notion_version: &'static str,
}

fn title_property(cfg: &Value) -> Result<String, NodeError> {
    match cfg.get("title_property") {
        None | Some(Value::Null) => Ok(DEFAULT_TITLE_PROPERTY.to_string()),
        Some(Value::String(s)) => {
            let t = s.trim();
            Ok(if t.is_empty() { DEFAULT_TITLE_PROPERTY } else { t }.to_string())
        }
        Some(_) => Err(NodeError::unrecoverable(
            "INVALID_TITLE_PROPERTY",
            "title_property must be text: the name of the database's title property",
        )),
    }
}

/// Accepts the properties as a JSON object or as JSON text (what the UI stores).
/// Unset or blank means none.
fn parse_properties(raw: Option<&Value>) -> Result<serde_json::Map<String, Value>, NodeError> {
    let parsed;
    let v = match raw {
        None | Some(Value::Null) => return Ok(serde_json::Map::new()),
        Some(Value::String(s)) => {
            if s.trim().is_empty() {
                return Ok(serde_json::Map::new());
            }
            parsed = serde_json::from_str::<Value>(s).map_err(|e| NodeError::unrecoverable(
                "INVALID_PROPERTIES_JSON",
                format!("properties is not valid JSON ({e}); expected an object such as {{\"Status\": {{\"select\": {{\"name\": \"Done\"}}}}}}"),
            ))?;
            &parsed
        }
        Some(other) => other,
    };
    match v {
        Value::Object(m) => Ok(m.clone()),
        _ => Err(NodeError::unrecoverable(
            "INVALID_PROPERTIES_SHAPE",
            "properties must be a JSON object keyed by property name, not an array or a single value",
        )),
    }
}

fn page_properties(cfg: &Value) -> Result<serde_json::Map<String, Value>, NodeError> {
    let key = title_property(cfg)?;
    let mut properties = parse_properties(cfg.get("properties"))?;
    if let Some(title) = cfg["title"].as_str().filter(|s| !s.is_empty()) {
        let clash = properties.iter()
            .find(|(k, v)| **k == key || v.get("title").is_some())
            .map(|(k, _)| k.clone());
        if let Some(existing) = clash {
            return Err(NodeError::unrecoverable(
                "TITLE_CONFLICT",
                format!("title is set and properties already holds '{existing}'; set the title in only one place"),
            ));
        }
        properties.insert(key, json!({ "title": [{ "text": { "content": title } }] }));
    }
    Ok(properties)
}

fn plan(cfg: &Value) -> Result<Request, NodeError> {
    match cfg["action"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        None => Err(NodeError::unrecoverable(
            "MISSING_ACTION",
            "action is required. Valid values: create_page, update_page",
        )),
        Some("create_page") => {
            let database_id = cfg["database_id"].as_str().map(str::trim).filter(|s| !s.is_empty());
            let data_source_id = cfg["data_source_id"].as_str().map(str::trim).filter(|s| !s.is_empty());
            let (parent, notion_version) = match (database_id, data_source_id) {
                (None, None) => return Err(NodeError::unrecoverable(
                    "MISSING_DATABASE_ID",
                    "database_id is required for create_page (or data_source_id, for a database with several data sources)",
                )),
                (Some(_), Some(_)) => return Err(NodeError::unrecoverable(
                    "PARENT_CONFLICT",
                    "database_id and data_source_id are both set; set only one",
                )),
                (Some(raw), None) => {
                    let id = database_id_from_input(raw).ok_or_else(|| NodeError::unrecoverable(
                        "INVALID_DATABASE_ID",
                        "database_id must be a Notion database ID (32 hex digits, with or without hyphens) or a link to the database on notion.so or notion.site",
                    ))?;
                    (json!({ "database_id": id }), NOTION_VERSION)
                }
                (None, Some(id)) => {
                    if !is_valid_notion_id(id) {
                        return Err(NodeError::unrecoverable(
                            "INVALID_DATA_SOURCE_ID",
                            "data_source_id must be a Notion data source ID (32 hex digits, with or without hyphens)",
                        ));
                    }
                    (json!({ "type": "data_source_id", "data_source_id": id }), DATA_SOURCE_NOTION_VERSION)
                }
            };
            let properties = page_properties(cfg)?;
            Ok(Request {
                method: "POST",
                url: "https://api.notion.com/v1/pages".to_string(),
                body: json!({ "parent": parent, "properties": Value::Object(properties) }),
                action: "create_page",
                notion_version,
            })
        }
        Some("update_page") => {
            let page_id = cfg["page_id"].as_str().filter(|s| !s.is_empty())
                .ok_or_else(|| NodeError::unrecoverable("MISSING_PAGE_ID", "page_id is required for update_page"))?;
            if !is_valid_notion_id(page_id) {
                return Err(NodeError::unrecoverable(
                    "INVALID_PAGE_ID",
                    "page_id must be a Notion page ID (32 hex digits, with or without hyphens)",
                ));
            }
            let properties = page_properties(cfg)?;
            if properties.is_empty() {
                return Err(NodeError::unrecoverable(
                    "NOTHING_TO_UPDATE",
                    "update_page needs a title or at least one property to change",
                ));
            }
            Ok(Request {
                method: "PATCH",
                url: format!("https://api.notion.com/v1/pages/{}", page_id),
                body: json!({ "properties": Value::Object(properties) }),
                action: "update_page",
                notion_version: NOTION_VERSION,
            })
        }
        Some(other) => Err(NodeError::unrecoverable(
            "INVALID_ACTION",
            format!("Unknown action '{}'. Valid values: create_page, update_page", other),
        )),
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
                "database_id": { "type": "string", "description": "Notion database ID (32 hex digits, with or without hyphens) or a link to the database on notion.so or notion.site — create_page needs this or data_source_id" },
                "data_source_id": { "type": "string", "description": "Notion data source ID, for a database with several data sources. Use instead of database_id, not with it" },
                "page_id":     { "type": "string", "description": "Notion page ID — required for update_page" },
                "title":       { "type": "string", "description": "Page title, written to the title property (see Title Property)" },
                "title_property": { "type": "string", "description": "Name of the database's title property (default Name). Set it when the database calls its title column something else" },
                "properties":  { "x-aerini-multiline": true, "description": "Additional Notion page properties as a JSON object, e.g. {\"Status\": {\"select\": {\"name\": \"Done\"}}}" },
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

        let req = match plan(&input.input) {
            Ok(r) => r,
            Err(e) => return NodeOutput::failure(e),
        };

        self.send_request(req.method, &req.url, &api_key, req.body, req.action, req.notion_version).await
    }
}

impl NotionNode {
    async fn send_request(&self, method: &str, url: &str, api_key: &str, body: Value, action: &str, notion_version: &str) -> NodeOutput {
        let req = if method == "POST" {
            super::shared_http_client().post(url)
        } else {
            super::shared_http_client().patch(url)
        };

        match req
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Notion-Version", notion_version)
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
                super::util::http_err_output(
                    if action == "update_page" { super::util::Replay::Safe } else { super::util::Replay::Never },
                    &e,
                )
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

#[cfg(test)]
mod request_tests {
    use super::*;

    const DB: &str = "0123456789abcdef0123456789abcdef";
    const PAGE: &str = "01234567-89ab-cdef-0123-456789abcdef";

    fn create(extra: Value) -> Result<Request, NodeError> {
        let mut cfg = json!({ "action": "create_page", "database_id": DB });
        for (k, v) in extra.as_object().unwrap() {
            cfg[k] = v.clone();
        }
        plan(&cfg)
    }

    fn update(extra: Value) -> Result<Request, NodeError> {
        let mut cfg = json!({ "action": "update_page", "page_id": PAGE });
        for (k, v) in extra.as_object().unwrap() {
            cfg[k] = v.clone();
        }
        plan(&cfg)
    }

    #[test]
    fn properties_given_as_json_text_are_sent_as_an_object() {
        let text = r#"{"Status": {"select": {"name": "Done"}}}"#;
        let from_text = create(json!({ "properties": text })).unwrap();
        let from_object = create(json!({ "properties": { "Status": { "select": { "name": "Done" } } } })).unwrap();
        assert_eq!(from_text.body, from_object.body);
        assert_eq!(from_text.body["properties"]["Status"]["select"]["name"], "Done");
        assert!(create(json!({ "properties": "  " })).unwrap().body["properties"].as_object().unwrap().is_empty());
    }

    #[test]
    fn properties_that_are_not_a_json_object_fail_with_a_named_unrecoverable_error() {
        let cases = [
            (json!("{\"a\":"), "INVALID_PROPERTIES_JSON"),
            (json!("plain words"), "INVALID_PROPERTIES_JSON"),
            (json!("[1,2]"), "INVALID_PROPERTIES_SHAPE"),
            (json!("null"), "INVALID_PROPERTIES_SHAPE"),
            (json!([1, 2]), "INVALID_PROPERTIES_SHAPE"),
            (json!(7), "INVALID_PROPERTIES_SHAPE"),
        ];
        for (raw, code) in cases {
            let err = create(json!({ "properties": raw })).err().expect("must fail");
            assert_eq!(err.code, code, "input: {raw}");
            assert!(!err.recoverable, "input: {raw}");
        }
    }

    #[test]
    fn title_goes_to_the_configured_title_property_and_defaults_to_name() {
        let default = create(json!({ "title": "Hi" })).unwrap();
        assert_eq!(default.body["properties"]["Name"]["title"][0]["text"]["content"], "Hi");

        let custom = create(json!({ "title": "Hi", "title_property": " Task " })).unwrap();
        let props = custom.body["properties"].as_object().unwrap();
        assert_eq!(props.len(), 1);
        assert_eq!(props["Task"]["title"][0]["text"]["content"], "Hi");

        let blank = update(json!({ "title": "Hi", "title_property": "" })).unwrap();
        assert!(blank.body["properties"].get("Name").is_some());

        assert_eq!(create(json!({ "title_property": 3 })).err().unwrap().code, "INVALID_TITLE_PROPERTY");
    }

    #[test]
    fn title_plus_another_title_value_in_properties_fails_instead_of_sending_two() {
        let own_key = json!({ "Task": { "title": [{ "text": { "content": "x" } }] } });
        assert_eq!(create(json!({ "title": "Hi", "properties": own_key })).err().unwrap().code, "TITLE_CONFLICT");
        let same_key = json!({ "Name": { "rich_text": [] } });
        assert_eq!(create(json!({ "title": "Hi", "properties": same_key })).err().unwrap().code, "TITLE_CONFLICT");
        assert!(create(json!({ "properties": own_key })).is_ok());
    }

    #[test]
    fn update_page_with_no_title_and_no_properties_is_rejected() {
        for extra in [json!({}), json!({ "properties": {} }), json!({ "properties": " " }), json!({ "title": "" })] {
            assert_eq!(update(extra.clone()).err().expect("must fail").code, "NOTHING_TO_UPDATE", "config: {extra}");
        }
        assert!(update(json!({ "title": "New" })).is_ok());
        assert!(update(json!({ "properties": { "Status": { "select": { "name": "Done" } } } })).is_ok());
    }

    #[test]
    fn database_id_must_be_a_notion_id_before_any_request() {
        assert_eq!(plan(&json!({ "action": "create_page" })).err().unwrap().code, "MISSING_DATABASE_ID");
        assert_eq!(plan(&json!({ "database_id": DB })).err().unwrap().code, "MISSING_ACTION");
        for bad in ["../users", "https://www.notion.so/abc", "0123456789abcdef0123456789abcdeg"] {
            let err = create(json!({ "database_id": bad })).err().expect("must fail");
            assert_eq!(err.code, "INVALID_DATABASE_ID", "input: {bad}");
        }
        assert!(create(json!({ "database_id": PAGE })).is_ok());
        assert!(create(json!({})).unwrap().body["properties"].as_object().unwrap().is_empty());
    }

    #[test]
    fn database_id_is_taken_from_a_pasted_notion_link() {
        let hyphenated = "01234567-89ab-cdef-0123-456789abcdef";
        for link in [
            format!("https://www.notion.so/{DB}?v=fedcba9876543210fedcba9876543210"),
            format!("https://www.notion.so/myworkspace/Tasks-{DB}?v=fedcba9876543210fedcba9876543210&pvs=4"),
            format!("HTTPS://team.notion.site/{DB}#top"),
            format!("https://notion.so/{hyphenated}"),
        ] {
            let body = create(json!({ "database_id": link })).unwrap().body;
            let got = body["parent"]["database_id"].as_str().unwrap().to_string();
            assert!(got == DB || got == hyphenated, "link {link} gave {got}");
        }
        for bad in [
            format!("https://evil.example/{DB}"),
            format!("https://notion.so.evil.example/{DB}"),
            format!("https://notion.so@evil.example/{DB}"),
            format!("https://www.notion.so/Tasks{DB}"),
            "https://www.notion.so/".to_string(),
            format!("ftp://www.notion.so/{DB}"),
        ] {
            assert_eq!(create(json!({ "database_id": bad.clone() })).err().expect("must fail").code, "INVALID_DATABASE_ID", "{bad}");
        }
    }

    #[test]
    fn data_source_id_sets_the_parent_and_the_per_call_api_version() {
        let ds = create(json!({ "database_id": "", "data_source_id": PAGE })).unwrap();
        assert_eq!(ds.body["parent"], json!({ "type": "data_source_id", "data_source_id": PAGE }));
        assert_eq!(ds.notion_version, "2025-09-03");
        let db = create(json!({})).unwrap();
        assert_eq!(db.notion_version, "2022-06-28");
        assert_eq!(db.body["parent"], json!({ "database_id": DB }));
        assert_eq!(update(json!({ "title": "x" })).unwrap().notion_version, "2022-06-28");

        assert_eq!(create(json!({ "data_source_id": PAGE })).err().unwrap().code, "PARENT_CONFLICT");
        let mut cfg = json!({ "action": "create_page", "data_source_id": "../x" });
        assert_eq!(plan(&cfg).err().unwrap().code, "INVALID_DATA_SOURCE_ID");
        cfg["data_source_id"] = json!("  ");
        assert_eq!(plan(&cfg).err().unwrap().code, "MISSING_DATABASE_ID");
    }
}
