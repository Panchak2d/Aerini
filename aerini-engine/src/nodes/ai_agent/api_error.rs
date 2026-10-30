use std::time::Duration;

use serde_json::Value;

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::ai_prompt::{
    extract_provider_error, is_request_timeout, timeout_error,
};
use crate::nodes::util::{is_retryable_network_error, reqwest_err_msg, Replay};

// ── API error classification ──────────────────────────────────────────
//
// `call_gemini_agent`/`call_openai_agent`/`call_anthropic_agent` duplicate
// ai_prompt/shared.rs's send_and_parse request/response shape (send, cap-read,
// parse) and carry the same recoverable/unrecoverable distinction ai_prompt
// has: a connection-failed send, an HTTP 429 or 529 (rate limited,
// overloaded) and a 502/503/504 gateway failure are Recoverable, including
// when the body is not JSON (a proxy's HTML error page); every other failure
// (auth, bad request, oversized body, a non-JSON body on any other status)
// stays Unrecoverable; a 400 saying the model does not support tools is the
// one bad request with its own code, `TOOLS_NOT_SUPPORTED`. A model call creates nothing, so repeating it after a
// gateway failure can at most bill twice. Every call site below must go
// through `agent_status_error` rather than wrapping every error in
// `NodeError::unrecoverable` regardless of cause: a rate-limited Agent node
// must stay retry-eligible like the otherwise-identical Prompt node hitting
// the same API. A request that outlives the node's `timeout_secs` is the
// exception among send failures: it is `TimedOut`, never retried.
pub(super) enum AgentApiError {
    Recoverable(String),
    Unavailable(String),
    Unrecoverable(String),
    ToolsNotSupported(String),
    TimedOut(Duration),
}

impl AgentApiError {
    pub(super) fn into_node_output(self) -> NodeOutput {
        match self {
            AgentApiError::Recoverable(msg) => {
                NodeOutput::failure(NodeError::recoverable("RATE_LIMITED", msg))
            }
            AgentApiError::Unavailable(msg) => {
                NodeOutput::failure(NodeError::recoverable("UPSTREAM_UNAVAILABLE", msg))
            }
            AgentApiError::Unrecoverable(msg) => {
                NodeOutput::failure(NodeError::unrecoverable("API_ERROR", msg))
            }
            AgentApiError::ToolsNotSupported(msg) => {
                NodeOutput::failure(NodeError::unrecoverable("TOOLS_NOT_SUPPORTED", msg))
            }
            AgentApiError::TimedOut(timeout) => NodeOutput::failure(timeout_error(timeout)),
        }
    }
}

pub(super) fn agent_status_error(status: u16, msg: String) -> AgentApiError {
    match status {
        429 | 529 => AgentApiError::Recoverable(msg),
        502..=504 => AgentApiError::Unavailable(msg),
        _ => AgentApiError::Unrecoverable(msg),
    }
}

/// The failure message for a provider response, or `None` for a good one. A
/// 4xx/5xx status with no `error` field is a failure, not an empty success.
pub(super) fn agent_response_error(status: u16, json: &Value, default: &str) -> Option<String> {
    extract_provider_error(json, default).or_else(|| (status >= 400).then(|| format!("{default} (HTTP {status})")))
}

pub(super) fn send_error(e: reqwest::Error, timeout: Duration) -> AgentApiError {
    if is_request_timeout(&e) {
        return AgentApiError::TimedOut(timeout);
    }
    let msg = reqwest_err_msg(&e);
    if is_retryable_network_error(Replay::Safe, &e) {
        AgentApiError::Recoverable(msg)
    } else {
        AgentApiError::Unrecoverable(msg)
    }
}
