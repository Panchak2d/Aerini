use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

pub struct EmailNode;

#[async_trait]
impl Node for EmailNode {
    fn type_id(&self) -> &'static str { "email_send" }
    fn display_name(&self) -> &'static str { "Send Email" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "2.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["smtp_host", "from", "to", "subject", "body"],
            "properties": {
                "smtp_host": { "type": "string", "description": "SMTP server e.g. smtp.gmail.com" },
                "smtp_port": { "type": "number", "description": "587 (STARTTLS, recommended) or 465 (SSL)", "default": 587 },
                "from":      { "type": "string", "description": "Sender email address" },
                "to":        { "type": "string", "description": "Recipient(s), comma-separated" },
                "subject":   { "type": "string" },
                "body":      { "type": "string" },
                "html":      { "type": "boolean", "description": "Send as HTML email (default: false)", "default": false },
                "username":  { "type": "string", "description": "SMTP username (usually your email address)" },
                "password":  { "type": "string" }
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
        match send_email(&input.input).await {
            Ok(log) => NodeOutput::success_with_logs(json!({ "sent": true }), vec![log]),
            Err(e)  => NodeOutput::failure(e),
        }
    }
}

async fn send_email(cfg: &Value) -> Result<String, NodeError> {
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;

    let smtp_host = cfg["smtp_host"].as_str().filter(|s| !s.is_empty())
        .ok_or_else(|| NodeError::unrecoverable("MISSING_SMTP_HOST", "smtp_host is required"))?
        .to_string();
    let smtp_port = cfg["smtp_port"].as_u64().unwrap_or(587) as u16;

    let from_str = cfg["from"].as_str().filter(|s| !s.is_empty())
        .ok_or_else(|| NodeError::unrecoverable("MISSING_FROM", "from address is required"))?;
    let to_str = cfg["to"].as_str().filter(|s| !s.is_empty())
        .ok_or_else(|| NodeError::unrecoverable("MISSING_TO", "to address is required"))?
        .to_string();

    let subject = cfg["subject"].as_str().unwrap_or("(no subject)");
    let body    = cfg["body"].as_str().unwrap_or("").to_string();
    let html    = cfg["html"].as_bool().unwrap_or(false);

    let from_mbox = from_str.parse::<lettre::message::Mailbox>()
        .map_err(|e| NodeError::unrecoverable("INVALID_FROM", &format!("Invalid from address: {e}")))?;

    let content_type = if html { ContentType::TEXT_HTML } else { ContentType::TEXT_PLAIN };

    let mut msg_builder = Message::builder().from(from_mbox).subject(subject);
    for addr in to_str.split(',') {
        let addr = addr.trim();
        if addr.is_empty() { continue; }
        let mbox = addr.parse::<lettre::message::Mailbox>()
            .map_err(|e| NodeError::unrecoverable("INVALID_TO", &format!("Invalid to address '{addr}': {e}")))?;
        msg_builder = msg_builder.to(mbox);
    }
    let message = msg_builder
        .header(content_type)
        .body(body)
        .map_err(|e| NodeError::unrecoverable("BUILD_ERROR", &format!("Failed to build email: {e}")))?;

    let username = cfg["username"].as_str().unwrap_or("").to_string();
    let password = cfg["password"].as_str().unwrap_or("").to_string();

    let mut builder = if smtp_port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp_host)
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp_host)
    }.map_err(|e| NodeError::unrecoverable("TRANSPORT_ERROR", &format!("SMTP transport init failed: {e}")))?;

    builder = builder.port(smtp_port);

    if !username.is_empty() && !password.is_empty() {
        builder = builder.credentials(Credentials::new(username, password));
    }

    let transport = builder.build();

    transport.send(message).await
        .map_err(|e| NodeError::recoverable("SMTP_ERROR", &format!("SMTP send failed: {e}")))?;

    Ok(format!("Email sent to {to_str} via {smtp_host}:{smtp_port}"))
}
