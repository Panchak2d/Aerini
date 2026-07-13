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
};

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
                // Merged with config["attachments"] (static design-time files from P6) before processing.
                PortDefinition { id: "attachments".to_string(), label: "Files".to_string(), position: PortPosition::Left,  port_type: Some("files".to_string()) },
            ],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Success".to_string(), position: PortPosition::Right, port_type: None },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(),   position: PortPosition::Right, port_type: None },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let prompt = match input.input["prompt"].as_str() {
            Some(p) if !p.trim().is_empty() => p.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_PROMPT", "prompt field is required")),
        };

        let mut system  = input.input["system"].as_str().unwrap_or("You are a helpful assistant.").to_string();
        let model       = input.input["model"].as_str().unwrap_or("gpt-4o").to_string();
        let api_key     = input.input["api_key"].as_str().unwrap_or("").to_string();
        let temperature = input.input["temperature"].as_f64().unwrap_or(0.7);
        let max_tokens  = input.input["max_tokens"].as_u64().unwrap_or(2048);
        let rate_limit  = input.input["rate_limit_rpm"].as_u64().unwrap_or(0);

        let user_url_raw = input.input["base_url"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or("");
        let raw_provider = input.input["provider"].as_str().unwrap_or("auto");
        let provider_id: &str = match raw_provider {
            "anthropic" | "gemini" | "openai" => raw_provider,
            _ => crate::provider::ProviderRegistry::detect_from_url(user_url_raw),
        };
        let base_url = crate::provider::ProviderRegistry::resolve_base_url(provider_id, user_url_raw);

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

        // Two attachment sources merged before processing:
        //   1. config["attachments"]      — static files set at design time (P6 config panel).
        //   2. config["attachments_expr"] — dynamic files from the "Files" input port (P8).
        //      Canvas.ts writes {{SourceNode.output.files}} here when a wire is connected.
        //      The executor resolves the expression to a JSON string; extract_port_attachments()
        //      parses it back into items so process_attachments() can classify them normally.
        let static_atts: Vec<Value> = input.input["attachments"].as_array().cloned().unwrap_or_default();
        let port_atts: Vec<Value>   = extract_port_attachments(&input.input["attachments_expr"]);
        let mut attachments_raw = static_atts;
        attachments_raw.extend(port_atts);
        let pa = process_attachments(&attachments_raw);
        if !pa.warning.is_empty() {
            system.push_str(&pa.warning);
        }

        let client = crate::provider::shared_ai_client();

        let mut output = match provider_id {
            "anthropic" => anthropic::call_anthropic(client, &base_url, &api_key, &model, &system, &prompt, temperature, max_tokens, &pa.images, &pa.docs).await,
            "gemini"    => gemini::call_gemini(client, &base_url, &api_key, &model, &system, &prompt, temperature, max_tokens, &pa.images, &pa.docs).await,
            _           => openai::call_openai_compatible(client, &base_url, &api_key, &model, &system, &prompt, temperature, max_tokens, &pa.images, &pa.docs).await,
        };
        if !pa.logs.is_empty() {
            output.logs.extend(pa.logs);
        }
        output
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
            node_id:      "n1".into(),
            workflow_id:  "w1".into(),
            execution_id: "e1".into(),
            input:        val,
            context:      ExecutionContext::default(),
        }
    }

    // NOTE: `model` and `base_url` are NOT required fields — both use `.unwrap_or` defaults.
    // Plan section §PATCH 21 listed them as required; that was stale. The only hard check
    // in execute() is prompt presence. Both tests below return before any async IO.

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

    /// T1-11 (S2-2): `base_url` was SSRF-checked under `SsrfPolicy::Strict`,
    /// which blocks loopback unconditionally — despite this node's own schema
    /// advertising local-model (Ollama) support. Spin up a real local server
    /// and confirm a loopback `base_url` now reaches it (any outcome other
    /// than `SSRF_BLOCKED` proves the request was not rejected pre-flight).
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
}
