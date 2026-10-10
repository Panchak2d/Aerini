use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

/// Slack truncates `text` beyond 40,000 characters, so a longer message is
/// refused instead of being posted cut short.
const SLACK_MAX_TEXT_CHARS: usize = 40_000;
const MAX_DETAIL_CHARS: usize = 500;

fn text_too_long(text: &str) -> bool {
    text.chars().count() > SLACK_MAX_TEXT_CHARS
}

/// The error code Slack returned, plus the scopes it names for
/// `missing_scope` (`needed` and `provided`), each capped in length.
fn slack_error_message(v: &Value) -> String {
    let code = v["error"].as_str().unwrap_or("unknown_error");
    let details: Vec<String> = ["needed", "provided"]
        .iter()
        .filter_map(|key| {
            let value = v[*key].as_str().filter(|s| !s.is_empty())?;
            Some(format!("{key}: {}", value.chars().take(MAX_DETAIL_CHARS).collect::<String>()))
        })
        .collect();
    if details.is_empty() {
        code.to_string()
    } else {
        format!("{code} ({})", details.join("; "))
    }
}

pub struct SlackNode;

#[async_trait]
impl Node for SlackNode {
    fn type_id(&self) -> &'static str { "slack" }
    fn display_name(&self) -> &'static str { "Slack" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Post a message to a Slack channel via the Slack API (chat.postMessage) using a Bot Token." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["channel", "text"],
            "properties": {
                "channel": { "type": "string", "description": "Channel ID or name (e.g. C1234567890 or #general)" },
                "text":    { "type": "string", "description": "Message text" },
                "api_key": { "type": "string", "description": "Slack Bot Token (xoxb-...)" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "ok":      { "type": "boolean" },
                "ts":      { "type": "string", "description": "Message timestamp" },
                "channel": { "type": "string" }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let channel = match input.input["channel"].as_str() {
            Some(c) if !c.trim().is_empty() => c.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_CHANNEL", "channel field is required")),
        };

        let text = match input.input["text"].as_str() {
            Some(t) if !t.trim().is_empty() => t.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_TEXT", "text field is required")),
        };

        if text_too_long(&text) {
            return NodeOutput::failure(NodeError::unrecoverable(
                "TEXT_TOO_LONG",
                "text exceeds Slack's 40,000-character limit",
            ));
        }

        let api_key = match input.input["api_key"].as_str().filter(|s| !s.is_empty()) {
            Some(k) => k.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_TOKEN", "Slack Bot Token is required — add it via the credential store")),
        };

        let body = json!({ "channel": channel, "text": text });

        match super::shared_http_client()
            .post("https://slack.com/api/chat.postMessage")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                match super::util::read_json_response_capped(resp).await {
                    Ok(v) => {
                        // Slack returns HTTP 200 even on error — check the ok field.
                        if v["ok"].as_bool() == Some(true) {
                            NodeOutput::success_with_logs(
                                v,
                                vec![format!("Slack message sent to {}", channel)],
                            )
                        } else {
                            NodeOutput::failure(super::util::provider_error(status, "SLACK_ERROR", slack_error_message(&v)))
                        }
                    }
                    Err(e) => NodeOutput::failure(super::util::provider_error(
                        status,
                        "PARSE_ERROR",
                        format!("HTTP {}: could not read Slack response: {}", status, e),
                    )),
                }
            }
            Err(e) => {
                super::util::http_err_output(super::util::Replay::Never, &e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_limit_is_inclusive_at_40000_chars() {
        assert!(!text_too_long(&"é".repeat(40_000)));
        assert!(text_too_long(&"é".repeat(40_001)));
    }

    #[test]
    fn missing_scope_error_names_the_scopes() {
        let v = json!({ "ok": false, "error": "missing_scope", "needed": "chat:write", "provided": "channels:read" });
        assert_eq!(slack_error_message(&v), "missing_scope (needed: chat:write; provided: channels:read)");
    }

    #[test]
    fn plain_error_stays_the_bare_code() {
        assert_eq!(slack_error_message(&json!({ "ok": false, "error": "channel_not_found" })), "channel_not_found");
        assert_eq!(slack_error_message(&json!({ "ok": false })), "unknown_error");
    }
}
