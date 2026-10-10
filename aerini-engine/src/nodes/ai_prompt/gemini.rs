use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;

use super::attachments::{ImageAttachment, DocAttachment};
use super::shared::{send_and_parse, extract_provider_error, error_output};

// Gemini
// Docs: https://ai.google.dev/api/generate-content
// Auth: x-goog-api-key (applied via ProviderRegistry::apply_auth)
// Endpoint: POST {base_url}/models/{model}:generateContent
// Body: { contents: [{ role, parts: [{text}] }], systemInstruction: { parts: [{text}] }, generationConfig }
// Response: candidates[0].content.parts[].text
// Inline data: { "inline_data": { "mime_type": <mime>, "data": <b64> } } — same shape for images and PDFs.
// ai.google.dev/gemini-api/docs, June 2026.

/// The model id as it goes in the URL: the `models/` prefix Google's own list
/// shows is accepted in the field and not doubled.
pub(crate) fn gemini_model_id(model: &str) -> &str {
    model.strip_prefix("models/").unwrap_or(model)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn call_gemini(
    client: reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    system_provided: bool,
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

    let endpoint = format!("{}/models/{}:generateContent", base_url, gemini_model_id(model));

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
            "temperature":       temperature,
            "maxOutputTokens":   max_tokens
        }
    });

    // `system_provided` (threaded from mod.rs, captured before its own
    // `.unwrap_or(...)` default collapses "field absent" into the same string
    // as "field explicitly set to that exact sentence") is the real signal
    // for "did the user configure a system prompt" — a user who explicitly
    // typed the literal default sentence now has it sent, unlike the old
    // magic-string check, which dropped it for Gemini only (Anthropic/OpenAI
    // already send `system` unconditionally). The `system != DEFAULT_SYSTEM`
    // fallback is kept, not removed: mod.rs appends an attachment-processing
    // warning onto `system` even when no system prompt was provided, and that
    // warning must still reach Gemini the same way it already reaches
    // Anthropic/OpenAI — dropping the fallback would silently regress that.
    const DEFAULT_SYSTEM: &str = "You are a helpful assistant.";
    if !system.is_empty() && (system_provided || system != DEFAULT_SYSTEM) {
        body["systemInstruction"] = json!({
            "parts": [{ "text": system }]
        });
    }

    let record = crate::provider::ProviderRegistry::global()
        .get("gemini")
        .expect("gemini always registered");
    let req = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post(&endpoint).header("Content-Type", "application/json"),
        api_key,
    ).json(&body);

    let (status, resp_json) = match send_and_parse(req).await {
        Ok(v)  => v,
        Err(e) => return e,
    };

    // Gemini error: { "error": { "code": 400, "message": "...", "status": "..." } }
    if let Some(out) = error_output(status, &resp_json, "Unknown Gemini error") {
        return out;
    }

    let candidate = &resp_json["candidates"][0];
    let content: String = candidate["content"]["parts"]
        .as_array()
        .map(|parts| {
            parts.iter()
                .filter(|p| p["thought"] != true)
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if content.is_empty() {
        if let Some(reason) = resp_json["promptFeedback"]["blockReason"].as_str() {
            return NodeOutput::failure(NodeError::unrecoverable(
                "GEMINI_NO_CONTENT",
                format!("Gemini blocked the prompt ({reason})."),
            ));
        }
        let finish = candidate["finishReason"].as_str().unwrap_or("");
        if candidate.is_null() || (finish != "STOP" && !finish.is_empty()) {
            let why = if candidate.is_null() { "no candidates".to_string() } else { finish.to_string() };
            return NodeOutput::failure(NodeError::unrecoverable(
                "GEMINI_NO_CONTENT",
                format!("Gemini returned no text ({why}). Check max_tokens and the safety filters."),
            ));
        }
    }

    let model_used = resp_json["modelVersion"].as_str().unwrap_or(model).to_string();
    let input_tok  = resp_json["usageMetadata"]["promptTokenCount"].as_u64().unwrap_or(0);
    let output_tok = resp_json["usageMetadata"]["candidatesTokenCount"].as_u64().unwrap_or(0);

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

/// `GET {base}/models?key=<api_key>` — VERIFIED (ai.google.dev/gemini-api/docs/api-errors,
/// checked this session): ListModels takes the key as a query parameter, not
/// only the `x-goog-api-key` header `call_gemini` above uses; `apply_auth`
/// still runs too, for header/extra_headers consistency with the rest of
/// this file. Response envelope is `{"models":[{"name":"models/xxx"}]}`,
/// different from the other two providers' `{"data":[{"id":...}]}` — the
/// `"models/"` prefix is stripped so all three return bare model ids.
pub(super) async fn list_models(
    client: reqwest::Client,
    base_url: &str,
    api_key: &str,
) -> Result<Vec<String>, NodeError> {
    if api_key.is_empty() {
        return Err(NodeError::unrecoverable(
            "MISSING_API_KEY",
            "Gemini requires an API key. Add one in Connections.",
        ));
    }

    let record = crate::provider::ProviderRegistry::global()
        .get("gemini")
        .expect("gemini always registered");
    let req = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.get(format!("{}/models", base_url)).query(&[("key", api_key)]),
        api_key,
    );

    let (status, resp_json) = send_and_parse(req).await.map_err(|out| {
        out.error.unwrap_or_else(|| NodeError::unrecoverable("NETWORK_ERROR", "request failed"))
    })?;

    if let Some(msg) = extract_provider_error(&resp_json, "Unknown Gemini error") {
        // VERIFIED: Gemini returns 400/INVALID_ARGUMENT with reason
        // API_KEY_INVALID for a bad key, not 401/403 like the other two
        // providers — detected from `details[].reason`, not the HTTP status.
        let is_bad_key = resp_json["error"]["details"]
            .as_array()
            .map(|d| d.iter().any(|item| item["reason"].as_str() == Some("API_KEY_INVALID")))
            .unwrap_or(false);
        return Err(if is_bad_key || status == 403 {
            NodeError::unrecoverable("BAD_KEY", msg)
        } else {
            NodeError::unrecoverable("API_ERROR", msg)
        });
    }

    Ok(resp_json["models"]
        .as_array()
        .map(|arr| arr.iter()
            .filter_map(|m| m["name"].as_str())
            .map(|s| s.trim_start_matches("models/").to_string())
            .collect())
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal mock that always returns a valid Gemini success body and hands
    /// the captured request body back over a oneshot channel — same technique
    /// as ai_agent.rs's spawn_capturing_mock, reimplemented locally since that
    /// one is private to its own file's test module.
    async fn spawn_capturing_mock() -> (String, tokio::sync::oneshot::Receiver<String>) {
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
            let _ = tx.send(String::from_utf8_lossy(&body_bytes).to_string());
            let resp_body = r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]}}],"modelVersion":"gemini-3.6-flash","usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1}}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                resp_body.len(), resp_body
            );
            let _ = w.write_all(resp.as_bytes()).await;
        });
        (format!("http://127.0.0.1:{}", port), rx)
    }

    const DEFAULT_SYSTEM: &str = "You are a helpful assistant.";

    /// a user who explicitly typed
    /// the exact default sentence must have it sent, not silently dropped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn system_provided_with_literal_default_text_is_sent() {
        let (base_url, rx) = spawn_capturing_mock().await;
        let _ = call_gemini(
            reqwest::Client::new(), &base_url, "test-key", "gemini-3.6-flash",
            DEFAULT_SYSTEM, /* system_provided */ true,
            "hello", 0.7, 100, &[], &[],
        ).await;
        let body = rx.await.expect("mock never received a request");
        assert!(body.contains("systemInstruction"), "body: {body}");
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn system_not_provided_with_bare_default_is_omitted() {
        let (base_url, rx) = spawn_capturing_mock().await;
        let _ = call_gemini(
            reqwest::Client::new(), &base_url, "test-key", "gemini-3.6-flash",
            DEFAULT_SYSTEM, /* system_provided */ false,
            "hello", 0.7, 100, &[], &[],
        ).await;
        let body = rx.await.expect("mock never received a request");
        assert!(!body.contains("systemInstruction"), "body: {body}");
    }

    // mod.rs appends an
    // attachment-processing warning onto a defaulted `system` even when the
    // user configured nothing — that warning must still reach Gemini, same
    // as it already reaches Anthropic/OpenAI (which send unconditionally).
    // A naive "replace the check with system_provided alone" fix would have
    // silently dropped this.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn system_not_provided_but_warning_appended_is_still_sent() {
        let (base_url, rx) = spawn_capturing_mock().await;
        let system_with_warning = format!("{DEFAULT_SYSTEM} [warning: 2 attachments skipped]");
        let _ = call_gemini(
            reqwest::Client::new(), &base_url, "test-key", "gemini-3.6-flash",
            &system_with_warning, /* system_provided */ false,
            "hello", 0.7, 100, &[], &[],
        ).await;
        let body = rx.await.expect("mock never received a request");
        assert!(body.contains("systemInstruction"), "body: {body}");
        assert!(body.contains("attachments skipped"), "body: {body}");
    }

    async fn spawn_status_mock(status_line: &'static str, response_body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            let (r, mut w) = stream.split();
            let mut reader = BufReader::new(r);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 { break; }
                if line.trim().is_empty() { break; }
            }
            let resp = format!(
                "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(), response_body
            );
            let _ = w.write_all(resp.as_bytes()).await;
        });
        format!("http://127.0.0.1:{}", port)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_models_strips_models_prefix() {
        let base_url = spawn_status_mock(
            "HTTP/1.1 200 OK",
            r#"{"models":[{"name":"models/gemini-3.6-flash"},{"name":"models/gemini-3.6-pro"}]}"#,
        ).await;
        let ids = list_models(reqwest::Client::new(), &base_url, "test-key")
            .await
            .expect("list_models should succeed against a canned models array");
        assert_eq!(ids, vec!["gemini-3.6-flash".to_string(), "gemini-3.6-pro".to_string()]);
    }

    /// Gemini's bad-key error is 400/INVALID_ARGUMENT, not 401/403 — this
    /// pins the `details[].reason == "API_KEY_INVALID"` detection so a future
    /// edit can't quietly fall back to the generic 401/403 check the other
    /// two providers use and misclassify this as a plain API_ERROR.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_models_bad_key_400_maps_to_bad_key_not_api_error() {
        let base_url = spawn_status_mock(
            "HTTP/1.1 400 Bad Request",
            r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"reason":"API_KEY_INVALID"}]}}"#,
        ).await;
        let err = list_models(reqwest::Client::new(), &base_url, "bad-key")
            .await
            .expect_err("invalid key must surface as an error");
        assert_eq!(err.code, "BAD_KEY");
    }
}
