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
    fn description(&self) -> &'static str { "Send an email via SMTP with plain text or HTML content and multiple recipients." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["smtp_host", "from", "to", "subject", "body"],
            "properties": {
                "smtp_host": { "type": "string", "description": "SMTP server e.g. smtp.gmail.com" },
                "smtp_port": { "type": "number", "description": "587 (STARTTLS, recommended) or 465 (SSL)", "default": 587 },
                "from":      { "type": "string", "description": "Sender email address" },
                "to":        { "type": "string", "description": "Recipient(s), comma-separated. Maximum 50 recipients." },
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

/// Missing or null means the default (587). Otherwise the port must be a
/// whole number from 1 to 65535, given as a number or as digits in a string.
fn parse_smtp_port(v: &Value) -> Result<u16, NodeError> {
    let parsed = match v {
        Value::Null => return Ok(587),
        Value::Number(n) => n.as_u64(),
        Value::String(s) if !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()) => s.parse::<u64>().ok(),
        _ => None,
    };
    parsed
        .filter(|p| (1..=65535).contains(p))
        .map(|p| p as u16)
        .ok_or_else(|| NodeError::unrecoverable("INVALID_PORT", "smtp_port must be a whole number from 1 to 65535"))
}

async fn send_email(cfg: &Value) -> Result<String, NodeError> {
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;

    let smtp_host = cfg["smtp_host"].as_str().filter(|s| !s.is_empty())
        .ok_or_else(|| NodeError::unrecoverable("MISSING_SMTP_HOST", "smtp_host is required"))?
        .to_string();
    let smtp_port = parse_smtp_port(&cfg["smtp_port"])?;

    let ssrf_host = url::Host::parse(&smtp_host)
        .map_err(|_| NodeError::unrecoverable("INVALID_HOST", "Invalid smtp_host"))?;
    let ssrf_host_ref: url::Host<&str> = match &ssrf_host {
        url::Host::Domain(s) => url::Host::Domain(s.as_str()),
        url::Host::Ipv4(ip)  => url::Host::Ipv4(*ip),
        url::Host::Ipv6(ip)  => url::Host::Ipv6(*ip),
    };
    crate::nodes::util::check_host_ssrf(ssrf_host_ref, smtp_port, crate::nodes::util::SsrfPolicy::Strict).await
        .map_err(|e| NodeError::unrecoverable("SSRF_BLOCKED", &e))?;

    let from_str = cfg["from"].as_str().filter(|s| !s.is_empty())
        .ok_or_else(|| NodeError::unrecoverable("MISSING_FROM", "from address is required"))?;
    let to_str = cfg["to"].as_str().filter(|s| !s.is_empty())
        .ok_or_else(|| NodeError::unrecoverable("MISSING_TO", "to address is required"))?
        .to_string();

    let subject = cfg["subject"].as_str().unwrap_or("(no subject)");
    let body    = cfg["body"].as_str().unwrap_or("").to_string();
    let html    = cfg["html"].as_bool().unwrap_or(false);

    let from_mbox = from_str.parse::<lettre::message::Mailbox>()
        .map_err(|e| NodeError::unrecoverable("INVALID_FROM", format!("Invalid from address: {e}")))?;

    const MAX_RECIPIENTS: usize = 50;
    let recipient_count = to_str.split(',').filter(|s| !s.trim().is_empty()).count();
    if recipient_count > MAX_RECIPIENTS {
        return Err(NodeError::unrecoverable(
            "TOO_MANY_RECIPIENTS",
            format!("Recipient count ({}) exceeds the maximum of {}.", recipient_count, MAX_RECIPIENTS),
        ));
    }

    let content_type = if html { ContentType::TEXT_HTML } else { ContentType::TEXT_PLAIN };

    let mut msg_builder = Message::builder().from(from_mbox).subject(subject);
    for addr in to_str.split(',') {
        let addr = addr.trim();
        if addr.is_empty() { continue; }
        let mbox = addr.parse::<lettre::message::Mailbox>()
            .map_err(|e| NodeError::unrecoverable("INVALID_TO", format!("Invalid to address '{addr}': {e}")))?;
        msg_builder = msg_builder.to(mbox);
    }
    let message = msg_builder
        .header(content_type)
        .body(body)
        .map_err(|e| NodeError::unrecoverable("BUILD_ERROR", format!("Failed to build email: {e}")))?;

    let username = cfg["username"].as_str().unwrap_or("").to_string();
    let password = cfg["password"].as_str().unwrap_or("").to_string();

    let mut builder = if smtp_port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp_host)
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp_host)
    }.map_err(|e| NodeError::unrecoverable("TRANSPORT_ERROR", format!("SMTP transport init failed: {e}")))?;

    builder = builder.port(smtp_port);

    if username.is_empty() != password.is_empty() {
        return Err(NodeError::unrecoverable(
            "INCOMPLETE_CREDENTIALS",
            "username and password must be set together (one is missing)",
        ));
    }
    if !username.is_empty() {
        builder = builder.credentials(Credentials::new(username, password));
    }

    let transport = builder.build();

    transport.send(message).await
        .map_err(|e| NodeError::recoverable("SMTP_ERROR", format!("SMTP send failed: {e}")))?;

    Ok(format!("Email sent to {to_str} via {smtp_host}:{smtp_port}"))
}


#[cfg(test)]
mod port_tests {
    use super::*;

    #[test]
    fn smtp_port_is_validated_not_wrapped() {
        assert_eq!(parse_smtp_port(&Value::Null).unwrap(), 587);
        assert_eq!(parse_smtp_port(&json!(465)).unwrap(), 465);
        assert_eq!(parse_smtp_port(&json!("2525")).unwrap(), 2525);
        for bad in [json!(65536), json!(66587), json!(0), json!(-1), json!(25.5), json!("abc"), json!(true)] {
            assert_eq!(parse_smtp_port(&bad).unwrap_err().code, "INVALID_PORT", "{bad}");
        }
    }
}
