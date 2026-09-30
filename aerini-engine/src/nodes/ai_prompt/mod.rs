mod attachments;
mod shared;
mod openai;
mod anthropic;
mod gemini;

// Re-export the types and helpers that ai_agent.rs imports via `super::ai_prompt::`.
// Keeps ai_agent.rs import path unchanged after the module split.
pub(crate) use attachments::{
    ImageAttachment,
    DocAttachment,
    process_attachments,
    extract_port_attachments,
    ATTACHMENT_ONLY_PROMPT,
};
pub(crate) use shared::extract_provider_error;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

pub struct AiPromptNode;

#[async_trait]
impl Node for AiPromptNode {
    fn type_id(&self) -> &'static str { "ai_prompt" }
    fn display_name(&self) -> &'static str { "AI Prompt" }
    fn node_type(&self) -> NodeType { NodeType::Ai }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Send a prompt to an AI model — Claude, GPT, Gemini, or a local Ollama model — and receive a reply." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["prompt"],
            "properties": {
                "prompt":         { "type": "string",  "description": "User prompt" },
                "system":         { "type": "string",  "description": "System / persona instructions" },
                "model":          { "type": "string",  "description": "Model name — e.g. gpt-5.6, claude-sonnet-5, gemini-3.6-flash, llama3", "x-aerini-model-picker": true },
                "provider":       { "type": "string",  "enum": ["auto", "openai", "anthropic", "gemini", "local"], "description": "API provider. 'auto' detects from base_url." },
                "base_url":       { "type": "string",  "description": "API base URL. Leave blank for OpenAI. Loopback and private-network addresses are allowed here, for local models such as Ollama." },
                "api_key":        { "type": "string",  "description": "API key — resolved from Connections" },
                "temperature":    { "type": "number",  "description": "Creativity: 0.0 (precise) to 2.0 (creative). Default 0.7" },
                "max_tokens":     { "type": "number",  "description": "Maximum response tokens. Default 2048. Higher values allow longer output but increase cost and latency. The ceiling that actually applies is set by the provider/model you select above, not by this node." },
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
                PortDefinition { id: "input".to_string(),       label: "In".to_string(),    position: PortPosition::Left,  port_type: None },
                // Runtime files — wired from image_gen, collect_files, text_to_file, etc.
                // Expression {{SourceNode.output.files}} is injected here by Canvas.ts when a wire lands.
                // Merged with config["attachments"] (static design-time files) before processing.
                PortDefinition { id: "attachments".to_string(), label: "Files".to_string(), position: PortPosition::Left,  port_type: Some("files".to_string()) },
            ],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Success".to_string(), position: PortPosition::Right, port_type: None },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(),   position: PortPosition::Right, port_type: None },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        // Two attachment sources merged before processing:
        //   1. config["attachments"]      — static files set at design time (config panel).
        //   2. config["attachments_expr"] — dynamic files from the "Files" input port.
        //      Canvas.ts writes {{SourceNode.output.files}} here when a wire is connected.
        //      The executor resolves the expression to a JSON string; extract_port_attachments()
        //      parses it back into items so process_attachments() can classify them normally.
        let static_atts: Vec<Value> = input.input["attachments"].as_array().cloned().unwrap_or_default();
        let port_atts: Vec<Value>   = extract_port_attachments(&input.input["attachments_expr"]);
        let mut attachments_raw = static_atts;
        attachments_raw.extend(port_atts);
        let pa = process_attachments(&attachments_raw);

        // A message that is only files (e.g. a Chat panel send with no text) resolves
        // the prompt expression to "". Usable attachment content stands in for the
        // text; without it there is nothing to send and the prompt stays required.
        let prompt = match input.input["prompt"].as_str() {
            Some(p) if !p.trim().is_empty() => p.to_string(),
            _ if pa.has_content() => ATTACHMENT_ONLY_PROMPT.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_PROMPT", "prompt field is required")),
        };

        // captured before the `.unwrap_or(...)` default below collapses
        // "field absent" and "field present" into indistinguishable strings —
        // gemini.rs needs this to know whether the user actually configured a
        // system prompt, since a magic-string comparison can't tell that apart
        // from a user who explicitly typed the exact default sentence.
        let system_provided = input.input["system"].as_str().is_some();
        let mut system  = input.input["system"].as_str().unwrap_or("You are a helpful assistant.").to_string();
        let api_key     = input.input["api_key"].as_str().unwrap_or("").to_string();
        let temperature = input.input["temperature"].as_f64().unwrap_or(0.7);
        let max_tokens  = input.input["max_tokens"].as_u64().unwrap_or(2048);
        let rate_limit  = input.input["rate_limit_rpm"].as_u64().unwrap_or(0);

        let user_url_raw = input.input["base_url"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or("");
        let raw_provider = input.input["provider"].as_str().unwrap_or("auto");
        let provider_id: &str = match raw_provider {
            "anthropic" | "gemini" | "openai" | "local" => raw_provider,
            _ => crate::provider::ProviderRegistry::detect_from_url(user_url_raw),
        };
        let base_url = crate::provider::ProviderRegistry::resolve_base_url(provider_id, user_url_raw);

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

        // Must branch by provider_id: an Anthropic or Gemini call with no `model`
        // set would otherwise silently send an OpenAI model string to that
        // provider's API, guaranteeing a model-not-found failure. Matches
        // ai_agent.rs's pattern.
        let default_model = match provider_id {
            "anthropic" => "claude-sonnet-5",
            "gemini"    => "gemini-3.6-flash",
            _           => "gpt-5.6",
        };
        let model = input.input["model"].as_str().unwrap_or(default_model).to_string();

        // AllowLocal (not Strict): this node's own schema advertises local-model
        // support (Ollama etc.), so loopback/private-range base_urls must be
        // reachable — same precedent as image_gen/a1111.rs and image_gen/comfyui.rs.
        if let Err(e) = crate::nodes::util::check_host_ssrf_from_url(&base_url, crate::nodes::util::SsrfPolicy::AllowLocal).await {
            return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
        }

        if rate_limit > 0 {
            let delay_ms = 60_000_u64.saturating_div(rate_limit);
            if delay_ms > 0 {
                sleep(Duration::from_millis(delay_ms)).await;
            }
        }

        if !pa.warning.is_empty() {
            system.push_str(&pa.warning);
        }

        let client = crate::provider::shared_ai_client();

        let mut output = match provider_id {
            "anthropic" => anthropic::call_anthropic(client, &base_url, &api_key, &model, &system, &prompt, temperature, max_tokens, &pa.images, &pa.docs).await,
            "gemini"    => gemini::call_gemini(client, &base_url, &api_key, &model, &system, system_provided, &prompt, temperature, max_tokens, &pa.images, &pa.docs).await,
            _           => openai::call_openai_compatible(client, &base_url, &api_key, &model, &system, &prompt, temperature, max_tokens, &pa.images, &pa.docs, provider_id).await,
        };
        if !pa.logs.is_empty() {
            output.logs.extend(pa.logs);
        }
        output
    }
}

/// Discovers a provider's available models, for the credential panel and
/// node-config "Fetch Models" affordance. Mirrors `execute()`'s own
/// resolve → SSRF-check → dispatch sequence above: same `resolve_base_url`,
/// same `SsrfPolicy::AllowLocal` (this hits the same user-supplied
/// `base_url` outside node execution, so it needs the identical guard), same
/// three-way provider dispatch.
pub async fn list_models(
    provider_id: &str,
    user_base_url: &str,
    api_key: &str,
) -> Result<Vec<String>, NodeError> {
    if !matches!(provider_id, "anthropic" | "gemini" | "openai" | "local") {
        return Err(NodeError::unrecoverable(
            "UNKNOWN_PROVIDER",
            format!("Unknown provider \"{}\"", provider_id),
        ));
    }

    let base_url = crate::provider::ProviderRegistry::resolve_base_url(provider_id, user_base_url);

    if let Err(e) = crate::nodes::util::check_host_ssrf_from_url(&base_url, crate::nodes::util::SsrfPolicy::AllowLocal).await {
        return Err(NodeError::unrecoverable("SSRF_BLOCKED", e));
    }

    let client = crate::provider::shared_ai_client();
    match provider_id {
        "anthropic" => anthropic::list_models(client, &base_url, api_key).await,
        "gemini"    => gemini::list_models(client, &base_url, api_key).await,
        _           => openai::list_models(client, &base_url, api_key, provider_id).await,
    }
}

#[cfg(test)]
mod tests {
    use super::AiPromptNode;
    use crate::model::{NodeInput, ExecutionContext};
    use crate::node::Node;
    use serde_json::json;

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

    // NOTE: `model` and `base_url` are NOT required fields — both use `.unwrap_or` defaults.
    // The only hard check in execute() is prompt presence. Both tests below return before any async IO.

    #[tokio::test]
    async fn missing_prompt_field_returns_missing_prompt_error() {
        let out = AiPromptNode.execute(make_input(json!({}))).await;
        assert!(!out.success);
        let err = out.error.expect("must carry NodeError");
        assert_eq!(err.code, "MISSING_PROMPT");
        assert!(!err.recoverable);
    }

    #[tokio::test]
    async fn whitespace_only_prompt_treated_as_missing() {
        let out = AiPromptNode.execute(make_input(json!({ "prompt": "   " }))).await;
        assert!(!out.success);
        let err = out.error.expect("must carry NodeError");
        assert_eq!(err.code, "MISSING_PROMPT");
        assert!(!err.recoverable);
    }

    #[tokio::test]
    async fn empty_prompt_with_only_unusable_attachments_still_missing_prompt() {
        let out = AiPromptNode.execute(make_input(json!({
            "prompt": "",
            "attachments": [{ "filename": "x.exe", "mime_type": "application/octet-stream", "data": "zz" }]
        }))).await;
        assert!(!out.success);
        assert_eq!(out.error.expect("must carry NodeError").code, "MISSING_PROMPT");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_prompt_with_image_attachment_reaches_provider_with_image_and_text() {
        let (base_url, rx) = spawn_capturing_mock(
            r#"{"candidates":[{"content":{"parts":[{"text":"ok"}]}}]}"#
        ).await;
        let out = AiPromptNode.execute(make_input(json!({
            "prompt": "", "provider": "gemini", "base_url": base_url, "api_key": "test-key",
            "attachments_expr": r#"[{"filename":"a.png","mime_type":"image/png","data":"AAAA"}]"#
        }))).await;
        assert!(out.success, "attachment-only request must run: {:?}", out.error);
        let req = rx.await.expect("mock never received a request");
        let v: serde_json::Value = serde_json::from_str(&req.body).expect("request body was not valid JSON");
        let parts = &v["contents"][0]["parts"];
        assert!(!parts[0]["text"].as_str().unwrap_or("").is_empty(), "user text part must not be empty");
        assert_eq!(parts[1]["inline_data"]["mime_type"].as_str(), Some("image/png"));
    }

    #[tokio::test]
    async fn missing_model_with_local_provider_returns_missing_model_error_without_request() {
        let out = AiPromptNode.execute(make_input(json!({
            "prompt": "hi", "provider": "local"
        }))).await;
        assert!(!out.success);
        let err = out.error.expect("must carry NodeError");
        assert_eq!(err.code, "MISSING_MODEL");
        assert!(!err.recoverable);
    }

    /// An explicit `provider: "local"` must not be overwritten by
    /// `detect_from_url` just because `base_url` looks like an ordinary
    /// public domain. If the passthrough were missing, provider_id would
    /// fall through to `detect_from_url("https://api.example.com")` ==
    /// "openai", which has its own default model and would never raise
    /// this error — so seeing MISSING_MODEL here proves provider stayed
    /// "local".
    #[tokio::test]
    async fn explicit_local_provider_survives_public_looking_base_url() {
        let out = AiPromptNode.execute(make_input(json!({
            "prompt": "hi", "provider": "local", "base_url": "https://api.example.com"
        }))).await;
        assert!(!out.success);
        let err = out.error.expect("must carry NodeError");
        assert_eq!(err.code, "MISSING_MODEL");
    }

    /// `base_url` is SSRF-checked under `SsrfPolicy::AllowLocal`, which
    /// permits loopback, matching this node's own schema advertising
    /// local-model (Ollama) support. Spin up a real local server and confirm
    /// a loopback `base_url` reaches it (any outcome other than
    /// `SSRF_BLOCKED` proves the request was not rejected pre-flight).
    async fn spawn_minimal_openai_mock() -> String {
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
            let resp_body = r#"{"choices":[{"message":{"content":"ok"}}],"model":"m","usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                resp_body.len(), resp_body
            );
            let _ = w.write_all(resp.as_bytes()).await;
        });
        format!("http://127.0.0.1:{}", port)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn loopback_base_url_is_no_longer_ssrf_blocked() {
        let base_url = spawn_minimal_openai_mock().await;
        let out = AiPromptNode.execute(make_input(json!({
            "prompt": "hi",
            "base_url": base_url,
            "provider": "openai"
        }))).await;
        if let Some(err) = &out.error {
            assert_ne!(err.code, "SSRF_BLOCKED", "loopback base_url must be allowed under SsrfPolicy::AllowLocal");
        }
        assert!(out.success, "request should reach the local mock server and succeed: {:?}", out.error);
    }

    /// `list_models` (the model-discovery dispatch function) must run the
    /// same `SsrfPolicy::AllowLocal` gate `execute()` runs above — this is a
    /// separate call path outside node execution, so nothing else guarantees
    /// it inherits that guard.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_models_allows_loopback_base_url_for_local_provider() {
        let base_url = spawn_minimal_openai_mock().await;
        let result = super::list_models("local", &base_url, "").await;
        if let Err(e) = &result {
            assert_ne!(e.code, "SSRF_BLOCKED", "loopback base_url must be allowed under SsrfPolicy::AllowLocal");
        }
        assert!(result.is_ok(), "list_models should reach the local mock server: {:?}", result.err());
    }

    #[tokio::test]
    async fn list_models_unknown_provider_returns_error_not_panic() {
        let err = super::list_models("bogus", "http://example.com", "")
            .await
            .expect_err("an unrecognized provider_id must error, not panic downstream");
        assert_eq!(err.code, "UNKNOWN_PROVIDER");
    }

    /// like `spawn_minimal_openai_mock`, but also captures the request
    /// line + body so a test can assert which "model" value execute() chose when
    /// the caller omitted one. Gemini puts its model in the URL path, not the
    /// JSON body, so both are captured.
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

    /// `model` omitted defaults per-provider to the current model — an unbranched
    /// default would send a provider-blind model string to Anthropic/Gemini's API.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn omitted_model_defaults_to_current_openai_flagship() {
        let (base_url, rx) = spawn_capturing_mock(
            r#"{"choices":[{"message":{"content":"hi"}}],"model":"m","usage":{"prompt_tokens":1,"completion_tokens":1}}"#
        ).await;
        let _ = AiPromptNode.execute(make_input(json!({
            "prompt": "hi", "provider": "openai", "base_url": base_url
        }))).await;
        let req = rx.await.expect("mock never received a request");
        let v: serde_json::Value = serde_json::from_str(&req.body).expect("request body was not valid JSON");
        assert_eq!(v["model"].as_str(), Some("gpt-5.6"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn omitted_model_defaults_to_current_anthropic_default() {
        let (base_url, rx) = spawn_capturing_mock(
            r#"{"content":[{"type":"text","text":"hi"}]}"#
        ).await;
        let _ = AiPromptNode.execute(make_input(json!({
            "prompt": "hi", "provider": "anthropic", "base_url": base_url, "api_key": "test-key"
        }))).await;
        let req = rx.await.expect("mock never received a request");
        let v: serde_json::Value = serde_json::from_str(&req.body).expect("request body was not valid JSON");
        assert_eq!(v["model"].as_str(), Some("claude-sonnet-5"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn omitted_model_defaults_to_current_gemini_flash() {
        let (base_url, rx) = spawn_capturing_mock(
            r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]}}]}"#
        ).await;
        let _ = AiPromptNode.execute(make_input(json!({
            "prompt": "hi", "provider": "gemini", "base_url": base_url, "api_key": "test-key"
        }))).await;
        let req = rx.await.expect("mock never received a request");
        // Gemini's model is part of the URL path, not the JSON body.
        assert!(
            req.request_line.contains("gemini-3.6-flash"),
            "expected default model 'gemini-3.6-flash' in request path, got: {}",
            req.request_line
        );
    }
}
