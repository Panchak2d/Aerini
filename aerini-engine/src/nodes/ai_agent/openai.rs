use std::time::Duration;

use serde_json::{json, Value};

use crate::nodes::ai_prompt::{
    extract_provider_error, rejected_param, RejectedParam, is_official_openai,
};

use super::api_error::{AgentApiError, agent_response_error, agent_status_error, send_error};
use super::settings::ModelSettings;
use super::tools::{ToolChoice, ToolControls, arguments_text};
use super::web_search::Grounding;

// ── OpenAI-compatible agent call ─────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(super) async fn call_openai_agent(
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
    provider_id: &str,
    timeout: Duration,
) -> Result<Value, AgentApiError> {
    let mut all_messages: Vec<Value> = Vec::new();
    if !system.trim().is_empty() {
        all_messages.push(json!({ "role": "system", "content": system }));
    }
    all_messages.extend_from_slice(messages);

    // provider_id reaches here from an unvalidated request field (schema
    // enum enforcement is non-strict by default), unlike ai_prompt/mod.rs's
    // equivalent call site — so an unregistered id falls back to the
    // "openai" record rather than panicking, matching resolve_base_url's
    // own fallback for the same situation.
    let registry = crate::provider::ProviderRegistry::global();
    let record = registry
        .get(provider_id)
        .or_else(|| registry.get("openai"))
        .expect("openai always registered");

    let mut send_temperature = true;
    let mut token_key = if is_official_openai(base_url) { "max_completion_tokens" } else { "max_tokens" };
    let mut swapped_token_key = false;

    loop {
        let mut body = json!({ "model": model, "messages": all_messages });
        if send_temperature {
            body["temperature"] = json!(temperature);
        }
        body[token_key] = json!(max_tokens);
        if !tools.is_empty() {
            body["tools"] = json!(openai_tools(tools));
            body["tool_choice"] = json!(match controls.choice {
                ToolChoice::Auto     => "auto",
                ToolChoice::Required => "required",
                ToolChoice::NoTools  => "none",
            });
            if controls.single_call {
                body["parallel_tool_calls"] = json!(false);
            }
        }
        if let Some(effort) = settings.effort {
            body["reasoning_effort"] = json!(effort.as_str());
        }
        if settings.web_search.is_some() {
            body["web_search_options"] = json!({});
        }
        if let Some(schema) = response_schema {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": { "name": "response", "strict": true, "schema": schema }
            });
        }

        let req = crate::provider::ProviderRegistry::apply_auth(
            record,
            client.post(format!("{}/chat/completions", base_url))
                .header("Content-Type", "application/json"),
            api_key,
        ).json(&body);

        let response = req.timeout(timeout).send().await.map_err(|e| send_error(e, timeout))?;
        let status = response.status().as_u16();
        let json: Value = crate::nodes::util::read_json_response_capped(response)
            .await
            .map_err(|e| agent_status_error(status, e))?;

        match rejected_param(status, &json) {
            Some(RejectedParam::Temperature) if send_temperature => {
                send_temperature = false;
                continue;
            }
            Some(RejectedParam::TokenLimit) if !swapped_token_key => {
                swapped_token_key = true;
                token_key = if token_key == "max_tokens" { "max_completion_tokens" } else { "max_tokens" };
                continue;
            }
            _ => {}
        }

        if !tools.is_empty() && is_tools_not_supported(status, &json) {
            return Err(AgentApiError::ToolsNotSupported(format!(
                "Model '{model}' does not support tools. Choose a model whose Ollama page lists \"tools\", or clear the Tools field."
            )));
        }

        if let Some(msg) = agent_response_error(status, &json, "Unknown API error") {
            return Err(agent_status_error(status, msg));
        }
        return Ok(json);
    }
}

/// The OpenAI chat-completions shape of parsed tools: each one under `function`.
fn openai_tools(tools: &[Value]) -> Vec<Value> {
    tools.iter().map(|t| json!({ "type": "function", "function": t })).collect()
}

/// The assistant turn of an OpenAI-compatible reply, rebuilt from the fields
/// a request may carry: `role`, `content`, a non-empty `refusal` and
/// `tool_calls` in the standard `{id, type, function}` shape. The reply also
/// holds response-only fields (`annotations`, `audio`) that the request schema
/// does not list and strict servers refuse. Arguments sent as an object by some
/// local servers are turned back into text.
pub(super) fn openai_assistant(message: &Value) -> Option<Value> {
    if !message.is_object() {
        return None;
    }
    let calls: Vec<Value> = message["tool_calls"].as_array().into_iter().flatten()
        .map(|c| json!({
            "id": c["id"].as_str().unwrap_or(""),
            "type": "function",
            "function": {
                "name": c["function"]["name"].as_str().unwrap_or("unknown"),
                "arguments": arguments_text(&c["function"]["arguments"]),
            },
        }))
        .collect();
    let mut turn = json!({
        "role": "assistant",
        "content": message["content"].as_str().unwrap_or(""),
    });
    if let Some(refusal) = message["refusal"].as_str().filter(|r| !r.is_empty()) {
        turn["refusal"] = json!(refusal);
    }
    if !calls.is_empty() {
        turn["tool_calls"] = json!(calls);
    }
    Some(turn)
}

/// True for the 400 an OpenAI-compatible server sends when the model has no
/// tool support, e.g. Ollama's `{"error":"<model> does not support tools"}`
/// (also nested as `error.message` on `/v1`). Only that one phrase counts, so
/// any other 400 keeps its own `API_ERROR`.
fn is_tools_not_supported(status: u16, json: &Value) -> bool {
    status == 400
        && extract_provider_error(json, "")
            .is_some_and(|msg| msg.to_ascii_lowercase().contains("does not support tools"))
}

/// Sources from an OpenAI search reply: each `url_citation` annotation on the
/// message. OpenAI does not report the queries it ran.
pub(super) fn openai_grounding(message: &Value) -> Grounding {
    let mut grounding = Grounding::default();
    for a in message["annotations"].as_array().into_iter().flatten() {
        if a["type"] == "url_citation" {
            let c = &a["url_citation"];
            grounding.add_source(c["title"].as_str().unwrap_or(""), c["url"].as_str().unwrap_or(""));
        }
    }
    grounding
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::nodes::ai_agent::web_search::{OPENAI_SEARCH_MODEL, WebSearch};

    /// A reply slower than the node's timeout fails with a non-retryable TIMEOUT.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_reply_slower_than_the_timeout_fails_with_a_non_retryable_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("mock bind failed");
        let base_url = format!("http://127.0.0.1:{}", listener.local_addr().expect("local_addr failed").port());
        tokio::spawn(async move {
            let _held = listener.accept().await;
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });
        let settings = ModelSettings { web_search: None, effort: None };
        let err = call_openai_agent(
            &reqwest::Client::new(), &base_url, "", "m", "s",
            &[json!({ "role": "user", "content": "hi" })], &[], ToolControls::default(), None, &settings, 0.3, 2048, "openai",
            std::time::Duration::from_millis(300),
        ).await.expect_err("a timed-out request must fail");
        let out = err.into_node_output().error.expect("expected a NodeError");
        assert_eq!(out.code, "TIMEOUT");
        assert!(!out.recoverable, "a timeout must not be retried");
    }

    #[test]
    fn openai_tools_are_wrapped_under_function() {
        let wrapped = openai_tools(&[json!({ "name": "a" })]);
        assert_eq!(wrapped[0]["type"], "function");
        assert_eq!(wrapped[0]["function"]["name"], "a");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn openai_search_request_and_citations_use_the_search_model_fields() {
        let reply = r#"{"choices":[{"message":{"content":"Answer","annotations":[
            {"type":"url_citation","url_citation":{"start_index":0,"end_index":6,"title":"T","url":"https://o.example/1"}},
            {"type":"url_citation","url_citation":{"start_index":0,"end_index":6,"title":"T","url":"https://o.example/1"}}]},"finish_reason":"stop"}]}"#;
        let (base_url, bodies) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
        let settings = ModelSettings { web_search: Some(WebSearch { max_uses: None, domains: Vec::new() }), effort: None };
        let Ok(resp) = call_openai_agent(
            &crate::provider::shared_ai_client(), &base_url, "k", OPENAI_SEARCH_MODEL, "",
            &[json!({ "role": "user", "content": "hi" })], &[], ToolControls::default(), None, &settings,
            0.3, 256, "openai", Duration::from_secs(10),
        ).await else { panic!("request failed") };

        let sent: Value = serde_json::from_str(&bodies.lock().expect("mock lock")[0]).expect("json");
        assert_eq!(sent["web_search_options"], json!({}));
        let grounding = openai_grounding(&resp["choices"][0]["message"]);
        assert_eq!(grounding.sources, vec![("T".to_string(), "https://o.example/1".to_string())]);
        assert!(grounding.queries.is_empty());
    }
}
