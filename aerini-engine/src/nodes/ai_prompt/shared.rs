use serde_json::Value;

use crate::error::NodeError;
use crate::model::NodeOutput;

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

pub(super) async fn send_and_parse(req: reqwest::RequestBuilder) -> Result<(u16, Value), NodeOutput> {
    let response = match req.send().await {
        Ok(r)  => r,
        Err(e) => {
            let recoverable = e.is_timeout() || e.is_connect();
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
        Err(e)   => Err(NodeOutput::failure(NodeError::unrecoverable("PARSE_ERROR", e))),
    }
}
