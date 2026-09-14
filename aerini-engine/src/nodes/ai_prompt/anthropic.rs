use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;

use super::attachments::{ImageAttachment, DocAttachment};
use super::shared::{send_and_parse, extract_provider_error};

// Anthropic (Claude)
// Docs: https://docs.anthropic.com/en/api/messages
// Auth: x-api-key + anthropic-version (applied via ProviderRegistry::apply_auth)
// Endpoint: POST /v1/messages
// Image block:    {"type":"image","source":{"type":"base64","media_type":<mime>,"data":<b64>}}
// Document block: {"type":"document","source":{"type":"base64","media_type":"application/pdf","data":<b64>}}
// Both are GA — no beta header required. VERIFIED: platform.claude.com docs, June 2026.

#[allow(clippy::too_many_arguments)]
pub(super) async fn call_anthropic(
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

    let endpoint = format!("{}/v1/messages", base_url);

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
        "model":       model,
        "system":      system,
        "messages":    [{ "role": "user", "content": user_content }],
        "max_tokens":  max_tokens,
        "temperature": temperature
    });

    let record = crate::provider::ProviderRegistry::global()
        .get("anthropic")
        .expect("anthropic always registered");
    let req = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post(&endpoint).header("Content-Type", "application/json"),
        api_key,
    ).json(&body);

    let (status, resp_json) = match send_and_parse(req).await {
        Ok(v)  => v,
        Err(e) => return e,
    };

    // Anthropic error: { "type": "error", "error": { "type": "...", "message": "..." } }
    if let Some(msg) = extract_provider_error(&resp_json, "Unknown Anthropic error") {
        return if status == 429 || status == 529 {
            NodeOutput::failure(NodeError::recoverable("RATE_LIMITED", msg))
        } else {
            NodeOutput::failure(NodeError::unrecoverable("API_ERROR", msg))
        };
    }

    let content = resp_json["content"]
        .as_array()
        .and_then(|arr| arr.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .unwrap_or("")
        .to_string();

    let model_used = resp_json["model"].as_str().unwrap_or(model).to_string();
    let input_tok  = resp_json["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let output_tok = resp_json["usage"]["output_tokens"].as_u64().unwrap_or(0);

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

/// `GET {base}/v1/models` — same `/v1` `call_anthropic` appends above, since
/// `resolve_base_url` strips it from `base_url` for this provider. Same
/// `{"data":[{"id":...}]}` shape as OpenAI's models list.
pub(super) async fn list_models(
    client: reqwest::Client,
    base_url: &str,
    api_key: &str,
) -> Result<Vec<String>, NodeError> {
    if api_key.is_empty() {
        return Err(NodeError::unrecoverable(
            "MISSING_API_KEY",
            "Anthropic requires an API key. Add one in Connections.",
        ));
    }

    let endpoint = format!("{}/v1/models", base_url);
    let record = crate::provider::ProviderRegistry::global()
        .get("anthropic")
        .expect("anthropic always registered");
    let req = crate::provider::ProviderRegistry::apply_auth(record, client.get(&endpoint), api_key);

    let (status, resp_json) = send_and_parse(req).await.map_err(|out| {
        out.error.unwrap_or_else(|| NodeError::unrecoverable("NETWORK_ERROR", "request failed"))
    })?;

    if let Some(msg) = extract_provider_error(&resp_json, "Unknown Anthropic error") {
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
mod list_models_tests {
    use super::*;

    async fn spawn_mock(response_body: &'static str) -> String {
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
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(), response_body
            );
            let _ = w.write_all(resp.as_bytes()).await;
        });
        format!("http://127.0.0.1:{}", port)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_models_parses_data_array_into_ids() {
        let base_url = spawn_mock(r#"{"data":[{"id":"claude-sonnet-5"},{"id":"claude-opus-5"}]}"#).await;
        let ids = list_models(reqwest::Client::new(), &base_url, "test-key")
            .await
            .expect("list_models should succeed against a canned data array");
        assert_eq!(ids, vec!["claude-sonnet-5".to_string(), "claude-opus-5".to_string()]);
    }
}
