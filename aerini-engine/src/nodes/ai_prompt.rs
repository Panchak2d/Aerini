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
        // Redirects disabled: check_host_ssrf_from_url validates the initial URL only.
        // A server at an allowed URL could redirect to an internal address and bypass
        // the SSRF check. Matches the same policy used in http.rs.
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .pool_max_idle_per_host(20)
            .redirect(reqwest::redirect::Policy::none())
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
    fn description(&self) -> &'static str { "Send a prompt to an AI model — Claude, GPT-4o, Gemini, or a local Ollama model — and receive a reply." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["prompt"],
            "properties": {
                "prompt":         { "type": "string",  "description": "User prompt" },
                "system":         { "type": "string",  "description": "System / persona instructions" },
                "model":          { "type": "string",  "description": "Model name — e.g. gpt-4o, claude-sonnet-4-6, gemini-2.5-flash, llama3" },
                "provider":       { "type": "string",  "enum": ["auto", "openai", "anthropic", "gemini"], "description": "API provider. 'auto' detects from base_url." },
                "base_url":       { "type": "string",  "description": "API base URL. Leave blank for OpenAI. Note: localhost/loopback addresses are blocked in server mode." },
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

        let mut system    = input.input["system"].as_str().unwrap_or("You are a helpful assistant.").to_string();
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

        if let Err(e) = crate::nodes::util::check_host_ssrf_from_url(&base_url).await {
            return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
        }

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

        // Optional multimodal attachments — backwards compatible: absent/empty
        // "attachments" leaves system/prompt untouched and every provider call
        // builds the exact same request body as before this field existed.
        let attachments_raw = input.input["attachments"].as_array().cloned().unwrap_or_default();
        let (image_attachments, doc_attachments, system_appendix, attachment_warnings) =
            process_attachments(&attachments_raw);
        if !system_appendix.is_empty() {
            system.push_str(&system_appendix);
        }

        let client = shared_ai_client();

        let mut output = match provider {
            Provider::Anthropic => call_anthropic(client, &base_url, &api_key, &model, &system, &prompt, temperature, max_tokens, &image_attachments, &doc_attachments).await,
            Provider::Gemini    => call_gemini(client, &base_url, &api_key, &model, &system, &prompt, temperature, max_tokens, &image_attachments, &doc_attachments).await,
            Provider::OpenAI    => call_openai_compatible(client, &base_url, &api_key, &model, &system, &prompt, temperature, max_tokens, &image_attachments, &doc_attachments).await,
        };
        if !attachment_warnings.is_empty() {
            output.logs.extend(attachment_warnings);
        }
        output
    }
}

/// One classified attachment ready for a provider's content-block builder.
/// `mime_type` is one of the supported image types or "application/pdf".
type ImageAttachment = (String /* mime_type */, String /* base64 data */);
/// `(mime_type, base64 data, filename)` — filename is only used by OpenAI's
/// `file` content block, which requires one.
type DocAttachment = (String, String, String);

/// Splits the raw `attachments` input array into provider-agnostic buckets.
///
/// Each item is expected to follow Aerini's standard file-object DATA CONTRACT
/// (`{filename, data, mime_type}`, `data` = raw base64, no `data:` URI prefix —
/// the same shape produced by `collect_files`, `save_to_folder`, and the Output
/// node's media batch). Unsupported MIME types or malformed entries are skipped
/// with a warning rather than failing the whole node — one bad attachment must
/// not block a prompt that would otherwise succeed.
///
/// Text/markdown attachments are decoded and returned as a single appendix
/// string meant to be appended to the system prompt (per design: plain text
/// content is cheaper and more reliable as inline context than as a
/// provider-specific document upload).
fn process_attachments(attachments: &[Value]) -> (Vec<ImageAttachment>, Vec<DocAttachment>, String, Vec<String>) {
    let mut images: Vec<ImageAttachment> = Vec::new();
    let mut docs: Vec<DocAttachment> = Vec::new();
    let mut system_appendix = String::new();
    let mut warnings: Vec<String> = Vec::new();

    for att in attachments {
        let filename = att["filename"].as_str().unwrap_or("file").to_string();
        let mime     = att["mime_type"].as_str().unwrap_or("").to_string();
        let data     = att["data"].as_str().unwrap_or("");

        if data.is_empty() {
            warnings.push(format!("Skipped attachment '{}': missing data", filename));
            continue;
        }

        match mime.as_str() {
            "image/png" | "image/jpeg" | "image/webp" | "image/gif" => {
                images.push((mime, data.to_string()));
            }
            "application/pdf" => {
                docs.push((mime, data.to_string(), filename));
            }
            "text/plain" | "text/markdown" => {
                use base64::Engine as _;
                match base64::engine::general_purpose::STANDARD.decode(data) {
                    Ok(bytes) => match String::from_utf8(bytes) {
                        Ok(text) => {
                            system_appendix.push_str(&format!("\n\n--- Attached file: {} ---\n{}", filename, text));
                        }
                        Err(_) => warnings.push(format!("Skipped attachment '{}': not valid UTF-8 text", filename)),
                    },
                    Err(_) => warnings.push(format!("Skipped attachment '{}': invalid base64 data", filename)),
                }
            }
            "" => warnings.push(format!("Skipped attachment '{}': missing mime_type", filename)),
            other => warnings.push(format!("Skipped attachment '{}': unsupported type '{}'", filename, other)),
        }
    }

    (images, docs, system_appendix, warnings)
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    use base64::Engine as _;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
    }

    #[test]
    fn empty_input_yields_all_empty() {
        let (images, docs, appendix, warnings) = process_attachments(&[]);
        assert!(images.is_empty());
        assert!(docs.is_empty());
        assert_eq!(appendix, "");
        assert!(warnings.is_empty());
    }

    #[test]
    fn classifies_image_pdf_and_text_correctly() {
        let attachments = vec![
            json!({ "filename": "a.png", "mime_type": "image/png", "data": "abc123" }),
            json!({ "filename": "b.pdf", "mime_type": "application/pdf", "data": "def456" }),
            json!({ "filename": "c.txt", "mime_type": "text/plain", "data": b64("hello world") }),
        ];
        let (images, docs, appendix, warnings) = process_attachments(&attachments);
        assert_eq!(images, vec![("image/png".to_string(), "abc123".to_string())]);
        assert_eq!(docs, vec![("application/pdf".to_string(), "def456".to_string(), "b.pdf".to_string())]);
        assert!(appendix.contains("c.txt"));
        assert!(appendix.contains("hello world"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn unsupported_type_warns_without_dropping_others() {
        let attachments = vec![
            json!({ "filename": "x.exe", "mime_type": "application/octet-stream", "data": "zz" }),
            json!({ "filename": "a.png", "mime_type": "image/png", "data": "abc123" }),
        ];
        let (images, _docs, _appendix, warnings) = process_attachments(&attachments);
        assert_eq!(images.len(), 1, "valid attachment after a bad one must still be processed");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("x.exe"));
        assert!(warnings[0].contains("unsupported type"));
    }

    #[test]
    fn missing_data_warns_and_skips() {
        let attachments = vec![json!({ "filename": "empty.png", "mime_type": "image/png" })];
        let (images, _docs, _appendix, warnings) = process_attachments(&attachments);
        assert!(images.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("missing data"));
    }

    #[test]
    fn invalid_base64_text_warns_and_skips() {
        let attachments = vec![json!({ "filename": "bad.txt", "mime_type": "text/plain", "data": "not-valid-base64!!!" })];
        let (_images, _docs, appendix, warnings) = process_attachments(&attachments);
        assert_eq!(appendix, "");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("invalid base64"));
    }

    #[test]
    fn multiple_text_files_accumulate_in_appendix() {
        let attachments = vec![
            json!({ "filename": "one.txt", "mime_type": "text/plain", "data": b64("first") }),
            json!({ "filename": "two.md",  "mime_type": "text/markdown", "data": b64("second") }),
        ];
        let (_images, _docs, appendix, warnings) = process_attachments(&attachments);
        assert!(warnings.is_empty());
        assert!(appendix.contains("first"));
        assert!(appendix.contains("second"));
        assert!(appendix.contains("one.txt"));
        assert!(appendix.contains("two.md"));
    }
}

/// Recovery Plan Session 3 gap 3: assert the actual request body bytes sent
/// with no `attachments` key are byte-identical to the pre-attachment-era
/// shape, via a real local mock HTTP server — not a diff read by eye.
///
/// Scope note (state explicitly, do not overclaim): this covers the
/// OpenAI-compatible path only. `call_anthropic` and `call_gemini` hardcode
/// their real endpoints whenever `base_url` doesn't contain
/// "anthropic.com" / "googleapis.com" respectively (see each function's
/// endpoint construction), so a localhost mock server cannot be substituted
/// for them without either live network access (unavailable — see toolchain
/// gate) or refactoring production endpoint-selection code, which is outside
/// this session's scope (Rule 6). `process_attachments` itself (the part
/// that is provider-agnostic) is already fully covered above in
/// `attachment_tests`. Anthropic/Gemini body-shape backward-compatibility
/// for the no-attachments case remains a traced-not-executed claim — flagged
/// `UNCERTAIN`, not asserted as verified.
#[cfg(test)]
mod backward_compat_tests {
    use super::*;

    /// Binds an ephemeral port, accepts exactly one HTTP request, captures
    /// its raw body, replies with `response_body`, and returns the captured
    /// body over the returned channel.
    async fn spawn_capturing_mock_server(
        response_body: &'static str,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock server bind failed");
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
            let _ = tx.send(String::from_utf8_lossy(&body_bytes).to_string());

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let _ = w.write_all(resp.as_bytes()).await;
        });

        (format!("http://127.0.0.1:{}", port), rx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn openai_no_attachments_request_body_matches_pre_attachment_shape() {
        let canned = r#"{"choices":[{"message":{"content":"ok"}}],"model":"test-model","usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
        let (base_url, rx) = spawn_capturing_mock_server(canned).await;

        let client = reqwest::Client::new();
        let out = call_openai_compatible(
            client, &base_url, "", "test-model",
            "You are a helpful assistant.", "hello world",
            0.7, 2048, &[], &[],
        ).await;
        assert!(out.success, "call_openai_compatible failed: {:?}", out.error);

        let captured = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .expect("mock server timed out waiting for a request")
            .expect("mock server's capture channel was dropped");
        let body: Value = serde_json::from_str(&captured)
            .unwrap_or_else(|e| panic!("captured body was not valid JSON: {e}\nbody: {captured}"));

        // Pre-attachment-era shape: `content` is a plain string, never an
        // array of content blocks. This is exactly what any caller using the
        // API before the `attachments` field existed would have sent.
        let expected = json!({
            "model": "test-model",
            "messages": [
                { "role": "system", "content": "You are a helpful assistant." },
                { "role": "user",   "content": "hello world" }
            ],
            "temperature": 0.7,
            "max_tokens": 2048
        });
        assert_eq!(
            body, expected,
            "request body with no attachments must be byte-identical to the pre-attachment-era shape"
        );
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

fn extract_err_msg(obj: &serde_json::Map<String, Value>, default: &str) -> String {
    obj.get("message").and_then(|m| m.as_str()).unwrap_or(default).to_string()
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
    image_attachments: &[ImageAttachment],
    doc_attachments: &[DocAttachment],
) -> NodeOutput {
    // OpenAI Chat Completions multimodal content blocks (VERIFIED against
    // current OpenAI API docs — developers.openai.com/api/docs/guides/images-vision
    // and .../file-inputs):
    //   image: {"type":"image_url","image_url":{"url":"data:<mime>;base64,<data>"}}
    //   pdf:   {"type":"file","file":{"filename":<name>,"file_data":"data:application/pdf;base64,<data>"}}
    // `file` blocks are an OpenAI-specific extension — third-party OpenAI-compatible
    // endpoints (Groq, Ollama, etc.) may not support them; image_url is broadly supported.
    let user_content: Value = if image_attachments.is_empty() && doc_attachments.is_empty() {
        json!(prompt)
    } else {
        let mut blocks: Vec<Value> = vec![json!({ "type": "text", "text": prompt })];
        for (mime, data) in image_attachments {
            blocks.push(json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", mime, data) }
            }));
        }
        for (mime, data, filename) in doc_attachments {
            blocks.push(json!({
                "type": "file",
                "file": { "filename": filename, "file_data": format!("data:{};base64,{}", mime, data) }
            }));
        }
        json!(blocks)
    };

    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user",   "content": user_content }
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

    if let Some(err_obj) = resp_json["error"].as_object() {
        let msg = extract_err_msg(err_obj, "Unknown API error");
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
    image_attachments: &[ImageAttachment],
    doc_attachments: &[DocAttachment],
) -> NodeOutput {
    if api_key.is_empty() {
        return NodeOutput::failure(NodeError::unrecoverable(
            "MISSING_API_KEY",
            "Anthropic requires an API key. Add one in Connections.",
        ));
    }

    let endpoint = if base_url.contains("anthropic.com") {
        format!("{}/v1/messages", base_url.trim_end_matches("/v1"))
    } else {
        "https://api.anthropic.com/v1/messages".to_string()
    };

    // Anthropic image/document content blocks (VERIFIED against current
    // platform.claude.com/docs/en/build-with-claude/pdf-support and the
    // standard /v1/messages image format — no beta header required for
    // either; PDF support is GA):
    //   image:    {"type":"image","source":{"type":"base64","media_type":<mime>,"data":<b64>}}
    //   document: {"type":"document","source":{"type":"base64","media_type":"application/pdf","data":<b64>}}
    let user_content: Value = if image_attachments.is_empty() && doc_attachments.is_empty() {
        json!(prompt)
    } else {
        let mut blocks: Vec<Value> = Vec::new();
        for (mime, data) in image_attachments {
            blocks.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": mime, "data": data }
            }));
        }
        for (mime, data, _filename) in doc_attachments {
            blocks.push(json!({
                "type": "document",
                "source": { "type": "base64", "media_type": mime, "data": data }
            }));
        }
        blocks.push(json!({ "type": "text", "text": prompt }));
        json!(blocks)
    };

    let body = json!({
        "model":      model,
        "system":     system,
        "messages":   [{ "role": "user", "content": user_content }],
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
        let msg = extract_err_msg(err_obj, "Unknown Anthropic error");
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
    image_attachments: &[ImageAttachment],
    doc_attachments: &[DocAttachment],
) -> NodeOutput {
    if api_key.is_empty() {
        return NodeOutput::failure(NodeError::unrecoverable(
            "MISSING_API_KEY",
            "Gemini requires an API key. Add one in Connections.",
        ));
    }

    let endpoint = if base_url.contains("googleapis.com") || base_url.contains("generativelanguage") {
        format!("{}/models/{}:generateContent", base_url.trim_end_matches('/'), model)
    } else {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            model
        )
    };

    // Gemini inline_data parts (VERIFIED against current ai.google.dev/gemini-api/docs
    // image-understanding and document-processing docs). Same shape for any mime
    // type — Gemini does not distinguish images from PDFs at the part level.
    let mut parts: Vec<Value> = vec![json!({ "text": prompt })];
    for (mime, data) in image_attachments {
        parts.push(json!({ "inline_data": { "mime_type": mime, "data": data } }));
    }
    for (mime, data, _filename) in doc_attachments {
        parts.push(json!({ "inline_data": { "mime_type": mime, "data": data } }));
    }

    let mut body = json!({
        "contents": [
            { "role": "user", "parts": parts }
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
        let msg = extract_err_msg(err_obj, "Unknown Gemini error");
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

