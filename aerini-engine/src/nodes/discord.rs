use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

pub struct DiscordNode;

#[async_trait]
impl Node for DiscordNode {
    fn type_id(&self) -> &'static str { "discord" }
    fn display_name(&self) -> &'static str { "Discord" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Send a text message to a Discord channel via a webhook URL." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["webhook_url", "content"],
            "properties": {
                "webhook_url": { "type": "string", "description": "Discord webhook URL (https://discord.com/api/webhooks/...)" },
                "content":     { "type": "string", "description": "Message content (max 2000 characters)" },
                "username":    { "type": "string", "description": "Override the webhook's display name (optional)" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "sent": { "type": "boolean" }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let webhook_url = match input.input["webhook_url"].as_str() {
            Some(u) if !u.trim().is_empty() => u.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_WEBHOOK_URL", "webhook_url field is required")),
        };

        if !webhook_url.starts_with("https://discord.com/api/webhooks/")
            && !webhook_url.starts_with("https://discordapp.com/api/webhooks/")
        {
            return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_WEBHOOK_URL",
                "webhook_url must be a Discord webhook URL (https://discord.com/api/webhooks/...)",
            ));
        }

        let content = match input.input["content"].as_str() {
            Some(c) if !c.trim().is_empty() => c.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_CONTENT", "content field is required")),
        };

        // Enforce Discord's 2000-character message limit.
        if content.len() > 2000 {
            return NodeOutput::failure(NodeError::unrecoverable(
                "CONTENT_TOO_LONG",
                "content exceeds Discord's 2000-character limit",
            ));
        }

        let mut body = json!({ "content": content });
        if let Some(username) = input.input["username"].as_str().filter(|s| !s.is_empty()) {
            body["username"] = Value::String(username.to_string());
        }

        match super::shared_http_client().post(&webhook_url).json(&body).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                // Discord returns 204 No Content on success.
                if status == 204 || status == 200 {
                    NodeOutput::success_with_logs(
                        json!({ "sent": true }),
                        vec!["Discord message sent via webhook".to_string()],
                    )
                } else {
                    let body_text = super::util::read_error_snippet(resp).await;
                    NodeOutput::failure(super::util::http_status_error(
                        "DISCORD_ERROR",
                        status,
                        format!("HTTP {}: {}", status, body_text),
                    ))
                }
            }
            Err(e) => {
                super::util::http_err_output(&e)
            }
        }
    }
}

