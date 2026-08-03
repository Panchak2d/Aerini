use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

pub struct TelegramNode;

#[async_trait]
impl Node for TelegramNode {
    fn type_id(&self) -> &'static str { "telegram" }
    fn display_name(&self) -> &'static str { "Telegram" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Send a text message to a Telegram chat or channel via a bot token." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["chat_id", "text"],
            "properties": {
                "chat_id":    { "type": "string", "description": "Telegram chat ID or username (e.g. 123456789 or @channelname)" },
                "text":       { "type": "string", "description": "Message text" },
                "parse_mode": { "type": "string", "enum": ["", "Markdown", "HTML"], "description": "Optional text formatting mode" },
                "api_key":    { "type": "string", "description": "Telegram Bot Token (from @BotFather)" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message_id": { "type": "number" },
                "chat":       { "type": "object" },
                "text":       { "type": "string" },
                "date":       { "type": "number" }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        // Telegram Bot Token goes in the URL, not a header.
        let bot_token = match input.input["api_key"].as_str().filter(|s| !s.is_empty()) {
            Some(t) => t.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_TOKEN",
                "Telegram Bot Token is required — add it via the credential store as 'api_key'",
            )),
        };

        let chat_id = match input.input["chat_id"].as_str().filter(|s| !s.is_empty()) {
            Some(id) => id.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_CHAT_ID", "chat_id is required")),
        };

        let text = match input.input["text"].as_str().filter(|s| !s.is_empty()) {
            Some(t) => t.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_TEXT", "text is required")),
        };

        let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);

        let mut body = json!({ "chat_id": chat_id, "text": text });
        if let Some(mode) = input.input["parse_mode"].as_str().filter(|s| !s.is_empty()) {
            body["parse_mode"] = Value::String(mode.to_string());
        }

        match super::shared_http_client().post(&url).json(&body).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                match resp.json::<Value>().await {
                    Ok(v) => {
                        if v["ok"].as_bool() == Some(true) {
                            let result = v["result"].clone();
                            let msg_id = result["message_id"].as_u64().unwrap_or(0);
                            NodeOutput::success_with_logs(
                                result,
                                vec![format!("Telegram message {} sent to {}", msg_id, chat_id)],
                            )
                        } else {
                            let description = v["description"].as_str().unwrap_or("unknown error").to_string();
                            NodeOutput::failure(NodeError::unrecoverable("TELEGRAM_ERROR", description))
                        }
                    }
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable(
                        "PARSE_ERROR",
                        format!("HTTP {}: {}", status, e),
                    )),
                }
            }
            Err(e) => {
                super::util::http_err_output(&e)
            }
        }
    }
}

