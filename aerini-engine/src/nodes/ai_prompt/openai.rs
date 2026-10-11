use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;

use super::attachments::{ImageAttachment, DocAttachment};
use super::shared::{send_and_parse, extract_provider_error, error_output, rejected_param, RejectedParam, is_official_openai};

// OpenAI Chat Completions — multimodal content blocks
// Docs: developers.openai.com/api/docs/guides/images-vision and .../file-inputs
//   image: {"type":"image_url","image_url":{"url":"data:<mime>;base64,<data>"}}
//   pdf:   {"type":"file","file":{"filename":<name>,"file_data":"data:application/pdf;base64,<data>"}}
// `file` blocks are OpenAI-specific — third-party compatible endpoints (Groq, Ollama)
// may not support them; image_url is broadly supported.

#[allow(clippy::too_many_arguments)]
pub(super) async fn call_openai_compatible(
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
    provider_id: &str,
) -> NodeOutput {
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

    let mut messages: Vec<Value> = Vec::new();
    if !system.trim().is_empty() {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.push(json!({ "role": "user", "content": user_content }));

    let record = crate::provider::ProviderRegistry::global()
        .get(provider_id)
        .expect("call_openai_compatible is only dispatched for \"openai\" or \"local\", both always registered");

    let mut send_temperature = true;
    let mut token_key = if is_official_openai(base_url) { "max_completion_tokens" } else { "max_tokens" };
    let mut swapped_token_key = false;
    let mut notes: Vec<String> = Vec::new();

    let (status, resp_json) = loop {
        let mut body = json!({ "model": model, "messages": messages });
        if send_temperature {
            body["temperature"] = json!(temperature);
        }
        body[token_key] = json!(max_tokens);

        let req = crate::provider::ProviderRegistry::apply_auth(
            record,
            client.post(format!("{}/chat/completions", base_url))
                .header("Content-Type", "application/json"),
            api_key,
        ).json(&body);

        let (status, resp_json) = match send_and_parse(req).await {
            Ok(v)  => v,
            Err(e) => return e,
        };

        match rejected_param(status, &resp_json) {
            Some(RejectedParam::Temperature) if send_temperature => {
                send_temperature = false;
                notes.push(format!("Model '{model}' does not accept a temperature; sent without it"));
            }
            Some(RejectedParam::TokenLimit) if !swapped_token_key => {
                swapped_token_key = true;
                token_key = if token_key == "max_tokens" { "max_completion_tokens" } else { "max_tokens" };
                notes.push(format!("Model '{model}' wants '{token_key}' for the token limit; resent with it"));
            }
            _ => break (status, resp_json),
        }
    };

    if let Some(mut out) = error_output(status, &resp_json, "Unknown API error") {
        out.logs.extend(notes);
        return out;
    }

    let choice     = &resp_json["choices"][0];
    let content    = choice["message"]["content"].as_str().unwrap_or("").to_string();
    let finish     = choice["finish_reason"].as_str().unwrap_or("");
    if content.is_empty() {
        if let Some(refusal) = choice["message"]["refusal"].as_str().filter(|r| !r.is_empty()) {
            return NodeOutput::failure(NodeError::unrecoverable("REFUSED", format!("The model refused the request: {refusal}")));
        }
        if finish == "length" {
            return NodeOutput::failure(NodeError::unrecoverable(
                "OUTPUT_TRUNCATED",
                format!("The model used all {max_tokens} tokens before writing a reply (reasoning models count their thinking). Raise max_tokens."),
            ));
        }
    }
    if finish == "length" {
        notes.push(format!("The reply was cut off at max_tokens ({max_tokens})"));
    }
    let model_used = resp_json["model"].as_str().unwrap_or(model).to_string();
    let input_tok  = resp_json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let output_tok = resp_json["usage"]["completion_tokens"].as_u64().unwrap_or(0);

    notes.push(format!("AI responded ({} chars, {}in/{}out tokens)", content.len(), input_tok, output_tok));
    NodeOutput::success_with_logs(
        json!({
            "content":       content,
            "model":         model_used,
            "provider":      "openai_compatible",
            "input_tokens":  input_tok,
            "output_tokens": output_tok
        }),
        notes,
    )
}

/// `GET {base}/models` — same OpenAI-compatible shape as the chat endpoint,
/// so this one function serves both "openai" and "local" (Ollama etc.).
/// `provider_id` selects the matching `ProviderRecord` for `apply_auth`,
/// since "local" is a distinct, keyless record.
pub(super) async fn list_models(
    client: reqwest::Client,
    base_url: &str,
    api_key: &str,
    provider_id: &str,
) -> Result<Vec<String>, NodeError> {
    let record = crate::provider::ProviderRegistry::global()
        .get(provider_id)
        .expect("list_models is only dispatched for \"openai\" or \"local\", both always registered");
    let req = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.get(format!("{}/models", base_url)),
        api_key,
    );

    let (status, resp_json) = send_and_parse(req).await.map_err(|out| {
        out.error.unwrap_or_else(|| NodeError::unrecoverable("NETWORK_ERROR", "request failed"))
    })?;

    if let Some(msg) = extract_provider_error(&resp_json, "Unknown API error") {
        return Err(if status == 401 || status == 403 {
            NodeError::unrecoverable("BAD_KEY", msg)
        } else {
            NodeError::unrecoverable("API_ERROR", msg)
        });
    }

    Ok(resp_json["data"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        .unwrap_or_default())
}

#[cfg(test)]
mod backward_compat_tests {
    use super::*;
    use serde_json::json;

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
            0.7, 2048, &[], &[], "openai",
        ).await;
        assert!(out.success, "call_openai_compatible failed: {:?}", out.error);

        let captured = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .expect("mock server timed out waiting for a request")
            .expect("mock server's capture channel was dropped");
        let body: Value = serde_json::from_str(&captured)
            .unwrap_or_else(|e| panic!("captured body was not valid JSON: {e}\nbody: {captured}"));

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

    const OK_REPLY: &str = r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}],"model":"m","usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
    const TEMP_REJECTED: &str = r#"{"error":{"message":"Unsupported value: 'temperature' does not support 0.7 with this model. Only the default (1) value is supported."}}"#;
    const TOKENS_REJECTED: &str = r#"{"error":{"message":"Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead."}}"#;

    async fn run_against(replies: Vec<(u16, &'static str)>, system: &str) -> (NodeOutput, Vec<Value>) {
        let (base_url, bodies) = super::super::shared::spawn_sequence_mock(replies).await;
        let out = call_openai_compatible(
            reqwest::Client::new(), &base_url, "", "m", system, "hi",
            0.7, 2048, &[], &[], "openai",
        ).await;
        let seen = bodies.lock().expect("mock lock")
            .iter().map(|b| serde_json::from_str(b).expect("request body is JSON")).collect();
        (out, seen)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn temperature_the_model_refuses_is_dropped_and_the_call_repeated_once() {
        let (out, bodies) = run_against(vec![(400, TEMP_REJECTED), (200, OK_REPLY)], "s").await;
        assert!(out.success, "{:?}", out.error);
        assert_eq!(bodies.len(), 2);
        assert!(bodies[0].get("temperature").is_some());
        assert!(bodies[1].get("temperature").is_none());
        assert!(out.logs.iter().any(|l| l.contains("temperature")));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn token_limit_key_is_swapped_when_the_model_refuses_max_tokens() {
        let (out, bodies) = run_against(vec![(400, TOKENS_REJECTED), (200, OK_REPLY)], "s").await;
        assert!(out.success, "{:?}", out.error);
        assert_eq!(bodies[0]["max_tokens"], 2048);
        assert_eq!(bodies[1]["max_completion_tokens"], 2048);
        assert!(bodies[1].get("max_tokens").is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refusal_that_repeats_is_not_retried_forever() {
        let (out, bodies) = run_against(vec![(400, TEMP_REJECTED), (400, TEMP_REJECTED)], "s").await;
        assert!(!out.success);
        assert_eq!(bodies.len(), 2);
        assert_eq!(out.error.expect("error").code, "API_ERROR");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_reply_cut_off_by_the_token_limit_fails_with_a_clear_code() {
        let reply = r#"{"choices":[{"message":{"content":""},"finish_reason":"length"}]}"#;
        let (out, _) = run_against(vec![(200, reply)], "s").await;
        assert_eq!(out.error.expect("error").code, "OUTPUT_TRUNCATED");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blank_system_prompt_sends_no_system_message() {
        let (out, bodies) = run_against(vec![(200, OK_REPLY)], "  ").await;
        assert!(out.success);
        let msgs = bodies[0]["messages"].as_array().expect("messages");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_openai_compatible_resolves_local_provider_record() {
        let canned = r#"{"choices":[{"message":{"content":"ok"}}],"model":"llama3","usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
        let (base_url, _rx) = spawn_capturing_mock_server(canned).await;
        let out = call_openai_compatible(
            reqwest::Client::new(), &base_url, "", "llama3",
            "You are a helpful assistant.", "hello world",
            0.7, 2048, &[], &[], "local",
        ).await;
        assert!(out.success, "provider_id=\"local\" must resolve its own record, not panic or fail: {:?}", out.error);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_models_parses_data_array_into_ids() {
        let canned = r#"{"data":[{"id":"gpt-5.6"},{"id":"gpt-5.6-mini"}]}"#;
        let (base_url, _rx) = spawn_capturing_mock_server(canned).await;
        let ids = list_models(reqwest::Client::new(), &base_url, "", "openai")
            .await
            .expect("list_models should succeed against a canned data array");
        assert_eq!(ids, vec!["gpt-5.6".to_string(), "gpt-5.6-mini".to_string()]);
    }
}
