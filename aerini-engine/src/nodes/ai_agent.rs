use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};
use super::util::{cfg_f64_opt, cfg_u64_opt};
use super::ai_prompt::{
    process_attachments, extract_port_attachments, ImageAttachment, DocAttachment,
    extract_provider_error, ATTACHMENT_ONLY_PROMPT,
    rejected_param, RejectedParam, is_official_openai, model_or_default,
    anthropic_text, clamp_claude_temperature, gemini_model_id,
};

/// AI Agent node: gives a model a goal and a set of tools and returns its answer.
///
/// The node makes one model call. It cannot run the tools it is given: when the
/// model asks for one, the request comes back in `tool_calls` with
/// `awaiting_tools` set, and the workflow's own nodes do the work. Inventing a
/// tool result so the model could continue would let it act on made-up data and
/// bill a call per round.
///
/// **Provider behaviour:**
/// - OpenAI / OpenAI-compatible and Gemini: tool definitions are sent; the
///   reply carries the text and any tool requests.
/// - Anthropic: tools are not sent (its `tool_use` format is not used here); the
///   model answers in a single pass.
pub struct AiAgentNode;

#[async_trait]
impl Node for AiAgentNode {
    fn type_id(&self) -> &'static str { "ai_agent" }
    fn display_name(&self) -> &'static str { "AI Agent" }
    fn node_type(&self) -> NodeType { NodeType::Ai }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Give an AI model a set of tools it can invoke to complete a goal autonomously over multiple steps." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["goal", "provider"],
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "What the agent should accomplish. Be specific and include constraints."
                },
                "system": {
                    "type": "string",
                    "description": "System instructions for the agent's persona and behavior."
                },
                "tools": {
                    "type": "string",
                    "description": "JSON array of tool definitions. Each tool: {name, description, parameters}. Used with OpenAI-compatible and Gemini providers; the node returns the model's tool requests and does not run them. Anthropic ignores tools."
                },
                "context": {
                    "type": "string",
                    "description": "Additional context to give the agent (from previous nodes, variables, etc.)"
                },
                "provider": {
                    "type": "string",
                    "enum": ["openai", "anthropic", "gemini", "local", "auto"],
                    "description": "AI provider. OpenAI/Gemini: tool definitions are sent and tool requests returned. Anthropic: single reasoning pass, no tools."
                },
                "model": {
                    "type": "string",
                    "description": "Model name. OpenAI: gpt-5.6. Anthropic: claude-sonnet-5. Gemini: gemini-3.6-flash.",
                    "x-aerini-model-picker": true
                },
                "base_url": {
                    "type": "string",
                    "description": "API base URL (leave blank for default)"
                },
                "api_key": {
                    "type": "string",
                    "description": "API key — resolved from Connections"
                },
                "max_iterations": {
                    "type": "number",
                    "description": "Not used: the agent makes one call. Kept so saved workflows still load."
                },
                "max_tokens": {
                    "type": "number",
                    "description": "Maximum tokens per agent response (default: 2048). Higher values allow longer output but increase cost."
                },
                "temperature": {
                    "type": "number",
                    "description": "Creativity (0.0-1.0). Agents work best at 0.2-0.5."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "result":        { "type": "string", "description": "Final answer or conclusion from the agent" },
                "iterations":    { "type": "number", "description": "Always 1: the agent makes one model call" },
                "tool_calls":    { "type": "array",  "description": "Tools the model asked for: {id, tool, arguments, parsed_arguments}. The node does not run them." },
                "awaiting_tools":{ "type": "boolean","description": "True when the model asked for tools and has not given a final answer" },
                "reasoning":     { "type": "array",  "description": "Step-by-step reasoning trace" },
                "finished":      { "type": "boolean","description": "True if the model answered cleanly; false if it asked for tools or the reply was cut off" },
                "truncated":     { "type": "boolean","description": "True if the provider cut off the response mid-generation (max_tokens hit). Raise max_tokens to fix." }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortDefinition { id: "input".to_string(),       label: "In".to_string(),    position: PortPosition::Left, port_type: None, arity: PortArity::Single },
                // Runtime files — wired from image_gen, collect_files, text_to_file, etc.
                // Merged with config["attachments"] (static design-time files) before the first
                // agent message is built. Same merge/classify pattern as ai_prompt.rs.
                PortDefinition { id: "attachments".to_string(), label: "Files".to_string(), position: PortPosition::Left, port_type: Some("files".to_string()), arity: PortArity::Single },
            ],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Done".to_string(),  position: PortPosition::Right , port_type: None, arity: PortArity::Single },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(), position: PortPosition::Right, port_type: None, arity: PortArity::Single },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        // Attachment handling — same merge/classify pattern as ai_prompt.
        // Two sources:
        //   1. config["attachments"]      — static files added at design time.
        //   2. config["attachments_expr"] — dynamic files from the "Files" input port.
        //      Canvas.ts writes {{SourceNode.output.files}} here when a wire is connected.
        let static_atts: Vec<Value> = input.input["attachments"].as_array().cloned().unwrap_or_default();
        let port_atts: Vec<Value>   = extract_port_attachments(&input.input["attachments_expr"]);
        let mut attachments_raw = static_atts;
        attachments_raw.extend(port_atts);
        let pa = process_attachments(&attachments_raw);

        // A file-only message resolves the goal expression to "". Readable
        // attachments stand in for the text; otherwise the goal stays required.
        let goal = match input.input["goal"].as_str() {
            Some(g) if !g.trim().is_empty() => g.to_string(),
            _ if pa.has_content() => ATTACHMENT_ONLY_PROMPT.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_GOAL", "goal is required")),
        };

        let provider_str = input.input["provider"].as_str().unwrap_or("auto");
        let user_url_raw = input.input["base_url"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or("");
        let provider_id: &str = if provider_str == "auto" {
            crate::provider::ProviderRegistry::detect_from_url(user_url_raw)
        } else {
            provider_str
        };
        let is_anthropic = provider_id == "anthropic";
        let is_gemini    = provider_id == "gemini";

        // No default model exists for "local" — a blank model would otherwise
        // silently inherit the OpenAI-flagship default below and get sent to
        // whatever server base_url points to, which is very unlikely to have it.
        if provider_id == "local"
            && input.input["model"].as_str().map(|s| s.trim().is_empty()).unwrap_or(true)
        {
            return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_MODEL",
                "model is required when provider is \"local\"",
            ));
        }

        let default_model = if is_anthropic {
            "claude-sonnet-5"
        } else if is_gemini {
            "gemini-3.6-flash"
        } else {
            "gpt-5.6"
        };

        let model   = model_or_default(&input.input["model"], default_model);
        let api_key = input.input["api_key"].as_str().unwrap_or("").to_string();

        let base_url = crate::provider::ProviderRegistry::resolve_base_url(provider_id, user_url_raw);

        // AllowLocal (not Strict): same policy as ai_prompt/mod.rs —
        // local inference servers (Ollama etc.) must be reachable via base_url.
        // Single check site, upstream of the is_gemini branch, so it covers all
        // three providers (OpenAI-compatible, Anthropic, Gemini).
        if let Err(e) = crate::nodes::util::check_host_ssrf_from_url(&base_url, crate::nodes::util::SsrfPolicy::AllowLocal).await {
            return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
        }

        if let Err(e) = cfg_u64_opt(&input.input["max_iterations"], "max_iterations") {
            return NodeOutput::failure(e);
        }
        let max_tokens = match cfg_u64_opt(&input.input["max_tokens"], "max_tokens") {
            Ok(v) => v.unwrap_or(2048),
            Err(e) => return NodeOutput::failure(e),
        };
        let temperature = match cfg_f64_opt(&input.input["temperature"], "temperature") {
            Ok(v) => v.unwrap_or(0.3),
            Err(e) => return NodeOutput::failure(e),
        };

        let mut system = input.input["system"].as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("You are a helpful AI agent. Complete the given goal step by step. When you have finished, provide a clear final answer.")
            .to_string();

        let context_str = input.input["context"].as_str().unwrap_or("").to_string();

        let tools = match parse_tools(&input.input["tools"]) {
            Ok(t) => t,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("INVALID_TOOLS", e)),
        };

        let client = crate::provider::shared_ai_client();

        let image_attachments   = pa.images;
        let doc_attachments     = pa.docs;
        let attachment_warnings = pa.logs;
        if !pa.warning.is_empty() {
            system.push_str(&pa.warning);
        }

        let user_content = if context_str.is_empty() {
            goal.clone()
        } else {
            format!("Context:\n{}\n\nGoal:\n{}", context_str, goal)
        };

        // Gemini uses a completely different message format, so it has its own function.
        if is_gemini {
            return run_gemini_agent(
                client, base_url, api_key, model, system,
                user_content, tools, max_tokens, temperature,
                image_attachments, doc_attachments, attachment_warnings,
            ).await;
        }

        let mut messages: Vec<Value> = Vec::new();
        // Attachment warnings go first in the reasoning trace.
        let mut reasoning_trace: Vec<String> = attachment_warnings
            .into_iter()
            .map(|w| format!("[Attachment Warning] {}", w))
            .collect();
        if is_anthropic && !tools.is_empty() {
            reasoning_trace.push("[Warning] Tools are not sent to Anthropic models; the agent answers in one pass.".to_string());
        }
        // Build the first user message. When attachments are present, the content
        // must use each provider's multimodal block format. When absent, a plain
        // string is used — identical to the pre-attachment-era shape.
        let first_content: Value = if is_anthropic {
            if image_attachments.is_empty() && doc_attachments.is_empty() {
                json!(user_content)
            } else {
                let mut blocks: Vec<Value> = Vec::new();
                for (mime, data) in &image_attachments {
                    blocks.push(json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": data } }));
                }
                for (mime, data, _filename) in &doc_attachments {
                    blocks.push(json!({ "type": "document", "source": { "type": "base64", "media_type": mime, "data": data } }));
                }
                blocks.push(json!({ "type": "text", "text": user_content }));
                json!(blocks)
            }
        } else {
            // OpenAI-compatible: image_url + file blocks
            if image_attachments.is_empty() && doc_attachments.is_empty() {
                json!(user_content)
            } else {
                let mut blocks: Vec<Value> = vec![json!({ "type": "text", "text": user_content })];
                for (mime, data) in &image_attachments {
                    blocks.push(json!({ "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", mime, data) } }));
                }
                for (mime, data, filename) in &doc_attachments {
                    blocks.push(json!({ "type": "file", "file": { "filename": filename, "file_data": format!("data:{};base64,{}", mime, data) } }));
                }
                json!(blocks)
            }
        };
        messages.push(json!({ "role": "user", "content": first_content }));

        let response = if is_anthropic {
            call_anthropic_agent(
                &client, &base_url, &api_key, &model, &system,
                &messages, temperature, max_tokens,
            ).await
        } else {
            call_openai_agent(
                &client, &base_url, &api_key, &model, &system,
                &messages, &tools, temperature, max_tokens, provider_id,
            ).await
        };
        let resp = match response {
            Ok(r)  => r,
            Err(e) => return e.into_node_output(),
        };

        let (content, finish_reason, requested): (String, String, Vec<RequestedTool>) = if is_anthropic {
            (
                anthropic_text(&resp),
                resp["stop_reason"].as_str().unwrap_or("end_turn").to_string(),
                Vec::new(),
            )
        } else {
            let choice = &resp["choices"][0];
            let calls = choice["message"]["tool_calls"].as_array()
                .map(|calls| calls.iter().map(|c| RequestedTool {
                    id:        c["id"].as_str().unwrap_or("").to_string(),
                    name:      c["function"]["name"].as_str().unwrap_or("unknown").to_string(),
                    arguments: c["function"]["arguments"].as_str().unwrap_or("{}").to_string(),
                }).collect())
                .unwrap_or_default();
            if choice["message"]["content"].as_str().unwrap_or("").is_empty() {
                if let Some(refusal) = choice["message"]["refusal"].as_str().filter(|r| !r.is_empty()) {
                    return NodeOutput::failure(NodeError::unrecoverable("REFUSED", format!("The model refused the request: {refusal}")));
                }
            }
            (
                choice["message"]["content"].as_str().unwrap_or("").to_string(),
                choice["finish_reason"].as_str().unwrap_or("stop").to_string(),
                calls,
            )
        };
        if is_anthropic && content.is_empty() && finish_reason == "refusal" {
            return NodeOutput::failure(NodeError::unrecoverable("REFUSED", "The model declined to answer this request."));
        }

        let truncated = finish_reason == "length" || finish_reason == "max_tokens";
        if truncated && content.is_empty() && requested.is_empty() {
            return NodeOutput::failure(NodeError::unrecoverable(
                "OUTPUT_TRUNCATED",
                format!("The model used all {max_tokens} tokens before writing a reply (reasoning models count their thinking). Raise max_tokens."),
            ));
        }
        let finished = matches!(finish_reason.as_str(), "stop" | "end_turn") && !truncated && requested.is_empty();
        agent_output(content, requested, reasoning_trace, finished, truncated)
    }
}

/// A tool the model asked for. The node cannot run it: the request goes to the
/// workflow in the output, so the nodes that do the work stay visible and
/// under the user's control.
struct RequestedTool {
    id:        String,
    name:      String,
    arguments: String,
}

fn agent_output(
    content: String,
    requested: Vec<RequestedTool>,
    mut reasoning: Vec<String>,
    finished: bool,
    truncated: bool,
) -> NodeOutput {
    if !content.is_empty() {
        reasoning.push(format!("[Reply] {}", content));
    }
    let awaiting_tools = !requested.is_empty();
    let tool_calls: Vec<Value> = requested.iter().map(|t| {
        reasoning.push(format!("[Tool Call] {} with args: {}", t.name, t.arguments));
        let parsed = serde_json::from_str::<Value>(&t.arguments).unwrap_or(Value::Null);
        json!({ "id": t.id, "tool": t.name, "arguments": t.arguments, "parsed_arguments": parsed })
    }).collect();
    let log = if awaiting_tools {
        format!("Agent requested {} tool call(s); it does not run tools, route them with the workflow", tool_calls.len())
    } else {
        "Agent replied".to_string()
    };
    NodeOutput::success_with_logs(
        json!({
            "result":         content,
            "iterations":     1,
            "tool_calls":     tool_calls,
            "awaiting_tools": awaiting_tools,
            "reasoning":      reasoning,
            "finished":       finished,
            "truncated":      truncated
        }),
        vec![log],
    )
}

// ── API error classification ──────────────────────────────────────────
//
// `call_gemini_agent`/`call_openai_agent`/`call_anthropic_agent` duplicate
// ai_prompt/shared.rs's send_and_parse request/response shape (send, cap-read,
// parse) and carry the same recoverable/unrecoverable distinction ai_prompt
// has: a timed-out/connection-failed send, an HTTP 429 or 529 (rate limited,
// overloaded) and a 502/503/504 gateway failure are Recoverable, including
// when the body is not JSON (a proxy's HTML error page); every other failure
// (auth, bad request, oversized body, a non-JSON body on any other status)
// stays Unrecoverable. A model call creates nothing, so repeating it after a
// gateway failure can at most bill twice. Every call site below must go
// through `agent_status_error` rather than wrapping every error in
// `NodeError::unrecoverable` regardless of cause: a rate-limited Agent node
// must stay retry-eligible like the otherwise-identical Prompt node hitting
// the same API.
enum AgentApiError {
    Recoverable(String),
    Unavailable(String),
    Unrecoverable(String),
}

impl AgentApiError {
    fn into_node_output(self) -> NodeOutput {
        match self {
            AgentApiError::Recoverable(msg) => {
                NodeOutput::failure(NodeError::recoverable("RATE_LIMITED", msg))
            }
            AgentApiError::Unavailable(msg) => {
                NodeOutput::failure(NodeError::recoverable("UPSTREAM_UNAVAILABLE", msg))
            }
            AgentApiError::Unrecoverable(msg) => {
                NodeOutput::failure(NodeError::unrecoverable("API_ERROR", msg))
            }
        }
    }
}

fn agent_status_error(status: u16, msg: String) -> AgentApiError {
    match status {
        429 | 529 => AgentApiError::Recoverable(msg),
        502..=504 => AgentApiError::Unavailable(msg),
        _ => AgentApiError::Unrecoverable(msg),
    }
}

/// The failure message for a provider response, or `None` for a good one. A
/// 4xx/5xx status with no `error` field is a failure, not an empty success.
fn agent_response_error(status: u16, json: &Value, default: &str) -> Option<String> {
    extract_provider_error(json, default).or_else(|| (status >= 400).then(|| format!("{default} (HTTP {status})")))
}

// ── Gemini agent loop ────────────────────────────────────────────────────────
//
// Gemini message format differs fundamentally from OpenAI:
//   - contents[].role is "user" | "model" (not "user" | "assistant" | "tool")
//   - parts[] holds text or functionCall objects
//   - tool results are sent as role:"user" with parts[].functionResponse
//   - tools declared as {"functionDeclarations":[{name, description, parameters}]}
//   - finish is candidates[0].finishReason == "STOP"
//
// Docs: https://ai.google.dev/api/generate-content
//       https://ai.google.dev/gemini-api/docs/function-calling

#[allow(clippy::too_many_arguments)]
async fn run_gemini_agent(
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    system: String,
    user_content: String,
    tools: Vec<Value>,
    max_tokens: u64,
    temperature: f64,
    image_attachments: Vec<ImageAttachment>,
    doc_attachments: Vec<DocAttachment>,
    attachment_warnings: Vec<String>,
) -> NodeOutput {
    if api_key.is_empty() {
        return NodeOutput::failure(NodeError::unrecoverable(
            "MISSING_API_KEY",
            "Gemini requires an API key. Add one in Connections.",
        ));
    }

    let mut first_parts: Vec<Value> = vec![json!({ "text": user_content })];
    for (mime, data) in &image_attachments {
        first_parts.push(json!({ "inline_data": { "mime_type": mime, "data": data } }));
    }
    for (mime, data, _filename) in &doc_attachments {
        first_parts.push(json!({ "inline_data": { "mime_type": mime, "data": data } }));
    }
    let contents: Vec<Value> = vec![json!({ "role": "user", "parts": first_parts })];
    let reasoning_trace: Vec<String> = attachment_warnings
        .into_iter()
        .map(|w| format!("[Attachment Warning] {}", w))
        .collect();

    let resp = match call_gemini_agent(
        &client, &base_url, &api_key, &model, &system,
        &contents, &tools, temperature, max_tokens,
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
            arguments: p["functionCall"]["args"].to_string(),
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
    agent_output(text_content, requested, reasoning_trace, finished, truncated)
}

// Gemini generateContent HTTP call.
//
// Endpoint: POST {base_url}/models/{model}:generateContent
// Auth:     x-goog-api-key header
// Tools:    {"functionDeclarations":[{name, description, parameters}]}
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
    temperature: f64,
    max_tokens: u64,
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

    // Convert tools from {name, description, parameters} to Gemini functionDeclarations format.
    // Input tools array is already in the right shape — wrap in the required object.
    if !tools.is_empty() {
        body["tools"] = json!([{ "functionDeclarations": tools }]);
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
        .send()
        .await
        .map_err(|e| {
            let recoverable = super::util::is_retryable_network_error(super::util::Replay::Safe, &e);
            if recoverable {
                AgentApiError::Recoverable(super::util::reqwest_err_msg(&e))
            } else {
                AgentApiError::Unrecoverable(super::util::reqwest_err_msg(&e))
            }
        })?;

    let status = response.status().as_u16();
    let json: Value = crate::nodes::util::read_json_response_capped(response)
        .await
        .map_err(|e| agent_status_error(status, e))?;

    if let Some(msg) = agent_response_error(status, &json, "Unknown Gemini API error") {
        return Err(agent_status_error(status, msg));
    }

    Ok(json)
}

// ── OpenAI-compatible agent call ─────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn call_openai_agent(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    messages: &[Value],
    tools: &[Value],
    temperature: f64,
    max_tokens: u64,
    provider_id: &str,
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
            body["tool_choice"] = json!("auto");
        }

        let req = crate::provider::ProviderRegistry::apply_auth(
            record,
            client.post(format!("{}/chat/completions", base_url))
                .header("Content-Type", "application/json"),
            api_key,
        ).json(&body);

        let response = req.send().await.map_err(send_error)?;
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

        if let Some(msg) = agent_response_error(status, &json, "Unknown API error") {
            return Err(agent_status_error(status, msg));
        }
        return Ok(json);
    }
}

fn send_error(e: reqwest::Error) -> AgentApiError {
    let msg = super::util::reqwest_err_msg(&e);
    if super::util::is_retryable_network_error(super::util::Replay::Safe, &e) {
        AgentApiError::Recoverable(msg)
    } else {
        AgentApiError::Unrecoverable(msg)
    }
}

/// Parses the Tools field into `{name, description, parameters}` entries. It
/// takes the documented flat shape and the OpenAI `{type, function}` shape, as
/// JSON text or an array, and rejects anything else instead of running the
/// agent with no tools.
fn parse_tools(raw: &Value) -> Result<Vec<Value>, String> {
    let list: Vec<Value> = match raw {
        Value::Null => return Ok(Vec::new()),
        Value::String(s) if s.trim().is_empty() => return Ok(Vec::new()),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Array(a)) => a,
            Ok(_) => return Err("tools must be a JSON array of tool definitions".to_string()),
            Err(e) => return Err(format!("tools is not valid JSON: {e}")),
        },
        Value::Array(a) => a.clone(),
        _ => return Err("tools must be a JSON array of tool definitions".to_string()),
    };
    let mut out = Vec::with_capacity(list.len());
    for (i, tool) in list.iter().enumerate() {
        let def = if tool["function"].is_object() { &tool["function"] } else { tool };
        match def["name"].as_str().map(str::trim) {
            Some(n) if !n.is_empty() => out.push(def.clone()),
            _ => return Err(format!("tools[{i}] needs a non-empty \"name\"")),
        }
    }
    Ok(out)
}

/// The OpenAI chat-completions shape of parsed tools: each one under `function`.
fn openai_tools(tools: &[Value]) -> Vec<Value> {
    tools.iter().map(|t| json!({ "type": "function", "function": t })).collect()
}

// ── Anthropic agent call ──────────────────────────────────────────────────────
//
// Anthropic uses /v1/messages with x-api-key header and a different
// message/response format from OpenAI.
//
// Tools are intentionally omitted — the Anthropic provider runs in pure reasoning
// mode. The agent reasons through the goal in a single pass and returns a final
// answer. It does not iterate via tool_calls. Use OpenAI-compatible providers
// or Gemini if you need a multi-step tool-calling loop.
#[allow(clippy::too_many_arguments)]
async fn call_anthropic_agent(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    messages: &[Value],
    temperature: f64,
    max_tokens: u64,
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
        if send_temperature {
            body["temperature"] = json!(temperature);
        }

        let req = crate::provider::ProviderRegistry::apply_auth(
            record,
            client.post(format!("{}/v1/messages", base_url))
                .header("Content-Type", "application/json"),
            api_key,
        ).json(&body);

        let response = req.send().await.map_err(send_error)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use crate::node::Node;

    /// Same SSRF policy as ai_prompt/mod.rs's `loopback_base_url_is_no_longer_ssrf_blocked`
    /// — a loopback `base_url` must reach the local server (any outcome other than
    /// `SSRF_BLOCKED` proves the pre-flight check permits it).
    async fn spawn_minimal_openai_agent_mock() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            let (r, mut w) = stream.split();
            let mut reader = BufReader::new(r);
            let mut req_line = String::new();
            let _ = reader.read_line(&mut req_line).await;
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 { break; }
                if line.trim().is_empty() { break; }
                if let Some((k, v)) = line.trim().split_once(':') {
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0u8; content_length];
            if content_length > 0 {
                let _ = reader.read_exact(&mut body).await;
            }
            let resp_body = r#"{"choices":[{"message":{"content":"hello"},"finish_reason":"stop"}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                resp_body.len(), resp_body
            );
            let _ = w.write_all(resp.as_bytes()).await;
        });
        format!("http://127.0.0.1:{}", port)
    }

    /// like `spawn_minimal_openai_agent_mock`, but also captures the
    /// request line + body so a test can assert *what was sent* (specifically,
    /// which "model" value the node chose when the caller omitted one) rather
    /// than only whether a response was received. Gemini puts its model in the
    /// URL path (`/models/<model>:generateContent`), not the JSON body, so both
    /// are captured.
    struct CapturedRequest { request_line: String, body: String }

    async fn spawn_capturing_mock(resp_body: &'static str) -> (String, tokio::sync::oneshot::Receiver<CapturedRequest>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            let (r, mut w) = stream.split();
            let mut reader = BufReader::new(r);
            let mut req_line = String::new();
            let _ = reader.read_line(&mut req_line).await;
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 { break; }
                if line.trim().is_empty() { break; }
                if let Some((k, v)) = line.trim().split_once(':') {
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body_bytes = vec![0u8; content_length];
            if content_length > 0 {
                let _ = reader.read_exact(&mut body_bytes).await;
            }
            let _ = tx.send(CapturedRequest {
                request_line: req_line.trim().to_string(),
                body: String::from_utf8_lossy(&body_bytes).to_string(),
            });
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                resp_body.len(), resp_body
            );
            let _ = w.write_all(resp.as_bytes()).await;
        });
        (format!("http://127.0.0.1:{}", port), rx)
    }

    /// like `spawn_minimal_openai_agent_mock`, but returns a
    /// caller-chosen HTTP status line instead of always "200 OK" — needed to
    /// exercise the 429/529-recoverable classification path.
    async fn spawn_status_mock(status_line: &'static str, resp_body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            let (r, mut w) = stream.split();
            let mut reader = BufReader::new(r);
            let mut req_line = String::new();
            let _ = reader.read_line(&mut req_line).await;
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 { break; }
                if line.trim().is_empty() { break; }
                if let Some((k, v)) = line.trim().split_once(':') {
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0u8; content_length];
            if content_length > 0 {
                let _ = reader.read_exact(&mut body).await;
            }
            let resp = format!(
                "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                status_line, resp_body.len(), resp_body
            );
            let _ = w.write_all(resp.as_bytes()).await;
        });
        format!("http://127.0.0.1:{}", port)
    }

    fn make_input(val: serde_json::Value) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "n1".into(),
            workflow_id:  "w1".into(),
            execution_id: "e1".into(),
            input:        val,
            context:      ExecutionContext::default(),
        }
    }

    #[tokio::test]
    async fn missing_model_with_local_provider_returns_missing_model_error_without_request() {
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "say hi", "provider": "local"
        }))).await;
        assert!(!out.success);
        let err = out.error.expect("must carry NodeError");
        assert_eq!(err.code, "MISSING_MODEL");
        assert!(!err.recoverable);
    }

    /// Confirms the existing `if provider_str == "auto" {...} else { provider_str }`
    /// passthrough already lets an explicit "local" through untouched — a
    /// public-looking base_url would resolve to "openai" via detect_from_url
    /// if provider_id were wrongly re-derived, and "openai" has its own
    /// default model that would never raise this error.
    #[tokio::test]
    async fn explicit_local_provider_survives_public_looking_base_url() {
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "say hi", "provider": "local", "base_url": "https://api.example.com"
        }))).await;
        assert!(!out.success);
        let err = out.error.expect("must carry NodeError");
        assert_eq!(err.code, "MISSING_MODEL");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_provider_resolves_its_own_auth_record() {
        let base_url = spawn_minimal_openai_agent_mock().await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "say hi", "provider": "local", "model": "llama3", "base_url": base_url
        }))).await;
        assert!(out.success, "provider=\"local\" must resolve its own record, not panic or fail: {:?}", out.error);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unregistered_provider_id_falls_back_to_openai_auth_without_panicking() {
        let base_url = spawn_minimal_openai_agent_mock().await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "say hi", "provider": "custom-proxy", "base_url": base_url
        }))).await;
        assert!(out.success, "an unrecognized provider_id must fall back to openai auth, not panic: {:?}", out.error);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn loopback_base_url_is_no_longer_ssrf_blocked() {
        let base_url = spawn_minimal_openai_agent_mock().await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "say hi",
            "provider": "openai",
            "base_url": base_url
        }))).await;
        if let Some(err) = &out.error {
            assert_ne!(err.code, "SSRF_BLOCKED", "loopback base_url must be allowed under SsrfPolicy::AllowLocal");
        }
        assert!(out.success, "request should reach the local mock server and succeed: {:?}", out.error);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn omitted_model_defaults_to_current_openai_flagship() {
        let (base_url, rx) = spawn_capturing_mock(
            r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"stop"}]}"#
        ).await;
        let _ = AiAgentNode.execute(make_input(json!({
            "goal": "say hi", "provider": "openai", "base_url": base_url
        }))).await;
        let req = rx.await.expect("mock never received a request");
        let v: serde_json::Value = serde_json::from_str(&req.body).expect("request body was not valid JSON");
        assert_eq!(v["model"].as_str(), Some("gpt-5.6"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn omitted_model_defaults_to_current_anthropic_default() {
        let (base_url, rx) = spawn_capturing_mock(
            r#"{"content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn"}"#
        ).await;
        let _ = AiAgentNode.execute(make_input(json!({
            "goal": "say hi", "provider": "anthropic", "base_url": base_url
        }))).await;
        let req = rx.await.expect("mock never received a request");
        let v: serde_json::Value = serde_json::from_str(&req.body).expect("request body was not valid JSON");
        assert_eq!(v["model"].as_str(), Some("claude-sonnet-5"));
    }

    #[tokio::test]
    async fn empty_goal_without_readable_attachment_is_missing_goal() {
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "",
            "attachments": [{ "filename": "x.exe", "mime_type": "application/octet-stream", "data": "zz" }]
        }))).await;
        assert!(!out.success);
        assert_eq!(out.error.expect("must carry NodeError").code, "MISSING_GOAL");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_goal_with_image_attachment_sends_text_and_image() {
        let (base_url, rx) = spawn_capturing_mock(
            r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#
        ).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "", "provider": "openai", "base_url": base_url,
            "attachments_expr": r#"[{"filename":"a.png","mime_type":"image/png","data":"AAAA"}]"#
        }))).await;
        assert!(out.success, "attachment-only goal must run: {:?}", out.error);
        let req = rx.await.expect("mock never received a request");
        let v: serde_json::Value = serde_json::from_str(&req.body).expect("request body was not valid JSON");
        let blocks = &v["messages"][1]["content"];
        assert!(!blocks[0]["text"].as_str().unwrap_or("").is_empty(), "user text block must not be empty");
        assert_eq!(blocks[1]["type"].as_str(), Some("image_url"));
    }

    /// A 429 from an OpenAI-compatible provider is retry-eligible, not a permanent failure.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn openai_429_is_recoverable() {
        let base_url = spawn_status_mock(
            "429 Too Many Requests",
            r#"{"error":{"message":"rate limited"}}"#,
        ).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "say hi", "provider": "openai", "base_url": base_url
        }))).await;
        assert!(!out.success);
        let err = out.error.expect("expected a NodeError");
        assert!(err.recoverable, "429 must be recoverable, got: {:?}", err);
        assert_eq!(err.code, "RATE_LIMITED");
    }

    /// Anthropic's 529 ("overloaded") is recoverable, matching ai_prompt/anthropic.rs's
    /// 429-or-529 check; a plain 400 stays unrecoverable.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn anthropic_529_is_recoverable_400_is_not() {
        let overloaded_url = spawn_status_mock(
            "529 Overloaded",
            r#"{"error":{"message":"overloaded"}}"#,
        ).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "say hi", "provider": "anthropic", "base_url": overloaded_url
        }))).await;
        let err = out.error.expect("expected a NodeError");
        assert!(err.recoverable, "529 must be recoverable, got: {:?}", err);

        let bad_request_url = spawn_status_mock(
            "400 Bad Request",
            r#"{"error":{"message":"invalid request"}}"#,
        ).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "say hi", "provider": "anthropic", "base_url": bad_request_url
        }))).await;
        let err = out.error.expect("expected a NodeError");
        assert!(!err.recoverable, "400 must stay unrecoverable, got: {:?}", err);
        assert_eq!(err.code, "API_ERROR");
    }

    /// A gateway failure on a model call is retry-eligible (the call creates
    /// nothing); a plain 500 is not.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn openai_503_is_recoverable_500_is_not() {
        let unavailable_url = spawn_status_mock(
            "503 Service Unavailable",
            r#"{"error":{"message":"upstream unavailable"}}"#,
        ).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "say hi", "provider": "openai", "base_url": unavailable_url
        }))).await;
        let err = out.error.expect("expected a NodeError");
        assert!(err.recoverable, "503 must be recoverable, got: {:?}", err);
        assert_eq!(err.code, "UPSTREAM_UNAVAILABLE");

        let server_error_url = spawn_status_mock(
            "500 Internal Server Error",
            r#"{"error":{"message":"boom"}}"#,
        ).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "say hi", "provider": "openai", "base_url": server_error_url
        }))).await;
        let err = out.error.expect("expected a NodeError");
        assert!(!err.recoverable, "500 must stay unrecoverable, got: {:?}", err);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn omitted_model_defaults_to_current_gemini_flash() {
        let (base_url, rx) = spawn_capturing_mock(
            r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}]}"#
        ).await;
        let _ = AiAgentNode.execute(make_input(json!({
            "goal": "say hi", "provider": "gemini", "base_url": base_url, "api_key": "test-key"
        }))).await;
        let req = rx.await.expect("mock never received a request");
        // Gemini's model is part of the URL path, not the JSON body.
        assert!(
            req.request_line.contains("gemini-3.6-flash"),
            "expected default model 'gemini-3.6-flash' in request path, got: {}",
            req.request_line
        );
    }
    #[test]
    fn tools_field_accepts_flat_and_function_shapes_and_rejects_the_rest() {
        let flat = json!(r#"[{"name":"a","description":"d","parameters":{}}]"#);
        let wrapped = json!([{ "type": "function", "function": { "name": "b" } }]);
        assert_eq!(parse_tools(&flat).unwrap()[0]["name"], "a");
        assert_eq!(parse_tools(&wrapped).unwrap()[0]["name"], "b");
        assert!(parse_tools(&Value::Null).unwrap().is_empty());
        assert!(parse_tools(&json!("  ")).unwrap().is_empty());
        assert!(parse_tools(&json!("not json")).is_err());
        assert!(parse_tools(&json!(r#"{"name":"a"}"#)).is_err());
        assert!(parse_tools(&json!([{ "description": "no name" }])).is_err());
    }

    #[test]
    fn openai_tools_are_wrapped_under_function() {
        let wrapped = openai_tools(&[json!({ "name": "a" })]);
        assert_eq!(wrapped[0]["type"], "function");
        assert_eq!(wrapped[0]["function"]["name"], "a");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tool_request_is_returned_to_the_workflow_after_one_call() {
        let tool_reply = r#"{"choices":[{"message":{"role":"assistant","content":"","tool_calls":[{"id":"c1","type":"function","function":{"name":"lookup","arguments":"{\"q\":\"x\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        let (base_url, bodies) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, tool_reply)]).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "g", "provider": "openai", "base_url": base_url, "api_key": "k",
            "tools": r#"[{"name":"lookup","description":"d","parameters":{"type":"object","properties":{}}}]"#
        }))).await;
        assert!(out.success, "{:?}", out.error);
        let output = out.output.expect("output");
        assert_eq!(output["awaiting_tools"], true);
        assert_eq!(output["finished"], false);
        assert_eq!(output["iterations"], 1);
        assert_eq!(output["tool_calls"][0]["tool"], "lookup");
        assert_eq!(output["tool_calls"][0]["parsed_arguments"]["q"], "x");
        let seen = bodies.lock().expect("mock lock");
        assert_eq!(seen.len(), 1);
        let first: Value = serde_json::from_str(&seen[0]).expect("json");
        assert_eq!(first["tools"][0]["type"], "function");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plain_reply_is_a_finished_answer_with_no_tool_requests() {
        let reply = r#"{"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}]}"#;
        let (base_url, _) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "g", "provider": "openai", "base_url": base_url, "api_key": "k"
        }))).await;
        let output = out.output.expect("output");
        assert_eq!(output["result"], "done");
        assert_eq!(output["finished"], true);
        assert_eq!(output["awaiting_tools"], false);
    }

    #[tokio::test]
    async fn malformed_tools_fail_before_any_request() {
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "g", "provider": "openai", "tools": "not json"
        }))).await;
        assert_eq!(out.error.expect("error").code, "INVALID_TOOLS");
    }
}
