use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn client() -> reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build Slack HTTP client")
    }).clone()
}

pub struct SlackNode;

#[async_trait]
impl Node for SlackNode {
    fn type_id(&self) -> &'static str { "slack" }
    fn display_name(&self) -> &'static str { "Slack" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }

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

        let api_key = match input.input["api_key"].as_str().filter(|s| !s.is_empty()) {
            Some(k) => k.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_TOKEN", "Slack Bot Token is required — add it via the credential store")),
        };

        let body = json!({ "channel": channel, "text": text });

        match client()
            .post("https://slack.com/api/chat.postMessage")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                match resp.json::<Value>().await {
                    Ok(v) => {
                        // Slack returns HTTP 200 even on error — check the ok field.
                        if v["ok"].as_bool() == Some(true) {
                            NodeOutput::success_with_logs(
                                v,
                                vec![format!("Slack message sent to {}", channel)],
                            )
                        } else {
                            let err = v["error"].as_str().unwrap_or("unknown_error").to_string();
                            NodeOutput::failure(NodeError::unrecoverable("SLACK_ERROR", err))
                        }
                    }
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable(
                        "PARSE_ERROR",
                        format!("HTTP {}: could not parse Slack response: {}", status, e),
                    )),
                }
            }
            Err(e) => {
                super::util::http_err_output(&e)
            }
        }
    }
}
