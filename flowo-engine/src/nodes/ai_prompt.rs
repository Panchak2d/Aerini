use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::time::{sleep, Duration};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

// Shared AI HTTP client — 120s timeout for long-running LLM inference calls.
// All concurrent AI node executions share one connection pool.
static AI_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn shared_ai_client() -> reqwest::Client {
    AI_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .pool_max_idle_per_host(20)
            .build()
            .expect("Failed to build shared AI HTTP client")
    }).clone()
}

pub struct AiPromptNode;

#[async_trait]
impl Node for AiPromptNode {
    fn type_id(&self) -> &'static str { "ai_prompt" }
    fn display_name(&self) -> &'static str { "AI Prompt" }
    fn node_type(&self) -> NodeType { NodeType::Ai }
    fn version(&self) -> &'static str { "1.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["prompt"],
            "properties": {
                "prompt":         { "type": "string",  "description": "User prompt" },
                "system":         { "type": "string",  "description": "System / persona instructions" },
                "model":          { "type": "string",  "description": "Model name — e.g. gpt-4o, claude-sonnet-4-6, gemini-2.5-flash, llama3" },
                "provider":       { "type": "string",  "enum": ["auto", "openai", "anthropic", "gemini"], "description": "API provider. 'auto' detects from base_url." },
                "base_url":       { "type": "string",  "description": "API base URL. Leave blank for OpenAI. Ollama: http://localhost:11434/v1" },
                "api_key":        { "type": "string",  "description": "API key — resolved from Connections" },
                "temperature":    { "type": "number",  "description": "Creativity: 0.0 (precise) to 2.0 (creative). Default 0.7" },
                "max_tokens":     { "type": "number",  "description": "Maximum response tokens. Default 2048" },
                "rate_limit_rpm": { "type": "number",  "description": "Max requests per minute. 0 = unlimited" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content":      { "type": "string" },
                "model":        { "type": "string" },
                "provider":     { "type": "string" },
                "input_tokens": { "type": "number" },
                "output_tokens":{ "type": "number" }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortDefinition { id: "input".to_string(), label: "In".to_string(), position: PortPosition::Left },
            ],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Success".to_string(), position: PortPosition::Right },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(),   position: PortPosition::Right },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let prompt = match input.input["prompt"].as_str() {
            Some(p) if !p.trim().is_empty() => p.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_PROMPT", "prompt field is required")),
        };

        let system        = input.input["system"].as_str().unwrap_or("You are a helpful assistant.").to_string();
        let model         = input.input["model"].as_str().unwrap_or("gpt-4o").to_string();
        let api_key       = input.input["api_key"].as_str().unwrap_or("").to_string();
        let temperature   = input.input["temperature"].as_f64().unwrap_or(0.7);
        let max_tokens    = input.input["max_tokens"].as_u64().unwrap_or(2048);
        let rate_limit    = input.input["rate_limit_rpm"].as_u64().unwrap_or(0);

        let base_url = input.input["base_url"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("https://api.openai.com/v1")
            .trim_end_matches('/')
            .to_string();

        let provider = match input.input["provider"].as_str().unwrap_or("auto") {
            "anthropic" => Provider::Anthropic,
            "gemini"    => Provider::Gemini,
            "openai"    => Provider::OpenAI,
            _           => detect_provider(&base_url),
        };

        if rate_limit > 0 {
            let delay_ms = 60_000_u64.saturating_div(rate_limit);
            if delay_ms > 0 {
                sleep(Duration::from_millis(delay_ms)).await;
            }
        }

        // Shared client — connection pool and TLS sessions reused across all AI calls.
        let client = shared_ai_client();

        match provider {
            Provider::Anthropic => call_anthropic(client, &base_url, &api_key, &model, &system, &prompt, temperature, max_tokens).await,
            Provider::Gemini    => call_gemini(client, &base_url, &api_key, &model, &system, &prompt, temperature, max_tokens).await,
            Provider::OpenAI    => call_openai_compatible(client, &base_url, &api_key, &model, &system, &prompt, temperature, max_tokens).await,
        }
    }
}

enum Provider { OpenAI, Anthropic, Gemini }

/// Detect provider from base_url when provider field is "auto".
fn detect_provider(base_url: &str) -> Provider {
    let url = base_url.to_lowercase();
    if url.contains("anthropic.com") {
        Provider::Anthropic
    } else if url.contains("googleapis.com") || url.contains("generativelanguage") {
        Provider::Gemini
    } else {
        // Covers: api.openai.com, localhost (Ollama/LM Studio), Groq, Together,
        // Mistral, OpenRouter, and any other OpenAI-compatible endpoint.
        Provider::OpenAI
    }
}

#[allow(clippy::too_many_arguments)]
async fn call_openai_compatible(
    client: reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    prompt: &str,
    temperature: f64,
    max_tokens: u64,
) -> NodeOutput {
    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user",   "content": prompt }
        ],
        "temperature": temperature,
        "max_tokens":  max_tokens
    });

    let mut req = client
        .post(format!("{}/chat/completions", base_url))
        .header("Content-Type", "application/json")
        .json(&body);

    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let (status, resp_json) = match send_and_parse(req).await {
        Ok(v)  => v,
        Err(e) => return e,
    };

    // API-level error object
    if let Some(err_obj) = resp_json["error"].as_object() {
        let msg = err_obj.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown API error").to_string();
        return if status == 429 {
            NodeOutput::failure(NodeError::recoverable("RATE_LIMITED", msg))
        } else {
            NodeOutput::failure(NodeError::unrecoverable("API_ERROR", msg))
        };
    }

    let content      = resp_json["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
    let model_used   = resp_json["model"].as_str().unwrap_or(model).to_string();
    let input_tok    = resp_json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let output_tok   = resp_json["usage"]["completion_tokens"].as_u64().unwrap_or(0);

    NodeOutput::success_with_logs(
        json!({
            "content":       content,
            "model":         model_used,
            "provider":      "openai_compatible",
            "input_tokens":  input_tok,
            "output_tokens": output_tok
        }),
        vec![format!("AI responded ({} chars, {}in/{}out tokens)", content.len(), input_tok, output_tok)],
    )
}

// Anthropic (Claude)
// Docs: https://docs.anthropic.com/en/api/messages
// Auth: x-api-key header + anthropic-version header
// Endpoint: POST /v1/messages
// Body: { model, system, messages: [{role, content}], max_tokens }
// Response: content[0].text

#[allow(clippy::too_many_arguments)]
async fn call_anthropic(
    client: reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    prompt: &str,
    temperature: f64,
    max_tokens: u64,
) -> NodeOutput {
    if api_key.is_empty() {
        return NodeOutput::failure(NodeError::unrecoverable(
            "MISSING_API_KEY",
            "Anthropic requires an API key. Add one in Connections.",
        ));
    }

    // Use provided base_url if it's the Anthropic endpoint, otherwise default.
    let endpoint = if base_url.contains("anthropic.com") {
        format!("{}/v1/messages", base_url.trim_end_matches("/v1"))
    } else {
        "https://api.anthropic.com/v1/messages".to_string()
    };

    let body = json!({
        "model":      model,
        "system":     system,
        "messages":   [{ "role": "user", "content": prompt }],
        "max_tokens": max_tokens,
        "temperature": temperature
    });

    let req = client
        .post(&endpoint)
        .header("Content-Type",      "application/json")
        .header("x-api-key",         api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body);

    let (status, resp_json) = match send_and_parse(req).await {
        Ok(v)  => v,
        Err(e) => return e,
    };

    // Anthropic error format: { "type": "error", "error": { "type": "...", "message": "..." } }
    if let Some(err_obj) = resp_json["error"].as_object() {
        let msg = err_obj.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown Anthropic error").to_string();
        return if status == 429 || status == 529 {
            NodeOutput::failure(NodeError::recoverable("RATE_LIMITED", msg))
        } else {
            NodeOutput::failure(NodeError::unrecoverable("API_ERROR", msg))
        };
    }

    // Response: content is an array, first item has type "text"
    let content = resp_json["content"]
        .as_array()
        .and_then(|arr| arr.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .unwrap_or("")
        .to_string();

    let model_used  = resp_json["model"].as_str().unwrap_or(model).to_string();
    let input_tok   = resp_json["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let output_tok  = resp_json["usage"]["output_tokens"].as_u64().unwrap_or(0);

    NodeOutput::success_with_logs(
        json!({
            "content":       content,
            "model":         model_used,
            "provider":      "anthropic",
            "input_tokens":  input_tok,
            "output_tokens": output_tok
        }),
        vec![format!("Anthropic responded ({} chars, {}in/{}out tokens)", content.len(), input_tok, output_tok)],
    )
}

// Gemini
// Docs: https://ai.google.dev/api/generate-content
// Auth: x-goog-api-key request header (preferred over query parameter)
// Endpoint: POST https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent
// Body: { contents: [{ role, parts: [{ text }] }], systemInstruction: { parts: [{ text }] }, generationConfig }
// Response: candidates[0].content.parts[0].text

#[allow(clippy::too_many_arguments)]
async fn call_gemini(
    client: reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    prompt: &str,
    temperature: f64,
    max_tokens: u64,
) -> NodeOutput {
    if api_key.is_empty() {
        return NodeOutput::failure(NodeError::unrecoverable(
            "MISSING_API_KEY",
            "Gemini requires an API key. Add one in Connections.",
        ));
    }

    // Build endpoint. Use provided base_url if it already looks like a Gemini URL,
    // otherwise use the canonical Google AI endpoint.
    let endpoint = if base_url.contains("googleapis.com") || base_url.contains("generativelanguage") {
        format!("{}/models/{}:generateContent", base_url.trim_end_matches('/'), model)
    } else {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            model
        )
    };

    let mut body = json!({
        "contents": [
            { "role": "user", "parts": [{ "text": prompt }] }
        ],
        "generationConfig": {
            "temperature":  temperature,
            "maxOutputTokens": max_tokens
        }
    });

    // System instruction is a separate top-level field in Gemini
    if !system.is_empty() && system != "You are a helpful assistant." {
        body["systemInstruction"] = json!({
            "parts": [{ "text": system }]
        });
    }

    let req = client
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .header("x-goog-api-key", api_key)
        .json(&body);

    let (status, resp_json) = match send_and_parse(req).await {
        Ok(v)  => v,
        Err(e) => return e,
    };

    // Gemini error format: { "error": { "code": 400, "message": "...", "status": "..." } }
    if let Some(err_obj) = resp_json["error"].as_object() {
        let msg = err_obj.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown Gemini error").to_string();
        return if status == 429 {
            NodeOutput::failure(NodeError::recoverable("RATE_LIMITED", msg))
        } else {
            NodeOutput::failure(NodeError::unrecoverable("API_ERROR", msg))
        };
    }

    let content = resp_json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let model_used   = resp_json["modelVersion"].as_str().unwrap_or(model).to_string();
    let input_tok    = resp_json["usageMetadata"]["promptTokenCount"].as_u64().unwrap_or(0);
    let output_tok   = resp_json["usageMetadata"]["candidatesTokenCount"].as_u64().unwrap_or(0);

    NodeOutput::success_with_logs(
        json!({
            "content":       content,
            "model":         model_used,
            "provider":      "gemini",
            "input_tokens":  input_tok,
            "output_tokens": output_tok
        }),
        vec![format!("Gemini responded ({} chars, {}in/{}out tokens)", content.len(), input_tok, output_tok)],
    )
}

async fn send_and_parse(req: reqwest::RequestBuilder) -> Result<(u16, Value), NodeOutput> {
    let response = match req.send().await {
        Ok(r)  => r,
        Err(e) => {
            let recoverable = e.is_timeout() || e.is_connect();
            return Err(if recoverable {
                NodeOutput::failure(NodeError::recoverable("NETWORK_ERROR", e.to_string()))
            } else {
                NodeOutput::failure(NodeError::unrecoverable("NETWORK_ERROR", e.to_string()))
            });
        }
    };

    let status = response.status().as_u16();

    match response.json::<Value>().await {
        Ok(json) => Ok((status, json)),
        Err(e)   => Err(NodeOutput::failure(NodeError::unrecoverable("PARSE_ERROR", e.to_string()))),
    }
}
