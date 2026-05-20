use async_trait::async_trait;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{json, Map, Value};
use std::convert::Infallible;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use blake3;
use tokio::sync::{oneshot, Mutex};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

pub struct WebhookNode;

// Shared across the accept loop and each hyper service invocation.
struct HandlerState {
    path:   String,
    method: String,
    secret: String,
    // Taken on the first valid request; None afterwards signals the loop to exit.
    tx: Option<oneshot::Sender<Result<Value, String>>>,
}

#[async_trait]
impl Node for WebhookNode {
    fn type_id(&self) -> &'static str { "webhook" }
    fn display_name(&self) -> &'static str { "Webhook" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "port":         { "type": "number", "description": "Port to listen on (default 3456)" },
                "path":         { "type": "string", "description": "URL path (default /webhook)" },
                "method":       { "type": "string", "enum": ["GET","POST","PUT","ANY"] },
                "secret":       { "type": "string", "description": "Optional shared secret header" },
                "timeout_secs": { "type": "number", "description": "Wait timeout (default 60)" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "body":    {},
                "headers": { "type": "object" },
                "method":  { "type": "string" },
                "path":    { "type": "string" }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Triggered".to_string(), position: PortPosition::Right },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(),     position: PortPosition::Right },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let port = input.input["port"].as_u64().unwrap_or(3456) as u16;
        if port < 1024 {
            return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_PORT",
                "Webhook port must be 1024 or higher (privileged ports require root)",
            ));
        }
        let path         = input.input["path"].as_str().unwrap_or("/webhook").to_string();
        let method       = input.input["method"].as_str().unwrap_or("ANY").to_uppercase();
        let secret       = input.input["secret"].as_str().unwrap_or("").to_string();
        let timeout_secs = input.input["timeout_secs"].as_u64().unwrap_or(60);

        let (tx, rx) = oneshot::channel::<Result<Value, String>>();

        let listener = match tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await {
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("BIND_ERR", e.to_string())),
            Ok(l)  => l,
        };

        // Always warn: the webhook binds to 127.0.0.1 (loopback only).
        // In server/API deployments this means external traffic cannot reach it
        // without a reverse proxy forwarding to this port.
        tracing::warn!(
            "Webhook node bound to 127.0.0.1:{} — external traffic requires a reverse proxy \
             forwarding to this port. If running in desktop mode this is expected.",
            port
        );

        let state = Arc::new(Mutex::new(HandlerState {
            path:   path.clone(),
            method,
            secret,
            tx: Some(tx),
        }));

        // Captured for the success log message.
        let port_for_log = port;
        let path_for_log = path;

        tokio::spawn(async move {
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_secs(timeout_secs);

            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    let mut st = state.lock().await;
                    if let Some(s) = st.tx.take() {
                        let _ = s.send(Err("TIMEOUT".into()));
                    }
                    return;
                }

                let accept_result =
                    match tokio::time::timeout(remaining, listener.accept()).await {
                        Err(_) => {
                            let mut st = state.lock().await;
                            if let Some(s) = st.tx.take() {
                                let _ = s.send(Err("TIMEOUT".into()));
                            }
                            return;
                        }
                        Ok(r) => r,
                    };

                let (stream, _) = match accept_result {
                    Err(e) => {
                        let mut st = state.lock().await;
                        if let Some(s) = st.tx.take() {
                            let _ = s.send(Err(e.to_string()));
                        }
                        return;
                    }
                    Ok(s) => s,
                };

                // Per-connection read budget: capped at global deadline and 30s hard max.
                // Prevents a stalled sender from blocking the loop after a connection is accepted.
                let per_conn = deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .min(std::time::Duration::from_secs(30));

                let io      = TokioIo::new(stream);
                let state_c = Arc::clone(&state);

                // serve_connection drives the full HTTP/1.1 request/response exchange.
                // Chunked encoding, pipelining, large headers, and malformed requests are
                // all handled by hyper's parser — no manual byte slicing.
                let conn = http1::Builder::new().serve_connection(
                    io,
                    service_fn(move |req: Request<Incoming>| {
                        let state_i = Arc::clone(&state_c);
                        async move { handle_request(req, state_i).await }
                    }),
                );

                // Drive the connection under the per-connection deadline. Errors and
                // timeouts are discarded — invalid/dropped connections just cause the
                // loop to accept the next one.
                let _ = tokio::time::timeout(per_conn, conn).await;

                // tx being None means a valid request was handled inside service_fn.
                if state.lock().await.tx.is_none() {
                    return;
                }
                // tx still Some: method/path/secret mismatch or connection error.
                // Keep accepting.
            }
        });

        match rx.await {
            Err(_) => NodeOutput::failure(NodeError::unrecoverable("CHANNEL_ERR", "Internal error")),
            Ok(Err(e)) if e == "TIMEOUT" => NodeOutput::failure(NodeError::recoverable(
                "TIMEOUT",
                format!("No request in {}s", timeout_secs),
            )),
            Ok(Err(e)) => NodeOutput::failure(NodeError::unrecoverable("WEBHOOK_ERR", e)),
            Ok(Ok(data)) => NodeOutput::success_with_logs(
                data,
                vec![format!("Webhook received on :{}{}", port_for_log, path_for_log)],
            ),
        }
    }
}

/// Validates and processes a single HTTP request for the webhook.
///
/// Returns a Response in all cases — hyper requires an Infallible handler.
/// The oneshot sender inside `state` is taken only on a valid request; all
/// invalid requests return an HTTP error without consuming it, so the outer
/// accept loop knows to keep waiting.
async fn handle_request(
    req:   Request<Incoming>,
    state: Arc<Mutex<HandlerState>>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let req_method = req.method().as_str().to_string();
    let req_path   = req.uri().path().to_string();

    // Build headers map before consuming the request with into_body().
    let mut headers = Map::new();
    for (name, value) in req.headers() {
        if let Ok(v) = value.to_str() {
            headers.insert(name.as_str().to_string(), Value::String(v.to_string()));
        }
    }

    // Collect body. Hyper handles chunked transfer encoding and Content-Length
    // framing; we just read the aggregated bytes.
    let body_bytes = match req.into_body().collect().await {
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::new()))
                .unwrap());
        }
        Ok(b) => b.to_bytes(),
    };

    // Hard cap at 1 MB, matching prior behaviour.
    if body_bytes.len() > 1_000_000 {
        return Ok(Response::builder()
            .status(StatusCode::PAYLOAD_TOO_LARGE)
            .body(Full::new(Bytes::new()))
            .unwrap());
    }

    let body_str = String::from_utf8_lossy(&body_bytes).to_string();

    let mut st = state.lock().await;

    // Method mismatch: 405. Keep listening — caller used the wrong method.
    if st.method != "ANY" && req_method != st.method {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Full::new(Bytes::new()))
            .unwrap());
    }

    // Path mismatch: 401 (not 404). Returning 404 would confirm the port is
    // active and reveal that the guessed path did not match.
    if req_path != st.path {
        return Ok(Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Full::new(Bytes::new()))
            .unwrap());
    }

    // Secret validation via constant-time comparison to prevent timing attacks.
    // Both sides are hashed with BLAKE3 to normalise to a fixed 32-byte length
    // before the ct_eq call — prevents the length oracle present in direct
    // ct_eq comparison of differently-lengthed slices.
    if !st.secret.is_empty() {
        let provided = headers
            .get("x-webhook-secret")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let expected_hash = blake3::hash(st.secret.as_bytes());
        let provided_hash = blake3::hash(provided.as_bytes());
        if expected_hash.as_bytes().ct_eq(provided_hash.as_bytes()).unwrap_u8() != 1 {
            return Ok(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Full::new(Bytes::new()))
                .unwrap());
        }
    }

    // Valid request — parse body and signal the executor.
    let body_value: Value =
        serde_json::from_str(&body_str).unwrap_or(Value::String(body_str));

    let result = json!({
        "body":    body_value,
        "headers": headers,
        "method":  req_method,
        "path":    req_path,
    });

    if let Some(s) = st.tx.take() {
        let _ = s.send(Ok(result));
    }

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-length", "2")
        .body(Full::new(Bytes::from_static(b"OK")))
        .unwrap())
}
