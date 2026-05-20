use async_trait::async_trait;
use lettre::{
    message::{header::ContentType, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

pub struct EmailNode;

#[async_trait]
impl Node for EmailNode {
    fn type_id(&self)      -> &'static str { "email_send" }
    fn display_name(&self) -> &'static str { "Send Email" }
    fn node_type(&self)    -> NodeType     { NodeType::Action }
    fn version(&self)      -> &'static str { "2.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["to", "subject", "body", "smtp_host"],
            "properties": {
                "smtp_host": { "type": "string", "description": "SMTP server, e.g. smtp.gmail.com" },
                "smtp_port": { "type": "number", "description": "Port: 587 (STARTTLS, default) or 465 (SSL)" },
                "from":      { "type": "string", "description": "Sender email address" },
                "to":        { "type": "string", "description": "Recipient(s), comma-separated" },
                "subject":   { "type": "string" },
                "body":      { "type": "string" },
                "html":      { "type": "boolean", "description": "Send as HTML (default: false)" },
                "username":  { "type": "string", "description": "SMTP username (usually your email)" },
                "password":  { "type": "string", "description": "SMTP password — use a Credential" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "properties": {
            "sent":    { "type": "boolean" },
            "to":      { "type": "string" },
            "subject": { "type": "string" }
        }})
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let smtp_host = match input.input["smtp_host"].as_str() {
            Some(h) if !h.is_empty() => h.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_HOST",
                "smtp_host is required. Example: smtp.gmail.com")),
        };
        let to = match input.input["to"].as_str() {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_TO",
                "to address is required")),
        };

        let from     = input.input["from"].as_str().unwrap_or("").to_string();
        let subject  = input.input["subject"].as_str().unwrap_or("(no subject)").to_string();
        let body     = input.input["body"].as_str().unwrap_or("").to_string();
        let port     = input.input["smtp_port"].as_u64().unwrap_or(587) as u16;
        let username = input.input["username"].as_str().unwrap_or("").to_string();
        let password = input.input["password"].as_str().unwrap_or("").to_string();
        let is_html  = input.input["html"].as_bool().unwrap_or(false);

        let sender_str = if from.is_empty() { username.clone() } else { from.clone() };
        if sender_str.is_empty() {
            return NodeOutput::failure(NodeError::unrecoverable("MISSING_FROM",
                "from address is required"));
        }

        // Build transport first — no ? operator, use match instead
        let transport: AsyncSmtpTransport<Tokio1Executor> = if port == 465 {
            match AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp_host) {
                Err(e) => return NodeOutput::failure(NodeError::unrecoverable("TRANSPORT_ERROR",
                    format!("Could not create SMTP transport: {}", e))),
                Ok(mut b) => {
                    if !username.is_empty() && !password.is_empty() {
                        b = b.credentials(Credentials::new(username.clone(), password.clone()));
                    }
                    b.port(port).build()
                }
            }
        } else {
            match AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp_host) {
                Err(e) => return NodeOutput::failure(NodeError::unrecoverable("TRANSPORT_ERROR",
                    format!("Could not create SMTP transport: {}", e))),
                Ok(mut b) => {
                    if !username.is_empty() && !password.is_empty() {
                        b = b.credentials(Credentials::new(username.clone(), password.clone()));
                    }
                    b.port(port).build()
                }
            }
        };

        let sender_mailbox: lettre::message::Mailbox = match sender_str.parse() {
            Ok(m) => m,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("INVALID_FROM",
                format!("Invalid from address '{}': {}", sender_str, e))),
        };

        let mut email_builder = Message::builder()
            .from(sender_mailbox)
            .subject(subject.clone());

        for recipient in to.split(',') {
            let addr = recipient.trim();
            let mailbox: lettre::message::Mailbox = match addr.parse() {
                Ok(m) => m,
                Err(_) => return NodeOutput::failure(NodeError::unrecoverable("INVALID_TO",
                    format!("Invalid recipient address: {}", addr))),
            };
            email_builder = email_builder.to(mailbox);
        }

        let email = if is_html {
            email_builder.multipart(
                MultiPart::alternative()
                    .singlepart(SinglePart::builder()
                        .header(ContentType::TEXT_PLAIN)
                        .body(strip_html(&body)))
                    .singlepart(SinglePart::builder()
                        .header(ContentType::TEXT_HTML)
                        .body(body.clone()))
            )
        } else {
            email_builder.singlepart(
                SinglePart::builder()
                    .header(ContentType::TEXT_PLAIN)
                    .body(body.clone())
            )
        };

        let email = match email {
            Ok(e) => e,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("BUILD_ERROR",
                format!("Failed to build email: {}", e))),
        };

        match transport.send(email).await {
            Ok(_) => NodeOutput::success_with_logs(
                json!({ "sent": true, "to": to, "subject": subject }),
                vec![format!("Email sent to {} via {}:{}", to, smtp_host, port)],
            ),
            Err(e) => NodeOutput::failure(NodeError::recoverable("SMTP_ERROR",
                format!("Send failed: {}. Check credentials and SMTP settings.", e))),
        }
    }
}

fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}
