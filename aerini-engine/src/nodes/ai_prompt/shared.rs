use serde_json::Value;

use crate::error::NodeError;
use crate::model::NodeOutput;

pub(super) fn extract_err_msg(obj: &serde_json::Map<String, Value>, default: &str) -> String {
    obj.get("message").and_then(|m| m.as_str()).unwrap_or(default).to_string()
}

pub(super) async fn send_and_parse(req: reqwest::RequestBuilder) -> Result<(u16, Value), NodeOutput> {
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

    // Capped read (S2-6): a provider response of unbounded size (misconfigured
    // base_url, or a malicious local a1111/comfyui/Ollama-compatible endpoint)
    // can no longer be buffered without limit before parsing.
    match crate::nodes::util::read_json_response_capped(response).await {
        Ok(json) => Ok((status, json)),
        Err(e)   => Err(NodeOutput::failure(NodeError::unrecoverable("PARSE_ERROR", e))),
    }
}
