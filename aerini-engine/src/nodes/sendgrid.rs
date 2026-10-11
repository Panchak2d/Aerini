use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

const MAX_RECIPIENTS: usize = 1000;
const MAX_ADDRESS_CHARS: usize = 254;
const MAX_FROM_NAME_CHARS: usize = 200;

fn is_valid_address(address: &str) -> bool {
    if address.is_empty()
        || address.chars().count() > MAX_ADDRESS_CHARS
        || address.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | ';' | '"'))
    {
        return false;
    }
    match address.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && !domain.contains("..")
        }
        None => false,
    }
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Recipients from a string split on commas and semicolons, or an array of
/// such strings. Every address is checked, duplicates (ignoring case) are
/// dropped, and the list is capped at SendGrid's 1000 personalizations.
fn recipients(to: &Value) -> Result<Vec<String>, NodeError> {
    let mut pieces: Vec<&str> = Vec::new();
    match to {
        Value::Null => {}
        Value::String(s) => pieces.extend(s.split([',', ';'])),
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(s) => pieces.extend(s.split([',', ';'])),
                    other => {
                        return Err(NodeError::unrecoverable(
                            "INVALID_TO_EMAIL",
                            format!("every item in to_email must be a string (got {})", json_type(other)),
                        ))
                    }
                }
            }
        }
        other => {
            return Err(NodeError::unrecoverable(
                "INVALID_TO_EMAIL",
                format!("to_email must be a string or an array of strings (got {})", json_type(other)),
            ))
        }
    }

    let mut out: Vec<String> = Vec::new();
    for piece in pieces.into_iter().map(str::trim).filter(|p| !p.is_empty()) {
        if !is_valid_address(piece) {
            return Err(NodeError::unrecoverable(
                "INVALID_TO_EMAIL",
                format!("'{}' is not a valid email address", piece.chars().take(100).collect::<String>()),
            ));
        }
        if !out.iter().any(|seen| seen.eq_ignore_ascii_case(piece)) {
            out.push(piece.to_string());
        }
    }
    if out.is_empty() {
        return Err(NodeError::unrecoverable("MISSING_TO_EMAIL", "to_email is required"));
    }
    if out.len() > MAX_RECIPIENTS {
        return Err(NodeError::unrecoverable(
            "TOO_MANY_RECIPIENTS",
            format!("to_email has {} recipients; SendGrid accepts at most {} per send", out.len(), MAX_RECIPIENTS),
        ));
    }
    Ok(out)
}

/// The optional sender display name. Blank or missing is no name.
fn from_name(v: &Value) -> Result<Option<String>, NodeError> {
    let invalid = |why: String| NodeError::unrecoverable("INVALID_FROM_NAME", format!("from_name {why}"));
    match v {
        Value::Null => Ok(None),
        Value::String(s) => {
            let name = s.trim();
            if name.is_empty() {
                Ok(None)
            } else if name.chars().any(char::is_control) {
                Err(invalid("must not contain line breaks or control characters".to_string()))
            } else if name.chars().count() > MAX_FROM_NAME_CHARS {
                Err(invalid(format!("is longer than {MAX_FROM_NAME_CHARS} characters")))
            } else {
                Ok(Some(name.to_string()))
            }
        }
        other => Err(invalid(format!("must be text (got {})", json_type(other)))),
    }
}

/// One personalization per recipient, so each person gets their own copy and
/// cannot see the other addresses.
fn build_payload(recipients: &[String], from_email: &str, from_name: Option<&str>, subject: &str, body: &str) -> Value {
    let mut from = json!({ "email": from_email });
    if let Some(name) = from_name {
        from["name"] = Value::String(name.to_string());
    }
    json!({
        "personalizations": recipients.iter().map(|e| json!({ "to": [{ "email": e }] })).collect::<Vec<_>>(),
        "from":    from,
        "subject": subject,
        "content": [{ "type": "text/plain", "value": body }]
    })
}

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
                "to_email":   { "type": "string", "description": "Recipient email address, or several separated by commas (up to 1000; each gets a separate copy)" },
                "from_email": { "type": "string", "description": "Sender email address (must be verified in SendGrid). Write the bare address; use from_name for a display name." },
                "from_name":  { "type": "string", "description": "Sender display name (optional, max 200 characters)" },
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

        let to_emails = match recipients(&input.input["to_email"]) {
            Ok(list) => list,
            Err(e) => return NodeOutput::failure(e),
        };

        let from_email = match input.input["from_email"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            Some(e) if is_valid_address(e) => e.to_string(),
            Some(_) => return NodeOutput::failure(NodeError::unrecoverable("INVALID_FROM_EMAIL", "from_email is not a valid email address (write the bare address; use from_name for a sender name)")),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_FROM_EMAIL", "from_email is required")),
        };

        let from_name = match from_name(&input.input["from_name"]) {
            Ok(n) => n,
            Err(e) => return NodeOutput::failure(e),
        };

        let subject = match input.input["subject"].as_str().filter(|s| !s.is_empty()) {
            Some(s) => s.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_SUBJECT", "subject is required")),
        };

        let body_text = match &input.input["body"] {
            Value::String(b) if !b.trim().is_empty() => b.clone(),
            Value::Null | Value::String(_) => return NodeOutput::failure(NodeError::unrecoverable("MISSING_BODY", "body is required and must not be empty")),
            other => return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_BODY",
                format!("body must be text (got {})", json_type(other)),
            )),
        };

        let payload = build_payload(&to_emails, &from_email, from_name.as_deref(), &subject, &body_text);

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
                        vec![match to_emails.as_slice() {
                            [only] => format!("Email sent to {} via SendGrid", only),
                            many => format!("Email sent to {} recipients via SendGrid", many.len()),
                        }],
                    )
                } else {
                    let body_text = super::util::read_text_capped(resp, super::util::MAX_ERROR_BODY_BYTES).await;
                    NodeOutput::failure(super::util::provider_error(
                        status,
                        "SENDGRID_ERROR",
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
    use super::*;

    fn list(to: Value) -> Vec<String> {
        recipients(&to).expect("valid recipients")
    }

    fn code(to: Value) -> String {
        recipients(&to).expect_err("must be refused").code
    }

    #[test]
    fn comma_and_semicolon_lists_become_separate_recipients() {
        assert_eq!(list(json!("a@x.io")), ["a@x.io"]);
        assert_eq!(list(json!(" a@x.io, b@y.io ;c@z.io,")), ["a@x.io", "b@y.io", "c@z.io"]);
        assert_eq!(list(json!(["a@x.io, b@y.io", "c@z.io"])), ["a@x.io", "b@y.io", "c@z.io"]);
    }

    #[test]
    fn duplicate_recipients_are_sent_once_ignoring_case() {
        assert_eq!(list(json!("a@x.io, A@X.io, b@y.io")), ["a@x.io", "b@y.io"]);
    }

    #[test]
    fn bad_recipients_fail_before_any_request() {
        assert_eq!(code(json!("not-an-address")), "INVALID_TO_EMAIL");
        assert_eq!(code(json!("a@x.io, Jane <j@x.io>")), "INVALID_TO_EMAIL");
        assert_eq!(code(json!("a@@x.io")), "INVALID_TO_EMAIL");
        assert_eq!(code(json!("a@x")), "INVALID_TO_EMAIL");
        assert_eq!(code(json!(5)), "INVALID_TO_EMAIL");
        assert_eq!(code(json!(["a@x.io", 5])), "INVALID_TO_EMAIL");
        assert_eq!(code(json!(null)), "MISSING_TO_EMAIL");
        assert_eq!(code(json!(" , ; ")), "MISSING_TO_EMAIL");
    }

    #[test]
    fn recipient_count_is_capped_at_1000() {
        let at_limit: Vec<String> = (0..1000).map(|i| format!("u{i}@x.io")).collect();
        assert_eq!(list(json!(at_limit)).len(), 1000);
        let over: Vec<String> = (0..1001).map(|i| format!("u{i}@x.io")).collect();
        assert_eq!(code(json!(over)), "TOO_MANY_RECIPIENTS");
    }

    #[test]
    fn each_recipient_gets_their_own_personalization() {
        let payload = build_payload(&["a@x.io".into(), "b@y.io".into()], "me@x.io", None, "Hi", "Body");
        let p = payload["personalizations"].as_array().unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0]["to"], json!([{ "email": "a@x.io" }]));
        assert_eq!(p[1]["to"], json!([{ "email": "b@y.io" }]));
        assert_eq!(payload["content"][0]["value"], "Body");
        assert!(payload["from"].get("name").is_none());
    }

    #[test]
    fn from_name_is_sent_only_when_set_and_bad_names_are_refused() {
        let payload = build_payload(&["a@x.io".into()], "me@x.io", Some("Acme Support"), "Hi", "Body");
        assert_eq!(payload["from"], json!({ "email": "me@x.io", "name": "Acme Support" }));
        assert_eq!(from_name(&json!("  ")).unwrap(), None);
        assert_eq!(from_name(&json!(null)).unwrap(), None);
        assert_eq!(from_name(&json!(" Acme ")).unwrap().as_deref(), Some("Acme"));
        assert_eq!(from_name(&json!("a\nb")).unwrap_err().code, "INVALID_FROM_NAME");
        assert_eq!(from_name(&json!(5)).unwrap_err().code, "INVALID_FROM_NAME");
        assert_eq!(from_name(&json!("x".repeat(201))).unwrap_err().code, "INVALID_FROM_NAME");
        assert!(from_name(&json!("x".repeat(200))).is_ok());
    }

    fn node_input(cfg: Value) -> crate::model::NodeInput {
        crate::model::NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input: cfg,
            context: crate::model::ExecutionContext::default(),
        }
    }

    async fn failure_code(cfg: Value) -> String {
        SendGridNode.execute(node_input(cfg)).await.error.expect("must fail").code
    }

    #[tokio::test]
    async fn empty_or_non_text_body_and_bad_sender_fail_before_any_request() {
        let ok = json!({ "api_key": "SG.x", "to_email": "a@x.io", "from_email": "me@x.io", "subject": "Hi", "body": "Body" });
        let with = |key: &str, value: Value| {
            let mut cfg = ok.clone();
            cfg[key] = value;
            cfg
        };
        assert_eq!(failure_code(with("body", json!(""))).await, "MISSING_BODY");
        assert_eq!(failure_code(with("body", json!("  \n"))).await, "MISSING_BODY");
        assert_eq!(failure_code(with("body", json!(7))).await, "INVALID_BODY");
        assert_eq!(failure_code(with("from_email", json!("nope"))).await, "INVALID_FROM_EMAIL");
        assert_eq!(failure_code(with("to_email", json!("a@x.io, nope"))).await, "INVALID_TO_EMAIL");
    }
}
