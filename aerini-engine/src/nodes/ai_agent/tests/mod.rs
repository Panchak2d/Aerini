use serde_json::Value;

use crate::model::{NodeInput, ExecutionContext};

mod request;
mod errors;
mod tools;
mod structured;
mod search;
mod continuation;

/// Serves one fixed OpenAI-shaped reply.
async fn spawn_minimal_openai_agent_mock() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock bind failed");
    let port = listener.local_addr().expect("local_addr failed").port();
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let (mut stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(_) => return,
        };
        let (r, mut w) = stream.split();
        let mut reader = BufReader::new(r);
        let mut req_line = String::new();
        let _ = reader.read_line(&mut req_line).await;
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 { break; }
            if line.trim().is_empty() { break; }
            if let Some((k, v)) = line.trim().split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            let _ = reader.read_exact(&mut body).await;
        }
        let resp_body = r#"{"choices":[{"message":{"content":"hello"},"finish_reason":"stop"}]}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            resp_body.len(), resp_body
        );
        let _ = w.write_all(resp.as_bytes()).await;
    });
    format!("http://127.0.0.1:{}", port)
}

/// A request a mock server received: its request line and body.
struct CapturedRequest { request_line: String, body: String }

/// like `spawn_minimal_openai_agent_mock`, but also captures the
/// request line + body so a test can assert *what was sent* (specifically,
/// which "model" value the node chose when the caller omitted one) rather
/// than only whether a response was received. Gemini puts its model in the
/// URL path (`/models/<model>:generateContent`), not the JSON body, so both
/// are captured.
async fn spawn_capturing_mock(resp_body: &'static str) -> (String, tokio::sync::oneshot::Receiver<CapturedRequest>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock bind failed");
    let port = listener.local_addr().expect("local_addr failed").port();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let (mut stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(_) => return,
        };
        let (r, mut w) = stream.split();
        let mut reader = BufReader::new(r);
        let mut req_line = String::new();
        let _ = reader.read_line(&mut req_line).await;
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 { break; }
            if line.trim().is_empty() { break; }
            if let Some((k, v)) = line.trim().split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut body_bytes = vec![0u8; content_length];
        if content_length > 0 {
            let _ = reader.read_exact(&mut body_bytes).await;
        }
        let _ = tx.send(CapturedRequest {
            request_line: req_line.trim().to_string(),
            body: String::from_utf8_lossy(&body_bytes).to_string(),
        });
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            resp_body.len(), resp_body
        );
        let _ = w.write_all(resp.as_bytes()).await;
    });
    (format!("http://127.0.0.1:{}", port), rx)
}

/// like `spawn_minimal_openai_agent_mock`, but returns a
/// caller-chosen HTTP status line instead of always "200 OK" — needed to
/// exercise the 429/529-recoverable classification path.
async fn spawn_status_mock(status_line: &'static str, resp_body: &'static str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock bind failed");
    let port = listener.local_addr().expect("local_addr failed").port();
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let (mut stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(_) => return,
        };
        let (r, mut w) = stream.split();
        let mut reader = BufReader::new(r);
        let mut req_line = String::new();
        let _ = reader.read_line(&mut req_line).await;
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 { break; }
            if line.trim().is_empty() { break; }
            if let Some((k, v)) = line.trim().split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            let _ = reader.read_exact(&mut body).await;
        }
        let resp = format!(
            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            status_line, resp_body.len(), resp_body
        );
        let _ = w.write_all(resp.as_bytes()).await;
    });
    format!("http://127.0.0.1:{}", port)
}

fn make_input(val: serde_json::Value) -> NodeInput {
    NodeInput {
        resolved_credentials: std::collections::HashMap::new(),
        cancel_token: None,
        node_id:      "n1".into(),
        workflow_id:  "w1".into(),
        execution_id: "e1".into(),
        input:        val,
        context:      ExecutionContext::default(),
    }
}

const LOOKUP_TOOL: &str = r#"[{"name":"lookup","description":"d","parameters":{"type":"object","properties":{}}}]"#;

const PLAIN_OPENAI_REPLY: &str = r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#;

fn with_fields(mut base: Value, extra: Value) -> Value {
    base.as_object_mut().expect("object").extend(extra.as_object().expect("object").clone());
    base
}

const RESPONSE_SCHEMA: &str = r#"{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"],"additionalProperties":false}"#;
