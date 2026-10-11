use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

/// Discord caps message content at 2000 characters, not bytes.
const DISCORD_MAX_CONTENT_CHARS: usize = 2000;

fn content_too_long(content: &str) -> bool {
    content.chars().count() > DISCORD_MAX_CONTENT_CHARS
}

fn is_api_version(segment: &str) -> bool {
    segment.strip_prefix('v').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Parses a webhook URL and returns it only if, after the parser has
/// normalised dot segments, it is `/api/[vN/]webhooks/<id>/<token>` on
/// discord.com or discordapp.com with no credentials or port.
fn parse_webhook_url(raw: &str) -> Option<url::Url> {
    let url = url::Url::parse(raw.trim()).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("discord.com" | "discordapp.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let segments: Vec<&str> = url.path_segments()?.collect();
    let tail = match segments.as_slice() {
        ["api", "webhooks", tail @ ..] => tail,
        ["api", version, "webhooks", tail @ ..] if is_api_version(version) => tail,
        _ => return None,
    };
    match tail {
        [id, token]
            if !id.is_empty()
                && id.bytes().all(|b| b.is_ascii_digit())
                && !token.is_empty()
                && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') =>
        {
            Some(url)
        }
        _ => None,
    }
}

/// Pings for `@everyone` and `@here` are suppressed unless `allow_everyone`
/// is set, so upstream data cannot notify a whole server by default; user and
/// role mentions still ping.
fn message_body(content: &str, username: Option<&str>, allow_everyone: bool) -> Value {
    let parse: &[&str] = if allow_everyone { &["users", "roles", "everyone"] } else { &["users", "roles"] };
    let mut body = json!({
        "content": content,
        "allowed_mentions": { "parse": parse }
    });
    if let Some(name) = username {
        body["username"] = Value::String(name.to_string());
    }
    body
}

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
                "username":    { "type": "string", "description": "Override the webhook's display name (optional)" },
                "allow_everyone_mentions": { "type": "boolean", "description": "Let @everyone and @here in the content ping the channel (default false). The webhook's channel permissions can still block it." }
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
        let raw_webhook_url = match input.input["webhook_url"].as_str() {
            Some(u) if !u.trim().is_empty() => u,
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_WEBHOOK_URL", "webhook_url field is required")),
        };

        let webhook_url = match parse_webhook_url(raw_webhook_url) {
            Some(u) => u,
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_WEBHOOK_URL",
                "webhook_url must be a Discord webhook URL (https://discord.com/api/webhooks/<id>/<token>)",
            )),
        };

        let content = match input.input["content"].as_str() {
            Some(c) if !c.trim().is_empty() => c.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_CONTENT", "content field is required")),
        };

        if content_too_long(&content) {
            return NodeOutput::failure(NodeError::unrecoverable(
                "CONTENT_TOO_LONG",
                "content exceeds Discord's 2000-character limit",
            ));
        }

        let allow_everyone = match super::util::cfg_bool_opt(&input.input["allow_everyone_mentions"], "allow_everyone_mentions") {
            Ok(v) => v.unwrap_or(false),
            Err(e) => return NodeOutput::failure(e),
        };

        let body = message_body(&content, input.input["username"].as_str().filter(|s| !s.is_empty()), allow_everyone);

        match super::shared_http_client().post(webhook_url.as_str()).json(&body).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                // Discord returns 204 No Content on success.
                if status == 204 || status == 200 {
                    NodeOutput::success_with_logs(
                        json!({ "sent": true }),
                        vec!["Discord message sent via webhook".to_string()],
                    )
                } else {
                    let body_text = super::util::read_text_capped(resp, super::util::MAX_ERROR_BODY_BYTES).await;
                    NodeOutput::failure(super::util::provider_error(
                        status,
                        "DISCORD_ERROR",
                        format!("HTTP {}: {}", status, body_text),
                    ))
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
    use super::{content_too_long, message_body, parse_webhook_url};

    #[test]
    fn limit_is_inclusive_at_2000_chars() {
        assert!(!content_too_long(&"a".repeat(2000)));
        assert!(content_too_long(&"a".repeat(2001)));
    }

    #[test]
    fn multibyte_content_is_counted_in_chars_not_bytes() {
        assert!(!content_too_long(&"é".repeat(1500)));
        assert!(content_too_long(&"é".repeat(2001)));
    }

    #[test]
    fn webhook_url_accepts_the_documented_shapes() {
        for raw in [
            "https://discord.com/api/webhooks/123456/abc-DEF_9",
            "https://discordapp.com/api/webhooks/123456/abc",
            "https://discord.com/api/v10/webhooks/123456/abc?wait=true",
            "  https://discord.com/api/webhooks/123456/abc\n",
        ] {
            assert!(parse_webhook_url(raw).is_some(), "{raw:?}");
        }
    }

    #[test]
    fn webhook_url_refuses_other_hosts_shapes_and_escapes() {
        for raw in [
            "http://discord.com/api/webhooks/1/a",
            "https://discord.com.evil.example/api/webhooks/1/a",
            "https://discord.com@evil.example/api/webhooks/1/a",
            "https://user:pw@discord.com/api/webhooks/1/a",
            "https://discord.com:8443/api/webhooks/1/a",
            "https://discord.com/api/webhooks/../../x/1/a",
            "https://discord.com/api/webhooks/%2e%2e/x/1/a",
            "https://discord.com/api/webhooks/1/a/slack",
            "https://discord.com/api/webhooks/1",
            "https://discord.com/api/webhooks/abc/a",
            "https://discord.com/api/webhooks/1/a%2Fb",
            "https://discord.com/api/vx/webhooks/1/a",
        ] {
            assert!(parse_webhook_url(raw).is_none(), "{raw:?}");
        }
    }

    #[test]
    fn message_never_pings_everyone_but_still_pings_users_and_roles() {
        let body = message_body("hi @everyone", Some("Bot"), false);
        let parse: Vec<&str> = body["allowed_mentions"]["parse"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(parse, ["users", "roles"]);
        assert_eq!(body["username"], "Bot");
        assert!(message_body("hi", None, false).get("username").is_none());
    }

    #[test]
    fn everyone_is_only_allowed_when_the_field_is_set() {
        let body = message_body("hi", None, true);
        assert_eq!(body["allowed_mentions"]["parse"], serde_json::json!(["users", "roles", "everyone"]));
    }
}
