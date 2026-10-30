use std::time::Duration;

use serde_json::{json, Value};

use crate::nodes::ai_prompt::{
    rejected_param, RejectedParam, clamp_claude_temperature,
};

use super::api_error::{AgentApiError, agent_response_error, agent_status_error, send_error};
use super::output::RequestedTool;
use super::settings::ModelSettings;
use super::tools::{ToolChoice, ToolControls, arguments_text};
use super::web_search::{Grounding, WEB_SEARCH_TOOL_NAME, WebSearch};

const ANTHROPIC_WEB_SEARCH_TOOL: &str = "web_search_20250305";

const DEFAULT_WEB_SEARCH_MAX_USES: u64 = 5;

#[allow(clippy::too_many_arguments)]
pub(super) async fn call_anthropic_agent(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    messages: &[Value],
    tools: &[Value],
    controls: ToolControls,
    response_schema: Option<&Value>,
    settings: &ModelSettings,
    temperature: f64,
    max_tokens: u64,
    timeout: Duration,
) -> Result<Value, AgentApiError> {
    let record = crate::provider::ProviderRegistry::global()
        .get("anthropic")
        .expect("anthropic always registered");
    let temperature = clamp_claude_temperature(temperature);
    let mut send_temperature = true;

    loop {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "max_tokens": max_tokens
        });
        if !system.trim().is_empty() {
            body["system"] = json!(system);
        }
        let mut request_tools = tools.to_vec();
        if let Some(search) = &settings.web_search {
            request_tools.push(anthropic_search_tool(search));
        }
        if !request_tools.is_empty() {
            body["tools"] = json!(request_tools);
        }
        if !tools.is_empty() {
            if let Some(choice) = anthropic_tool_choice(controls) {
                body["tool_choice"] = choice;
            }
        }
        let mut output_config = serde_json::Map::new();
        if let Some(effort) = settings.effort {
            output_config.insert("effort".to_string(), json!(effort.as_str()));
        }
        if let Some(schema) = response_schema {
            output_config.insert("format".to_string(), json!({ "type": "json_schema", "schema": schema }));
        }
        if !output_config.is_empty() {
            body["output_config"] = Value::Object(output_config);
        }
        if send_temperature {
            body["temperature"] = json!(temperature);
        }

        let req = crate::provider::ProviderRegistry::apply_auth(
            record,
            client.post(format!("{}/v1/messages", base_url))
                .header("Content-Type", "application/json"),
            api_key,
        ).json(&body);

        let response = req.timeout(timeout).send().await.map_err(|e| send_error(e, timeout))?;
        let status = response.status().as_u16();
        let json: Value = crate::nodes::util::read_json_response_capped(response)
            .await
            .map_err(|e| agent_status_error(status, e))?;

        if send_temperature && rejected_param(status, &json) == Some(RejectedParam::Temperature) {
            send_temperature = false;
            continue;
        }
        if let Some(msg) = agent_response_error(status, &json, "Unknown Anthropic API error") {
            return Err(agent_status_error(status, msg));
        }
        return Ok(json);
    }
}

/// The server tool entry that switches Anthropic's web search on.
fn anthropic_search_tool(search: &WebSearch) -> Value {
    let mut tool = json!({
        "type": ANTHROPIC_WEB_SEARCH_TOOL,
        "name": WEB_SEARCH_TOOL_NAME,
        "max_uses": search.max_uses.unwrap_or(DEFAULT_WEB_SEARCH_MAX_USES),
    });
    if !search.domains.is_empty() {
        tool["allowed_domains"] = json!(search.domains);
    }
    tool
}

/// Web search queries and sources from an Anthropic reply: the `server_tool_use`
/// blocks hold the queries, the `web_search_tool_result` blocks the pages (or
/// an error code, returned with HTTP 200), and the text blocks' citations name
/// the pages the answer quotes.
pub(super) fn anthropic_grounding(resp: &Value) -> Grounding {
    let mut grounding = Grounding::default();
    let Some(blocks) = resp["content"].as_array() else { return grounding };
    for block in blocks {
        match block["type"].as_str() {
            Some("server_tool_use") if block["name"] == WEB_SEARCH_TOOL_NAME => {
                if let Some(query) = block["input"]["query"].as_str() {
                    grounding.add_query(query);
                }
            }
            Some("web_search_tool_result") => match &block["content"] {
                Value::Array(results) => {
                    for r in results {
                        grounding.add_source(r["title"].as_str().unwrap_or(""), r["url"].as_str().unwrap_or(""));
                    }
                }
                Value::Object(error) => {
                    let code = error.get("error_code").and_then(Value::as_str).unwrap_or("unknown");
                    grounding.warnings.push(format!("a web search failed: {code}"));
                }
                _ => {}
            },
            Some("text") => {
                for c in block["citations"].as_array().into_iter().flatten() {
                    grounding.add_source(c["title"].as_str().unwrap_or(""), c["url"].as_str().unwrap_or(""));
                }
            }
            _ => {}
        }
    }
    grounding
}

/// The Anthropic shape of parsed tools: `{name, description, input_schema}`,
/// where `input_schema` is the tool's `parameters` and must be an object
/// schema. A tool with no parameters gets an empty one. A name or schema
/// Anthropic would reject fails here, naming the tool, instead of as an opaque
/// HTTP 400 from the provider.
pub(super) fn anthropic_tools(tools: &[Value]) -> Result<Vec<Value>, String> {
    tools.iter().map(|t| {
        let name = t["name"].as_str().unwrap_or("");
        if !is_anthropic_tool_name(name) {
            return Err(format!(
                "tool \"{name}\" cannot be sent to Anthropic: a name may use only letters, digits, underscores and hyphens, up to 64 characters"
            ));
        }
        let mut schema = match &t["parameters"] {
            Value::Null => serde_json::Map::new(),
            Value::Object(m) => m.clone(),
            _ => return Err(format!("tool \"{name}\": parameters must be a JSON Schema object")),
        };
        match schema.get("type") {
            None => { schema.insert("type".to_string(), json!("object")); }
            Some(Value::String(ty)) if ty == "object" => {}
            Some(_) => return Err(format!("tool \"{name}\": Anthropic needs the top level of parameters to have type \"object\"")),
        }
        schema.entry("properties").or_insert_with(|| json!({}));
        let mut def = json!({ "name": name, "input_schema": schema });
        if let Some(d) = t["description"].as_str().filter(|d| !d.trim().is_empty()) {
            def["description"] = json!(d);
        }
        Ok(def)
    }).collect()
}

/// The tools an Anthropic reply asks for: its `tool_use` blocks, in order.
pub(super) fn anthropic_tool_requests(resp: &Value) -> Vec<RequestedTool> {
    resp["content"].as_array()
        .map(|blocks| blocks.iter()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| RequestedTool {
                id:        b["id"].as_str().unwrap_or("").to_string(),
                name:      b["name"].as_str().unwrap_or("unknown").to_string(),
                arguments: arguments_text(&b["input"]),
            })
            .collect())
        .unwrap_or_default()
}

// ── Anthropic agent call ──────────────────────────────────────────────────────
//
// Anthropic uses /v1/messages with x-api-key header and a different
// message/response format from OpenAI.
//
// Tools: {"tools":[{name, description, input_schema}]}. The reply's `content`
// holds `text`, `thinking` and `tool_use` blocks; `stop_reason` is "tool_use"
// when the model asked for tools. `tool_choice` is left out unless a setting
// needs it, so it defaults to auto, the only mode besides "none" that current
// Claude models accept. Web Search adds Anthropic's `web_search` server tool to
// the same list: its `server_tool_use` and `web_search_tool_result` blocks are
// run by Anthropic and are not tool requests for the workflow.
/// Anthropic's `tool_choice` for the node's settings, or `None` for the default.
/// `required` never gets here: `check_tool_controls` rejects it for Anthropic.
fn anthropic_tool_choice(controls: ToolControls) -> Option<Value> {
    if controls.choice == ToolChoice::NoTools {
        Some(json!({ "type": "none" }))
    } else if controls.single_call {
        Some(json!({ "type": "auto", "disable_parallel_tool_use": true }))
    } else {
        None
    }
}

/// Tool names Anthropic accepts: 1 to 64 letters, digits, underscores or hyphens.
fn is_anthropic_tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The assistant turn of an Anthropic reply: its whole `content` array, so
/// thinking, `tool_use` and server-tool blocks go back exactly as received.
pub(super) fn anthropic_assistant(resp: &Value) -> Option<Value> {
    resp["content"].is_array().then(|| json!({ "role": "assistant", "content": resp["content"].clone() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_tools_use_input_schema_with_object_defaults_and_reject_unsendable_ones() {
        let built = anthropic_tools(&[
            json!({ "name": "ping", "parameters": {} }),
            json!({ "name": "find-it_2", "description": "d", "parameters": { "properties": { "q": { "type": "string" } }, "required": ["q"] } }),
        ]).expect("valid tools");
        assert_eq!(built[0], json!({ "name": "ping", "input_schema": { "type": "object", "properties": {} } }));
        assert!(built[0].get("description").is_none());
        assert_eq!(built[1]["description"], "d");
        assert_eq!(built[1]["input_schema"]["type"], "object");
        assert_eq!(built[1]["input_schema"]["required"], json!(["q"]));

        let bad_name = anthropic_tools(&[json!({ "name": "get weather" })]).expect_err("space in name");
        assert!(bad_name.contains("get weather"), "{bad_name}");
        assert!(anthropic_tools(&[json!({ "name": "a".repeat(65) })]).is_err());
        assert!(anthropic_tools(&[json!({ "name": "a", "parameters": { "type": "string" } })]).is_err());
        assert!(anthropic_tools(&[json!({ "name": "a", "parameters": "nope" })]).is_err());
    }
}
