use std::time::Duration;

use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::ai_prompt::gemini_model_id;

use super::api_error::{AgentApiError, agent_response_error, agent_status_error, send_error};
use super::conversation::capture_transcript;
use super::output::{AgentReply, RequestedTool, Usage, agent_output};
use super::settings::ModelSettings;
use super::tools::{ToolChoice, ToolControls, arguments_text, is_blank_schema};
use super::web_search::Grounding;

// ── Gemini agent loop ────────────────────────────────────────────────────────
//
// Gemini message format differs fundamentally from OpenAI:
//   - contents[].role is "user" | "model" (not "user" | "assistant" | "tool")
//   - parts[] holds text or functionCall objects
//   - tool results are sent as role:"user" with parts[].functionResponse
//   - tools declared as {"functionDeclarations":[{name, description, parametersJsonSchema}]}
//   - finish is candidates[0].finishReason == "STOP"
//
// Docs: https://ai.google.dev/api/generate-content
//       https://ai.google.dev/gemini-api/docs/function-calling

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_gemini_agent(
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    system: String,
    contents: Vec<Value>,
    tools: Vec<Value>,
    controls: ToolControls,
    response_schema: Option<Value>,
    settings: ModelSettings,
    max_tokens: u64,
    temperature: f64,
    return_messages: bool,
    mut reasoning_trace: Vec<String>,
    timeout: Duration,
) -> NodeOutput {
    if api_key.is_empty() {
        return NodeOutput::failure(NodeError::unrecoverable(
            "MISSING_API_KEY",
            "Gemini requires an API key. Add one in Connections.",
        ));
    }

    let resp = match call_gemini_agent(
        &client, &base_url, &api_key, &model, &system,
        &contents, &tools, controls, response_schema.as_ref(), &settings, temperature, max_tokens, timeout,
    ).await {
        Ok(r)  => r,
        Err(e) => return e.into_node_output(),
    };

    let model_content = &resp["candidates"][0]["content"];
    if model_content.is_null() {
        return NodeOutput::failure(NodeError::unrecoverable(
            "GEMINI_NO_CONTENT",
            "Gemini returned no content — request may have been blocked by safety filters.",
        ));
    }
    let finish_reason = resp["candidates"][0]["finishReason"].as_str().unwrap_or("STOP");
    let empty_parts = vec![];
    let parts = model_content["parts"].as_array().unwrap_or(&empty_parts);

    let text_content: String = parts.iter()
        .filter(|p| p["thought"] != true)
        .filter_map(|p| p["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let requested: Vec<RequestedTool> = parts.iter()
        .filter(|p| !p["functionCall"].is_null())
        .map(|p| RequestedTool {
            id:        p["functionCall"]["id"].as_str().unwrap_or("").to_string(),
            name:      p["functionCall"]["name"].as_str().unwrap_or("unknown").to_string(),
            arguments: arguments_text(&p["functionCall"]["args"]),
        })
        .collect();

    let truncated = finish_reason == "MAX_TOKENS";
    if text_content.is_empty() && requested.is_empty() && finish_reason != "STOP" {
        return NodeOutput::failure(NodeError::unrecoverable(
            "GEMINI_NO_CONTENT",
            format!("Gemini returned no text ({finish_reason}). Check max_tokens and the safety filters."),
        ));
    }
    // Gemini reports STOP on a reply that carries functionCall parts.
    let finished = finish_reason == "STOP" && requested.is_empty();
    let usage = Usage {
        model:         resp["modelVersion"].as_str().unwrap_or(model.as_str()).to_string(),
        provider:      "gemini",
        input_tokens:  resp["usageMetadata"]["promptTokenCount"].as_u64().unwrap_or(0),
        output_tokens: resp["usageMetadata"]["candidatesTokenCount"].as_u64().unwrap_or(0)
            + resp["usageMetadata"]["thoughtsTokenCount"].as_u64().unwrap_or(0),
    };
    let grounding = gemini_grounding(&resp["candidates"][0]);
    let transcript = capture_transcript(
        return_messages, "gemini", contents, with_default_role(model_content, "model"),
        false, truncated && !requested.is_empty(), &mut reasoning_trace,
    );
    agent_output(
        AgentReply { content: text_content, requested, finished, truncated, usage, tool_required: controls.requires_tool(&tools), response_schema, grounding, transcript },
        &tools,
        reasoning_trace,
    )
}

// Gemini generateContent HTTP call.
//
// Endpoint: POST {base_url}/models/{model}:generateContent
// Auth:     x-goog-api-key header
// Tools:    {"functionDeclarations":[{name, description, parametersJsonSchema}]}
// System:   top-level "systemInstruction" field
#[allow(clippy::too_many_arguments)]
async fn call_gemini_agent(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    contents: &[Value],
    tools: &[Value],
    controls: ToolControls,
    response_schema: Option<&Value>,
    settings: &ModelSettings,
    temperature: f64,
    max_tokens: u64,
    timeout: Duration,
) -> Result<Value, AgentApiError> {
    let endpoint = format!("{}/models/{}:generateContent", base_url, gemini_model_id(model));

    let mut body = json!({
        "contents": contents,
        "generationConfig": {
            "temperature":     temperature,
            "maxOutputTokens": max_tokens
        }
    });

    if !system.is_empty() {
        body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }

    if let Some(schema) = response_schema {
        body["generationConfig"]["responseMimeType"] = json!("application/json");
        body["generationConfig"]["responseJsonSchema"] = schema.clone();
    }

    if let Some(effort) = settings.effort {
        body["generationConfig"]["thinkingConfig"] = json!({ "thinkingLevel": effort.as_str() });
    }

    if settings.web_search.is_some() {
        body["tools"] = json!([{ "google_search": {} }]);
    } else if !tools.is_empty() {
        body["tools"] = json!([{ "functionDeclarations": gemini_declarations(tools) }]);
        let mode = match controls.choice {
            ToolChoice::Auto     => None,
            ToolChoice::Required => Some("ANY"),
            ToolChoice::NoTools => Some("NONE"),
        };
        if let Some(mode) = mode {
            body["toolConfig"] = json!({ "functionCallingConfig": { "mode": mode } });
        }
    }

    let record = crate::provider::ProviderRegistry::global()
        .get("gemini")
        .expect("gemini always registered");
    // registry-managed: auth header
    let response = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post(&endpoint).header("Content-Type", "application/json"),
        api_key,
    )
        .json(&body)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| send_error(e, timeout))?;

    let status = response.status().as_u16();
    let json: Value = crate::nodes::util::read_json_response_capped(response)
        .await
        .map_err(|e| agent_status_error(status, e))?;

    if let Some(msg) = agent_response_error(status, &json, "Unknown Gemini API error") {
        return Err(agent_status_error(status, msg));
    }

    Ok(json)
}

/// Gemini function declarations from parsed tools. `type` and `strict` belong
/// to the OpenAI tool shape; Gemini rejects a declaration that carries them.
/// The schema goes under `parametersJsonSchema`, which takes full JSON Schema
/// including `additionalProperties`; the older `parameters` field takes an
/// OpenAPI subset and rejects that keyword. A tool with no arguments sends no
/// schema.
fn gemini_declarations(tools: &[Value]) -> Vec<Value> {
    tools.iter().map(|t| {
        let mut def = t.clone();
        if let Some(m) = def.as_object_mut() {
            m.remove("type");
            m.remove("strict");
            if let Some(schema) = m.remove("parameters") {
                if !is_blank_schema(&schema) {
                    m.insert("parametersJsonSchema".to_string(), schema);
                }
            }
        }
        def
    }).collect()
}

/// Queries, sources and the Search Suggestions HTML from a Gemini candidate's
/// `groundingMetadata`.
fn gemini_grounding(candidate: &Value) -> Grounding {
    let mut grounding = Grounding::default();
    let meta = &candidate["groundingMetadata"];
    grounding.suggestions = meta["searchEntryPoint"]["renderedContent"].as_str().unwrap_or("").to_string();
    for q in meta["webSearchQueries"].as_array().into_iter().flatten() {
        if let Some(q) = q.as_str() {
            grounding.add_query(q);
        }
    }
    for chunk in meta["groundingChunks"].as_array().into_iter().flatten() {
        grounding.add_source(chunk["web"]["title"].as_str().unwrap_or(""), chunk["web"]["uri"].as_str().unwrap_or(""));
    }
    grounding
}

/// A Gemini reply turn with `role` filled in when the provider left it out, so
/// the stored copy passes the role check on its way back in. Nothing else in
/// it is touched: thought parts and signatures must go back as received.
fn with_default_role(message: &Value, role: &str) -> Option<Value> {
    if !message.is_object() {
        return None;
    }
    let mut copy = message.clone();
    if copy["role"].is_null() {
        copy["role"] = json!(role);
    }
    Some(copy)
}
