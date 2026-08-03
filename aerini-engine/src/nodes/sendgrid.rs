use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

pub struct SendGridNode;

#[async_trait]
impl Node for SendGridNode {
    fn type_id(&self) -> &'static str { "sendgrid" }
    fn display_name(&self) -> &'static str { "SendGrid" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Send a plain-text transactional email via the SendGrid API." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["to_email", "from_email", "subject", "body"],
            "properties": {
                "to_email":   { "type": "string", "description": "Recipient email address" },
                "from_email": { "type": "string", "description": "Sender email address (must be verified in SendGrid)" },
                "subject":    { "type": "string", "description": "Email subject line" },
                "body":       { "type": "string", "description": "Email body (plain text)" },
                "api_key":    { "type": "string", "description": "SendGrid API key (SG....)" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "sent":       { "type": "boolean" },
                "message_id": { "type": "string" }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let api_key = match input.input["api_key"].as_str().filter(|s| !s.is_empty()) {
            Some(k) => k.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_API_KEY",
                "SendGrid API key is required — add it via the credential store",
            )),
        };

        let to_email = match input.input["to_email"].as_str().filter(|s| !s.is_empty()) {
            Some(e) => e.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_TO_EMAIL", "to_email is required")),
        };

        let from_email = match input.input["from_email"].as_str().filter(|s| !s.is_empty()) {
            Some(e) => e.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_FROM_EMAIL", "from_email is required")),
        };

        let subject = match input.input["subject"].as_str().filter(|s| !s.is_empty()) {
            Some(s) => s.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_SUBJECT", "subject is required")),
        };

        let body_text = match input.input["body"].as_str() {
            Some(b) => b.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_BODY", "body is required")),
        };

        let payload = json!({
            "personalizations": [{ "to": [{ "email": to_email }] }],
            "from":    { "email": from_email },
            "subject": subject,
            "content": [{ "type": "text/plain", "value": body_text }]
        });

        match super::shared_http_client()
            .post("https://api.sendgrid.com/v3/mail/send")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&payload)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                // SendGrid returns 202 Accepted on success with an empty body.
                if status == 202 {
                    let message_id = resp
                        .headers()
                        .get("x-message-id")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    NodeOutput::success_with_logs(
                        json!({ "sent": true, "message_id": message_id }),
                        vec![format!("Email sent to {} via SendGrid", to_email)],
                    )
                } else {
                    let body_text = resp.text().await.unwrap_or_default();
                    NodeOutput::failure(NodeError::unrecoverable(
                        "SENDGRID_ERROR",
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

