use serde_json::Value;

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::util::{Replay, provider_error_for};

pub(super) fn extract_err_msg(obj: &serde_json::Map<String, Value>, default: &str) -> String {
    obj.get("message").and_then(|m| m.as_str()).unwrap_or(default).to_string()
}

/// Extracts a provider error message from a chat-completions-style response.
/// Most OpenAI-compatible servers nest it as `{"error": {"message": ...}}`,
/// but not all: Ollama's own API docs show a bare string, `{"error": "..."}`,
/// for its own error responses. Checking only the object shape misreads a
/// string-shaped error as "no error field", which falls through to the
/// success path with empty output instead of surfacing the failure.
/// `pub(crate)` (not `pub(super)` like the rest of this module) so
/// `ai_agent.rs`'s three provider call sites can share it too, the same way
/// `ai_prompt::attachments` is already re-exported for cross-node reuse.
pub(crate) fn extract_provider_error(json: &Value, default: &str) -> Option<String> {
    match &json["error"] {
        Value::Null        => None,
        Value::String(s)   => Some(s.clone()),
        Value::Object(obj) => Some(extract_err_msg(obj, default)),
        _                  => Some(default.to_string()),
    }
}

/// Turns a provider response that reports an error into a failed node output,
/// or `None` for a good one. A 4xx/5xx status with no `error` field is a
/// failure too, not an empty success. Chat completions create nothing, so a
/// gateway failure is retry-eligible ([`Replay::Safe`]).
pub(super) fn error_output(status: u16, json: &Value, default: &str) -> Option<NodeOutput> {
    let msg = extract_provider_error(json, default)
        .or_else(|| (status >= 400).then(|| format!("{default} (HTTP {status})")))?;
    Some(NodeOutput::failure(provider_error_for(Replay::Safe, status, "API_ERROR", msg)))
}

/// A request parameter the provider refused for this model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RejectedParam {
    Temperature,
    TokenLimit,
}

/// Which parameter a 400/422 reply names, so the caller can resend once without
/// `temperature` or with the other token-limit key. Providers differ per model:
/// OpenAI reasoning models take only the default temperature and reject
/// `max_tokens` in favour of `max_completion_tokens`, and Claude Opus 4.7 and
/// later reject `temperature`. Matching the reply, not the model name, keeps new
/// model names working. Only `temperature` and the token limit are ever dropped
/// or swapped; any other rejection stays a failure.
pub(crate) fn rejected_param(status: u16, json: &Value) -> Option<RejectedParam> {
    if status != 400 && status != 422 {
        return None;
    }
    let msg = extract_provider_error(json, "").unwrap_or_default().to_ascii_lowercase();
    if msg.contains("temperature") {
        Some(RejectedParam::Temperature)
    } else if msg.contains("max_tokens") || msg.contains("max_completion_tokens") {
        Some(RejectedParam::TokenLimit)
    } else {
        None
    }
}

/// True for the hosted OpenAI API, the only endpoint known to want
/// `max_completion_tokens` first. Other OpenAI-compatible servers get
/// `max_tokens`, which they all accept; the swap above covers the rest.
pub(crate) fn is_official_openai(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.eq_ignore_ascii_case("api.openai.com")))
        .unwrap_or(false)
}

/// The configured model name, or `default` when the field is absent, blank or not text.
pub(crate) fn model_or_default(value: &Value, default: &str) -> String {
    value.as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(default).to_string()
}

/// The text of a Claude reply: every `text` block in order. A reply can carry
/// several (for example around citations), and every one belongs in the answer.
pub(crate) fn anthropic_text(resp: &Value) -> String {
    resp["content"]
        .as_array()
        .map(|blocks| {
            blocks.iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// Claude's `temperature` range is 0.0 to 1.0; the node advertises 0.0 to 2.0.
pub(crate) fn clamp_claude_temperature(t: f64) -> f64 {
    t.clamp(0.0, 1.0)
}

pub(super) async fn send_and_parse(req: reqwest::RequestBuilder) -> Result<(u16, Value), NodeOutput> {
    let response = match req.send().await {
        Ok(r)  => r,
        Err(e) => {
            let recoverable = crate::nodes::util::is_retryable_network_error(Replay::Safe, &e);
            return Err(if recoverable {
                NodeOutput::failure(NodeError::recoverable("NETWORK_ERROR", crate::nodes::util::reqwest_err_msg(&e)))
            } else {
                NodeOutput::failure(NodeError::unrecoverable("NETWORK_ERROR", crate::nodes::util::reqwest_err_msg(&e)))
            });
        }
    };

    let status = response.status().as_u16();

    // Capped read: a provider response of unbounded size (misconfigured
    // base_url, or a malicious local a1111/comfyui/Ollama-compatible endpoint)
    // can no longer be buffered without limit before parsing.
    match crate::nodes::util::read_json_response_capped(response).await {
        Ok(json) => Ok((status, json)),
        Err(e)   => Err(NodeOutput::failure(provider_error_for(Replay::Safe, status, "PARSE_ERROR", e))),
    }
}

/// Serves `replies` one per connection, in order, and records each request body.
#[cfg(test)]
pub(crate) async fn spawn_sequence_mock(
    replies: Vec<(u16, &'static str)>,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("mock bind failed");
    let port = listener.local_addr().expect("local_addr failed").port();
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = bodies.clone();
    tokio::spawn(async move {
        for (status, reply) in replies {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let (r, mut w) = stream.split();
            let mut reader = BufReader::new(r);
            let mut line = String::new();
            let mut content_length = 0usize;
            loop {
                line.clear();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 { break; }
                if line.trim().is_empty() { break; }
                if let Some((k, v)) = line.trim().split_once(':') {
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0u8; content_length];
            let _ = reader.read_exact(&mut body).await;
            seen.lock().expect("mock lock").push(String::from_utf8_lossy(&body).to_string());
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                reply.len(), reply
            );
            let _ = w.write_all(resp.as_bytes()).await;
        }
    });
    (format!("http://127.0.0.1:{port}"), bodies)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parts(out: NodeOutput) -> (bool, String) {
        let e = out.error.expect("expected a NodeError");
        (e.recoverable, e.code)
    }

    #[test]
    fn error_output_classifies_by_status() {
        let body = json!({ "error": { "message": "m" } });
        assert_eq!(parts(error_output(429, &body, "d").unwrap()), (true, "RATE_LIMITED".into()));
        assert_eq!(parts(error_output(503, &body, "d").unwrap()), (true, "UPSTREAM_UNAVAILABLE".into()));
        assert_eq!(parts(error_output(500, &body, "d").unwrap()), (false, "API_ERROR".into()));
        assert_eq!(parts(error_output(401, &body, "d").unwrap()), (false, "API_ERROR".into()));
    }

    #[test]
    fn error_status_without_an_error_field_is_still_a_failure() {
        let body = json!({ "message": "gateway" });
        assert_eq!(parts(error_output(502, &body, "d").unwrap()), (true, "UPSTREAM_UNAVAILABLE".into()));
        assert_eq!(parts(error_output(400, &body, "d").unwrap()), (false, "API_ERROR".into()));
    }

    #[test]
    fn rejected_param_names_only_the_parameters_it_can_drop() {
        let temp = json!({ "error": { "message": "Unsupported value: 'temperature' does not support 0.7 with this model." } });
        let tokens = json!({ "error": { "message": "Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead." } });
        let other = json!({ "error": { "message": "Invalid model" } });
        assert_eq!(rejected_param(400, &temp), Some(RejectedParam::Temperature));
        assert_eq!(rejected_param(400, &tokens), Some(RejectedParam::TokenLimit));
        assert_eq!(rejected_param(400, &other), None);
        assert_eq!(rejected_param(500, &temp), None);
        assert_eq!(rejected_param(401, &temp), None);
    }

    #[test]
    fn only_the_hosted_openai_host_counts_as_official() {
        assert!(is_official_openai("https://api.openai.com/v1"));
        assert!(!is_official_openai("http://127.0.0.1:11434/v1"));
        assert!(!is_official_openai("https://api.openai.com.example.net/v1"));
        assert!(!is_official_openai("not a url"));
    }

    #[test]
    fn blank_or_non_text_model_falls_back_to_the_default() {
        assert_eq!(model_or_default(&json!("  "), "d"), "d");
        assert_eq!(model_or_default(&Value::Null, "d"), "d");
        assert_eq!(model_or_default(&json!(" m1 "), "d"), "m1");
    }

    #[test]
    fn claude_reply_text_joins_every_text_block() {
        let resp = json!({ "content": [
            { "type": "text", "text": "a" },
            { "type": "tool_use", "name": "x" },
            { "type": "text", "text": "b" }
        ] });
        assert_eq!(anthropic_text(&resp), "ab");
        assert_eq!(anthropic_text(&json!({})), "");
    }

    #[test]
    fn claude_temperature_is_clamped_to_its_range() {
        assert_eq!(clamp_claude_temperature(1.7), 1.0);
        assert_eq!(clamp_claude_temperature(-0.5), 0.0);
        assert_eq!(clamp_claude_temperature(0.4), 0.4);
    }

    #[test]
    fn good_response_is_not_an_error() {
        assert!(error_output(200, &json!({ "choices": [] }), "d").is_none());
    }
}
