use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;
use super::util::cfg_bool_opt;

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
                "to":        { "type": "string", "description": "Recipient(s): comma-separated, or an array of addresses from an upstream node. A comma inside quotes or <angle brackets>, as in \"Doe, John\" <a@b.c>, does not split. Maximum 50 recipients." },
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

const MAX_RECIPIENTS: usize = 50;

/// Limits for one send. The server gets `command` to answer each step before the
/// message body is sent (RFC 5321 suggests minutes, far longer than a workflow
/// should wait) and `body` to take the body and give its final reply.
#[derive(Clone, Copy)]
struct Limits {
    command: std::time::Duration,
    body: std::time::Duration,
}

const LIMITS: Limits = Limits {
    command: std::time::Duration::from_secs(30),
    body: std::time::Duration::from_secs(120),
};

/// TCP connect limit handed to lettre. Its timeout covers the connect only.
const TCP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const QUIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

type SmtpError = lettre::transport::smtp::Error;

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

/// Splits on commas that are outside double quotes and outside `<...>`, so
/// `"Doe, John" <a@b.c>, x@y.z` is two recipients. Empty pieces are dropped.
fn split_recipients(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut in_quote, mut escaped, mut in_angle) = (0, false, false, false);
    for (i, c) in s.char_indices() {
        if in_quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_quote = false;
            }
            continue;
        }
        match c {
            '"' => in_quote = true,
            '<' => in_angle = true,
            '>' => in_angle = false,
            ',' if !in_angle => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect()
}

fn recipients(to: &Value) -> Result<Vec<String>, NodeError> {
    let mut out: Vec<String> = Vec::new();
    match to {
        Value::Null => {}
        Value::String(s) => out.extend(split_recipients(s).into_iter().map(String::from)),
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(s) => out.extend(split_recipients(s).into_iter().map(String::from)),
                    other => {
                        return Err(NodeError::unrecoverable(
                            "INVALID_TO",
                            format!("every item in to must be a string (got {})", json_type(other)),
                        ))
                    }
                }
            }
        }
        other => {
            return Err(NodeError::unrecoverable(
                "INVALID_TO",
                format!("to must be a string or an array of strings (got {})", json_type(other)),
            ))
        }
    }
    if out.is_empty() {
        return Err(NodeError::unrecoverable("MISSING_TO", "to address is required"));
    }
    Ok(out)
}

/// A required text field: absent or null is `missing_code`, any other
/// non-string is `invalid_code`. An empty string is a present value.
fn required_text<'a>(
    cfg: &'a Value,
    field: &str,
    missing_code: &str,
    invalid_code: &str,
) -> Result<&'a str, NodeError> {
    match &cfg[field] {
        Value::String(s) => Ok(s),
        Value::Null => Err(NodeError::unrecoverable(missing_code, format!("{field} is required"))),
        other => Err(NodeError::unrecoverable(
            invalid_code,
            format!("{field} must be a string (got {})", json_type(other)),
        )),
    }
}

struct Prepared {
    message: lettre::Message,
    smtp_host: String,
    smtp_port: u16,
    ssrf_host: url::Host<String>,
    tls: lettre::transport::smtp::client::Tls,
    credentials: Option<lettre::transport::smtp::authentication::Credentials>,
    to_display: String,
}

/// Every config check, with no network access.
fn prepare(cfg: &Value) -> Result<Prepared, NodeError> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;

    let smtp_host = cfg["smtp_host"].as_str().filter(|s| !s.is_empty())
        .ok_or_else(|| NodeError::unrecoverable("MISSING_SMTP_HOST", "smtp_host is required"))?
        .to_string();
    let smtp_port = parse_smtp_port(&cfg["smtp_port"])?;
    let ssrf_host = url::Host::parse(&smtp_host)
        .map_err(|_| NodeError::unrecoverable("INVALID_HOST", "Invalid smtp_host"))?;

    let from_str = cfg["from"].as_str().filter(|s| !s.is_empty())
        .ok_or_else(|| NodeError::unrecoverable("MISSING_FROM", "from address is required"))?;
    let to_list = recipients(&cfg["to"])?;
    let subject = required_text(cfg, "subject", "MISSING_SUBJECT", "INVALID_SUBJECT")?;
    let body = required_text(cfg, "body", "MISSING_BODY", "INVALID_BODY")?.to_string();
    let html = cfg_bool_opt(&cfg["html"], "html")?.unwrap_or(false);

    let from_mbox = from_str.parse::<lettre::message::Mailbox>()
        .map_err(|e| NodeError::unrecoverable("INVALID_FROM", format!("Invalid from address: {e}")))?;

    if to_list.len() > MAX_RECIPIENTS {
        return Err(NodeError::unrecoverable(
            "TOO_MANY_RECIPIENTS",
            format!("Recipient count ({}) exceeds the maximum of {}.", to_list.len(), MAX_RECIPIENTS),
        ));
    }

    let content_type = if html { ContentType::TEXT_HTML } else { ContentType::TEXT_PLAIN };
    let mut msg_builder = lettre::Message::builder().from(from_mbox).subject(subject);
    for addr in &to_list {
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
    if username.is_empty() != password.is_empty() {
        return Err(NodeError::unrecoverable(
            "INCOMPLETE_CREDENTIALS",
            "username and password must be set together (one is missing)",
        ));
    }
    let credentials = (!username.is_empty()).then(|| Credentials::new(username, password));

    let tls_params = lettre::transport::smtp::client::TlsParameters::new(smtp_host.clone())
        .map_err(|e| NodeError::unrecoverable("TRANSPORT_ERROR", format!("SMTP transport init failed: {e}")))?;
    let tls = if smtp_port == 465 {
        lettre::transport::smtp::client::Tls::Wrapper(tls_params)
    } else {
        lettre::transport::smtp::client::Tls::Required(tls_params)
    };

    Ok(Prepared { message, smtp_host, smtp_port, ssrf_host, tls, credentials, to_display: to_list.join(", ") })
}

/// How far a send got. Everything before `Body` happens before any message
/// data leaves this machine, so a failure there cannot have delivered the email.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Connect,
    Auth,
    Envelope,
    Body,
}

fn classify(phase: Phase, e: &SmtpError) -> NodeError {
    if e.is_permanent() {
        let what = match phase {
            Phase::Connect => "connection",
            Phase::Auth => "login",
            Phase::Envelope | Phase::Body => "message",
        };
        return NodeError::unrecoverable("SMTP_ERROR", format!("SMTP server rejected the {what}: {e}"));
    }
    if e.is_client() {
        return NodeError::unrecoverable("SMTP_ERROR", format!("SMTP send failed: {e}"));
    }
    if phase == Phase::Body {
        if e.is_transient() {
            return NodeError::unrecoverable(
                "SMTP_ERROR",
                format!("SMTP server refused the message for now: {e}. Not retried automatically, because the refusal came after the message body was sent. Run the workflow again to retry."),
            );
        }
        return NodeError::unrecoverable(
            "SMTP_DELIVERY_UNCERTAIN",
            format!("SMTP connection failed or was lost before the server confirmed the message: {e}. The email may or may not have been delivered; check before sending it again."),
        );
    }
    let step = match phase {
        Phase::Connect => "connection",
        Phase::Auth => "login",
        Phase::Envelope | Phase::Body => "message setup",
    };
    NodeError::recoverable(
        "SMTP_ERROR",
        format!("SMTP {step} failed: {e}. The server never received the message, so trying again is safe."),
    )
}

fn timed_out(phase: Phase, what: &str, limit: std::time::Duration) -> NodeError {
    if phase == Phase::Body {
        return NodeError::unrecoverable(
            "SMTP_DELIVERY_UNCERTAIN",
            format!("SMTP server did not finish {what} within {} s and the send was stopped. The email may or may not have been delivered; check before sending it again.", limit.as_secs()),
        );
    }
    NodeError::recoverable(
        "SMTP_ERROR",
        format!("SMTP server did not answer {what} within {} s. The server never received the message, so trying again is safe.", limit.as_secs()),
    )
}

/// Runs one SMTP step under its time limit and names the failure by phase.
async fn within<T, F>(limit: std::time::Duration, phase: Phase, what: &str, step: F) -> Result<T, NodeError>
where
    F: std::future::Future<Output = Result<T, SmtpError>>,
{
    match tokio::time::timeout(limit, step).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(classify(phase, &e)),
        Err(_) => Err(timed_out(phase, what, limit)),
    }
}

async fn open_connection(
    addr: std::net::SocketAddr,
    tls: &lettre::transport::smtp::client::Tls,
    hello: &lettre::transport::smtp::extension::ClientId,
) -> Result<lettre::transport::smtp::client::AsyncSmtpConnection, SmtpError> {
    use lettre::transport::smtp::client::{AsyncSmtpConnection, Tls};

    let implicit = match tls {
        Tls::Wrapper(params) => Some(params.clone()),
        _ => None,
    };
    let mut conn = AsyncSmtpConnection::connect_tokio1(addr, Some(TCP_CONNECT_TIMEOUT), hello, implicit, None).await?;
    if let Tls::Required(params) = tls {
        conn.starttls(params.clone(), hello).await?;
    }
    Ok(conn)
}

/// Connects to the first validated address that answers. A connection-level
/// failure moves on to the next address; a refusal by the server itself does not.
async fn connect(
    addrs: &[std::net::SocketAddr],
    tls: &lettre::transport::smtp::client::Tls,
    hello: &lettre::transport::smtp::extension::ClientId,
    limits: Limits,
) -> Result<lettre::transport::smtp::client::AsyncSmtpConnection, NodeError> {
    let mut last = None;
    for addr in addrs {
        match within(limits.command, Phase::Connect, "the connection", open_connection(*addr, tls, hello)).await {
            Ok(conn) => return Ok(conn),
            Err(e) if e.recoverable => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| NodeError::recoverable("SMTP_ERROR", "SMTP server has no address to connect to")))
}

/// Sends `message` step by step so every failure is known to be before or
/// after the body. Only a failure at or after the body can leave the email
/// delivered, so only those are never retried.
async fn deliver_message(
    addrs: &[std::net::SocketAddr],
    tls: &lettre::transport::smtp::client::Tls,
    credentials: Option<&lettre::transport::smtp::authentication::Credentials>,
    message: &lettre::Message,
    limits: Limits,
) -> Result<(), NodeError> {
    use lettre::transport::smtp::authentication::DEFAULT_MECHANISMS;
    use lettre::transport::smtp::commands::{Data, Mail, Rcpt};
    use lettre::transport::smtp::extension::{ClientId, Extension, MailBodyParameter, MailParameter};

    let hello = ClientId::default();
    let mut conn = connect(addrs, tls, &hello, limits).await?;

    if let Some(credentials) = credentials {
        within(limits.command, Phase::Auth, "the login", conn.auth(DEFAULT_MECHANISMS, credentials)).await?;
    }

    let envelope = message.envelope();
    let raw = message.formatted();
    let mut mail_options = Vec::new();
    let non_ascii_address = |a: &lettre::Address| !AsRef::<str>::as_ref(a).is_ascii();
    if envelope.from().is_some_and(non_ascii_address) || envelope.to().iter().any(non_ascii_address) {
        if !conn.server_info().supports_feature(Extension::SmtpUtfEight) {
            return Err(NodeError::unrecoverable(
                "SMTP_ERROR",
                "An address contains non-ASCII characters but the server does not support SMTPUTF8",
            ));
        }
        mail_options.push(MailParameter::SmtpUtfEight);
    }
    if !raw.is_ascii() {
        if !conn.server_info().supports_feature(Extension::EightBitMime) {
            return Err(NodeError::unrecoverable(
                "SMTP_ERROR",
                "The message contains non-ASCII characters but the server does not support 8BITMIME",
            ));
        }
        mail_options.push(MailParameter::Body(MailBodyParameter::EightBitMime));
    }

    within(limits.command, Phase::Envelope, "MAIL FROM", conn.command(Mail::new(envelope.from().cloned(), mail_options))).await?;
    for recipient in envelope.to() {
        within(limits.command, Phase::Envelope, "RCPT TO", conn.command(Rcpt::new(recipient.clone(), vec![]))).await?;
    }
    within(limits.command, Phase::Envelope, "DATA", conn.command(Data)).await?;
    within(limits.body, Phase::Body, "the message body", conn.message(&raw)).await?;

    let _ = tokio::time::timeout(QUIT_TIMEOUT, conn.quit()).await;
    Ok(())
}

async fn send_email(cfg: &Value) -> Result<String, NodeError> {
    let prepared = prepare(cfg)?;

    let ssrf_host_ref: url::Host<&str> = match &prepared.ssrf_host {
        url::Host::Domain(s) => url::Host::Domain(s.as_str()),
        url::Host::Ipv4(ip)  => url::Host::Ipv4(*ip),
        url::Host::Ipv6(ip)  => url::Host::Ipv6(*ip),
    };
    let validated = crate::nodes::util::resolve_host_validated(ssrf_host_ref, prepared.smtp_port, crate::nodes::util::SsrfPolicy::Strict).await
        .map_err(|e| NodeError::unrecoverable("SSRF_BLOCKED", &e))?;

    deliver_message(&validated, &prepared.tls, prepared.credentials.as_ref(), &prepared.message, LIMITS).await?;

    Ok(format!("Email sent to {} via {}:{}", prepared.to_display, prepared.smtp_host, prepared.smtp_port))
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

#[cfg(test)]
mod send_tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    fn valid_cfg() -> Value {
        json!({
            "smtp_host": "smtp.example.com",
            "from": "me@example.com",
            "to": "you@example.com",
            "subject": "Hi",
            "body": "Hello",
        })
    }

    fn code_of(cfg: Value) -> String {
        prepare(&cfg).err().expect("config should be refused").code
    }

    #[test]
    fn recipients_split_outside_quotes_and_angle_brackets_only() {
        let list = recipients(&json!(r#""Doe, John" <a@b.c>, x@y.z ,,<"p,q"@r.s>"#)).unwrap();
        assert_eq!(list, vec![r#""Doe, John" <a@b.c>"#, "x@y.z", r#"<"p,q"@r.s>"#]);
        let parsed = prepare(&{
            let mut c = valid_cfg();
            c["to"] = json!(r#""Doe, John" <a@b.c>, x@y.z"#);
            c
        })
        .unwrap();
        assert_eq!(parsed.message.envelope().to().len(), 2);
    }

    #[test]
    fn array_to_from_an_upstream_node_gives_separate_recipients() {
        let list = recipients(&json!(["a@b.c", r#""Doe, John" <d@e.f>"#, "g@h.i, j@k.l"])).unwrap();
        assert_eq!(list.len(), 4);
    }

    #[test]
    fn to_of_the_wrong_type_or_with_no_recipient_is_a_named_error() {
        for bad in [json!(5), json!({"a": 1}), json!(true), json!([1]), json!(["a@b.c", null])] {
            assert_eq!(recipients(&bad).unwrap_err().code, "INVALID_TO", "{bad}");
        }
        for none in [Value::Null, json!(""), json!(" , "), json!([]), json!([""])] {
            assert_eq!(recipients(&none).unwrap_err().code, "MISSING_TO", "{none}");
        }
    }

    #[test]
    fn missing_or_non_string_subject_and_body_are_named_errors() {
        let without = |key: &str| {
            let mut c = valid_cfg();
            c.as_object_mut().unwrap().remove(key);
            c
        };
        assert_eq!(code_of(without("subject")), "MISSING_SUBJECT");
        assert_eq!(code_of(without("body")), "MISSING_BODY");
        let mut c = valid_cfg();
        c["subject"] = Value::Null;
        assert_eq!(code_of(c), "MISSING_SUBJECT");
        let mut c = valid_cfg();
        c["subject"] = json!(7);
        assert_eq!(code_of(c), "INVALID_SUBJECT");
        let mut c = valid_cfg();
        c["body"] = json!({"text": "x"});
        assert_eq!(code_of(c), "INVALID_BODY");
        let mut c = valid_cfg();
        c["subject"] = json!("");
        c["body"] = json!("");
        assert!(prepare(&c).is_ok());
    }

    #[tokio::test]
    async fn config_errors_surface_before_the_dns_lookup() {
        let cfg = json!({
            "smtp_host": "no-such-host.invalid",
            "from": "me@example.com",
            "to": "you@example.com",
            "body": "Hello",
        });
        assert_eq!(send_email(&cfg).await.unwrap_err().code, "MISSING_SUBJECT");
    }

    #[derive(Clone, Copy)]
    enum Script {
        Accept,
        GreetingBusy,
        TempRejectRecipient,
        RejectRecipient,
        BadLogin,
        StallAtMail,
        RefuseAfterBody,
        DropAfterBody,
        StallAfterBody,
    }

    async fn fake_smtp(script: Script) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read);
            if matches!(script, Script::GreetingBusy) {
                write.write_all(b"421 4.3.2 busy, try later\r\n").await.unwrap();
                return;
            }
            write.write_all(b"220 fake ready\r\n").await.unwrap();
            let mut line = String::new();
            loop {
                line.clear();
                if lines.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let verb = line.trim_end().to_ascii_uppercase();
                if verb.starts_with("EHLO") {
                    let reply: &[u8] = if matches!(script, Script::BadLogin) {
                        b"250-fake\r\n250 AUTH PLAIN\r\n"
                    } else {
                        b"250 fake\r\n"
                    };
                    write.write_all(reply).await.unwrap();
                } else if verb.starts_with("AUTH") {
                    write.write_all(b"535 5.7.8 bad credentials\r\n").await.unwrap();
                } else if verb.starts_with("MAIL") && matches!(script, Script::StallAtMail) {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                } else if verb.starts_with("RCPT") && matches!(script, Script::RejectRecipient) {
                    write.write_all(b"550 5.1.1 no such user\r\n").await.unwrap();
                } else if verb.starts_with("RCPT") && matches!(script, Script::TempRejectRecipient) {
                    write.write_all(b"451 4.7.1 greylisted, try later\r\n").await.unwrap();
                } else if verb.starts_with("DATA") {
                    write.write_all(b"354 go ahead\r\n").await.unwrap();
                    loop {
                        line.clear();
                        if lines.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        if line == ".\r\n" {
                            break;
                        }
                    }
                    match script {
                        Script::RefuseAfterBody => write.write_all(b"451 4.3.0 try later\r\n").await.unwrap(),
                        Script::DropAfterBody => return,
                        Script::StallAfterBody => tokio::time::sleep(std::time::Duration::from_secs(30)).await,
                        _ => write.write_all(b"250 2.0.0 queued\r\n").await.unwrap(),
                    }
                } else if verb.starts_with("QUIT") {
                    write.write_all(b"221 bye\r\n").await.unwrap();
                    return;
                } else {
                    write.write_all(b"250 ok\r\n").await.unwrap();
                }
            }
        });
        addr
    }

    const TEST_LIMITS: Limits = Limits {
        command: std::time::Duration::from_millis(400),
        body: std::time::Duration::from_millis(400),
    };

    fn test_message() -> lettre::Message {
        lettre::Message::builder()
            .from("me@example.com".parse().unwrap())
            .to("you@example.com".parse().unwrap())
            .subject("Hi")
            .body("Hello".to_string())
            .unwrap()
    }

    async fn send_to(addrs: &[std::net::SocketAddr], credentials: Option<&lettre::transport::smtp::authentication::Credentials>) -> Result<(), NodeError> {
        deliver_message(addrs, &lettre::transport::smtp::client::Tls::None, credentials, &test_message(), TEST_LIMITS).await
    }

    async fn failure_of(script: Script) -> NodeError {
        let addr = fake_smtp(script).await;
        send_to(&[addr], None).await.expect_err("the fake server never accepts this send")
    }

    fn shape(e: &NodeError) -> (&str, bool) {
        (e.code.as_str(), e.recoverable)
    }

    #[tokio::test]
    async fn a_complete_send_succeeds() {
        let addr = fake_smtp(Script::Accept).await;
        send_to(&[addr], None).await.expect("the fake server accepts");
    }

    #[tokio::test]
    async fn a_busy_greeting_and_a_greylisted_recipient_are_recoverable_because_nothing_was_sent() {
        assert_eq!(shape(&failure_of(Script::GreetingBusy).await), ("SMTP_ERROR", true));
        assert_eq!(shape(&failure_of(Script::TempRejectRecipient).await), ("SMTP_ERROR", true));
    }

    #[tokio::test]
    async fn a_silent_server_before_the_body_times_out_as_recoverable() {
        let e = failure_of(Script::StallAtMail).await;
        assert_eq!(shape(&e), ("SMTP_ERROR", true));
        assert!(e.message.contains("MAIL FROM"), "{}", e.message);
    }

    #[tokio::test]
    async fn permanent_replies_are_unrecoverable() {
        let e = failure_of(Script::RejectRecipient).await;
        assert_eq!(shape(&e), ("SMTP_ERROR", false));
        assert!(e.message.contains("550"), "{}", e.message);

        let addr = fake_smtp(Script::BadLogin).await;
        let creds = lettre::transport::smtp::authentication::Credentials::new("u".into(), "p".into());
        let e = send_to(&[addr], Some(&creds)).await.expect_err("login is refused");
        assert_eq!(shape(&e), ("SMTP_ERROR", false));
        assert!(e.message.contains("535") && e.message.contains("login"), "{}", e.message);
    }

    #[tokio::test]
    async fn transient_reply_after_the_body_is_not_retried() {
        let e = failure_of(Script::RefuseAfterBody).await;
        assert_eq!(shape(&e), ("SMTP_ERROR", false));
        assert!(e.message.contains("451"), "{}", e.message);
    }

    #[tokio::test]
    async fn drop_or_timeout_after_the_body_is_delivery_uncertain_and_unrecoverable() {
        assert_eq!(shape(&failure_of(Script::DropAfterBody).await), ("SMTP_DELIVERY_UNCERTAIN", false));
        assert_eq!(shape(&failure_of(Script::StallAfterBody).await), ("SMTP_DELIVERY_UNCERTAIN", false));
    }

    #[tokio::test]
    async fn a_refused_connection_is_recoverable_and_the_next_address_is_tried() {
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead = closed.local_addr().unwrap();
        drop(closed);
        let e = send_to(&[dead], None).await.expect_err("nothing listens");
        assert_eq!(shape(&e), ("SMTP_ERROR", true));

        let live = fake_smtp(Script::Accept).await;
        send_to(&[dead, live], None).await.expect("the second address answers");
    }
}
