use async_trait::async_trait;
use http_body_util::{BodyExt, Full, Limited, LengthLimitError};
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
use once_cell::sync::Lazy;
use dashmap::DashSet;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

/// Global registry of ports currently held by active Webhook nodes.
/// Prevents two concurrent workflows from binding the same port and producing
/// an opaque `BIND_ERR`. The entry is inserted before `bind()` and removed on
/// all exit paths via `scopeguard::guard`.
static ACTIVE_PORTS: Lazy<DashSet<u16>> = Lazy::new(DashSet::new);

/// Reserved `ExecutionContext::variables` key the scheduler daemon
/// (`scheduler::runner::fire_once_with_vars`, called from
/// `TriggerKind::Webhook`) uses to hand this node an already-accepted
/// request, instead of letting it bind the port a second time. The daemon's
/// own listener is held for the life of the running job — a second bind on
/// the same port always fails. Namespaced with `__aerini_` so it cannot
/// collide with a workflow author's own `Set Variable` key.
///
/// Value shape: `{"node_id": string, "payload": {body,headers,method,path}}`.
/// `node_id` identifies which Webhook node this came from — `execute()` only
/// short-circuits when it matches its own `input.node_id`, so an unrelated
/// second Webhook node elsewhere in the same workflow still binds and waits
/// normally. Matched by node_id rather than port/path because a user-supplied
/// `port_override` (see `Scheduler::start_job`) can make the actually-bound
/// port differ from this node's own stored config.
///
/// Deliberately left in `variables` for the rest of the run (not cleared
/// after use): a `Get Variable` node reading this exact reserved key back is
/// not a leak, it's the same payload the trigger node already returned.
pub const WEBHOOK_TRIGGER_PAYLOAD_KEY: &str = "__aerini_webhook_payload";

pub struct WebhookNode;

// Shared across the accept loop and each hyper service invocation.
struct HandlerState {
    path:               String,
    method:             String,
    secret:             String,
    validate_timestamp: bool,
    // Taken on the first valid request; None afterwards signals the loop to exit.
    tx: Option<oneshot::Sender<Result<Value, String>>>,
}

#[async_trait]
impl Node for WebhookNode {
    fn type_id(&self) -> &'static str { "webhook" }
    fn display_name(&self) -> &'static str { "Webhook" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Start a workflow when an HTTP request arrives. Outputs the request body, headers, method, and path." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "port":         { "type": "number", "description": "Port to listen on (default 3456). In server/API mode, external traffic cannot reach 127.0.0.1 directly — a reverse proxy forwarding to this port is required." },
                "path":         { "type": "string", "description": "URL path (default /webhook)" },
                "method":       { "type": "string", "enum": ["GET","POST","PUT","ANY"] },
                "secret":       {
                    "type": "string",
                    "description": concat!(
                        "Optional shared secret validated via x-webhook-secret header. ",
                        "This verifies the caller knows the secret but does NOT sign the request body — ",
                        "captured requests can be replayed verbatim. ",
                        "For integrations that send HMAC-SHA256 body signatures (Stripe, GitHub, etc.), ",
                        "verify the platform's native signature header (e.g. x-hub-signature-256) ",
                        "in a Code node immediately downstream rather than relying on this field alone."
                    )
                },
                "timeout_secs":      { "type": "number", "description": "Wait timeout (default 60)" },
                "validate_timestamp": {
                    "type": "boolean",
                    "description": concat!(
                        "When true, requires callers to include an `x-webhook-timestamp` header containing a Unix ",
                        "timestamp (seconds). Requests older than 5 minutes are rejected, which reduces but does ",
                        "not eliminate the replay window — the timestamp is not cryptographically bound to the ",
                        "request body, so an attacker can replay with a captured secret and a fresh timestamp. ",
                        "For body integrity, verify a platform HMAC header (e.g. Stripe-Signature, ",
                        "X-Hub-Signature-256) in a downstream Code node. ",
                        "Default: false. Set to true to require timestamp headers (not compatible with GitHub, Stripe, or other platforms that do not send x-webhook-timestamp)."
                    )
                }
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
                PortDefinition { id: "output".to_string(),   label: "Triggered".to_string(), position: PortPosition::Right, port_type: None },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(),     position: PortPosition::Right, port_type: None },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let port = input.input["port"].as_u64().unwrap_or(3456) as u16;
        let path = input.input["path"].as_str().unwrap_or("/webhook").to_string();

        // Scheduler-driven run (TriggerKind::Webhook): the daemon's accept loop
        // already validated and parsed a real request for the entry trigger,
        // and is still holding that listener. Return the payload directly —
        // binding again here would always fail with PORT_IN_USE/address-in-use.
        // Mirrors ManualTriggerNode's mock_payload short-circuit and
        // ScheduleNode's "daemon owns timing" convention.
        //
        // Matched on node_id, not port/path: a workflow can contain a second,
        // unrelated Webhook node mid-graph waiting on its own different port.
        // The reserved key stays in `variables` for the rest of the run (see
        // WEBHOOK_TRIGGER_PAYLOAD_KEY doc), so without an identity check that
        // second node would wrongly short-circuit on the entry trigger's
        // payload instead of binding and waiting for its own real request.
        // node_id is also immune to `port_override` (Scheduler::start_job)
        // making the actually-bound port differ from this node's own config.
        if let Some(reserved) = input.context.variables.get(WEBHOOK_TRIGGER_PAYLOAD_KEY) {
            let node_matches = reserved.get("node_id").and_then(Value::as_str) == Some(input.node_id.as_str());
            if node_matches {
                let payload = reserved.get("payload").cloned().unwrap_or(Value::Null);
                return NodeOutput::success_with_logs(
                    payload,
                    vec!["Webhook payload received (scheduler-held listener)".to_string()],
                );
            }
        }

        if port < 1024 {
            return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_PORT",
                "Webhook port must be 1024 or higher (privileged ports require root)",
            ));
        }

        // Claim the port in the global registry before binding.
        // If the port is already held by another running Webhook node in this process,
        // return a clear error instead of producing a confusing BIND_ERR from the OS.
        if !ACTIVE_PORTS.insert(port) {
            return NodeOutput::failure(NodeError::unrecoverable(
                "PORT_IN_USE",
                format!(
                    "Port {} is already held by another running Webhook node. \
                     Choose a different port or wait for that workflow to finish.",
                    port
                ),
            ));
        }
        // Release the port registration on all exit paths (normal return, error, or panic).
        let _port_guard = scopeguard::guard((), |_| { ACTIVE_PORTS.remove(&port); });
        let method             = input.input["method"].as_str().unwrap_or("ANY").to_uppercase();
        let secret             = input.input["secret"].as_str().unwrap_or("").to_string();
        let validate_timestamp = input.input["validate_timestamp"].as_bool().unwrap_or(false);
        let timeout_secs       = input.input["timeout_secs"].as_u64().unwrap_or(60);

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
            validate_timestamp,
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

    // Cap body at 1 MB *before* collecting — Limited wraps the incoming stream
    // and returns LengthLimitError as soon as the limit is exceeded, preventing
    // full memory allocation before the size check fires.
    const MAX_BODY_BYTES: usize = 1_000_000;
    let body_bytes = match Limited::new(req.into_body(), MAX_BODY_BYTES).collect().await {
        Err(e) if e.downcast_ref::<LengthLimitError>().is_some() => {
            return Ok(Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .body(Full::new(Bytes::new()))
                .expect("static response builder parameters are infallible"));
        }
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::new()))
                .expect("static response builder parameters are infallible"));
        }
        Ok(b) => b.to_bytes(),
    };

    let body_str = String::from_utf8_lossy(&body_bytes).to_string();

    let mut st = state.lock().await;

    // Method mismatch: 405. Keep listening — caller used the wrong method.
    if st.method != "ANY" && req_method != st.method {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Full::new(Bytes::new()))
            .expect("static response builder parameters are infallible"));
    }

    // Path mismatch: 401 (not 404). Returning 404 would confirm the port is
    // active and reveal that the guessed path did not match.
    if req_path != st.path {
        return Ok(Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Full::new(Bytes::new()))
            .expect("static response builder parameters are infallible"));
    }

    // Secret validation via constant-time comparison to prevent timing attacks.
    // Both sides are hashed with BLAKE3 to normalise to a fixed 32-byte length
    // before the ct_eq call — prevents the length oracle present in direct
    // ct_eq comparison of differently-lengthed slices.
    //
    // SECURITY NOTE — replay attacks: this check validates only that the caller
    // knows the secret. It does NOT cryptographically bind the secret to the
    // request body. A captured valid request can be replayed in full. For
    // integrations that send HMAC-SHA256 body signatures (Stripe: Stripe-Signature,
    // GitHub: X-Hub-Signature-256), add a downstream Code node that verifies the
    // platform's native signature header against the raw body bytes.
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
                .expect("static response builder parameters are infallible"));
        }
    }

    // Timestamp replay-window check.
    // When validate_timestamp is true, callers must include x-webhook-timestamp
    // as a Unix epoch (seconds). Requests outside a ±5-minute window are rejected.
    // This REDUCES but does not eliminate the replay window — the timestamp is not
    // cryptographically bound to the body. An attacker with a captured secret can
    // replay by sending a fresh timestamp with any body. For body integrity, verify
    // a platform HMAC header in a downstream Code node.
    if st.validate_timestamp {
        const WINDOW_SECS: i64 = 300; // 5 minutes
        let provided_ts = headers
            .get("x-webhook-timestamp")
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse::<i64>().ok());
        match provided_ts {
            None => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from_static(b"x-webhook-timestamp required")))
                    .expect("static response builder parameters are infallible"));
            }
            Some(ts) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                if (now - ts).abs() > WINDOW_SECS {
                    return Ok(Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .body(Full::new(Bytes::from_static(b"Timestamp too old or too far in future")))
                        .expect("static response builder parameters are infallible"));
                }
            }
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
        .expect("static response builder parameters are infallible"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_input(node_id: &str, port: u64, path: &str, variables: HashMap<String, Value>) -> NodeInput {
        NodeInput {
            node_id:      node_id.to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({ "port": port, "path": path }),
            context: ExecutionContext {
                variables,
                node_outputs: Arc::new(HashMap::new()),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        }
    }

    /// The bug this patch fixes: a scheduler-driven webhook run used to make
    /// `WebhookNode::execute()` try to bind the port a second time while the
    /// daemon's own listener was still holding it. This asserts the reserved
    /// context-variable short-circuit fires instead — completes immediately
    /// with the handed-off payload, no bind attempt at all.
    #[tokio::test]
    async fn scheduler_handoff_short_circuits_without_binding() {
        let mut vars = HashMap::new();
        vars.insert(
            WEBHOOK_TRIGGER_PAYLOAD_KEY.to_string(),
            json!({
                "node_id": "trigger1",
                "payload": { "body": {"hello": "world"}, "headers": {}, "method": "POST", "path": "/hook" }
            }),
        );
        // Port 1 is a privileged port `execute()` would reject outright if it
        // ever reached the bind path — picked deliberately so the test fails
        // loudly (INVALID_PORT, not a hang/timeout) if the short-circuit
        // regresses, instead of trying to actually bind a port in CI.
        let input = make_input("trigger1", 1, "/hook", vars);
        let out = WebhookNode.execute(input).await;
        assert!(out.success, "expected short-circuit success, got: {:?}", out.error);
        let o = out.output.unwrap();
        assert_eq!(o["body"], json!({"hello": "world"}));
        assert_eq!(o["method"], "POST");
    }

    /// A second, unrelated Webhook node elsewhere in the same workflow (different
    /// node_id) must NOT short-circuit on the entry trigger's payload — it has
    /// its own port/path to wait on. Asserts it falls through to the real bind
    /// path instead (proven here by INVALID_PORT, since port 1 is privileged —
    /// confirms it did NOT take the short-circuit return).
    #[tokio::test]
    async fn unrelated_node_id_does_not_short_circuit() {
        let mut vars = HashMap::new();
        vars.insert(
            WEBHOOK_TRIGGER_PAYLOAD_KEY.to_string(),
            json!({
                "node_id": "trigger1",
                "payload": { "body": "irrelevant", "headers": {}, "method": "POST", "path": "/hook" }
            }),
        );
        let input = make_input("a_different_node", 1, "/other-hook", vars);
        let out = WebhookNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_PORT");
    }

    /// No reserved key at all (ad-hoc, non-scheduler invocation) — must behave
    /// exactly as before this patch: falls through to the normal bind path.
    #[tokio::test]
    async fn no_reserved_key_falls_through_to_normal_path() {
        let input = make_input("n1", 1, "/hook", HashMap::new());
        let out = WebhookNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_PORT");
    }

    /// A port already held by another Webhook node in ACTIVE_PORTS must return
    /// PORT_IN_USE immediately — no bind attempt, no hang, no panic.
    ///
    /// Tests the global port-registry guard added to prevent opaque BIND_ERR
    /// messages when two concurrent workflows share the same port number.
    ///
    /// NOTE — handle_request secret/signature validation:
    /// `handle_request` takes `hyper::Request<Incoming>`, which can only be
    /// obtained from hyper's server accept loop and cannot be constructed in a
    /// unit test without binding a real port. Testing the BLAKE3 constant-time
    /// secret check, path/method guards, and timestamp replay-window logic
    /// therefore requires an integration test that spins up a real local
    /// listener. That is deferred to a separate integration test suite.
    ///
    /// PLAN DISCREPANCY (flagged, not fixed):
    /// PLAN §P20 lists HMAC-SHA256/SHA1 tests. The actual code uses BLAKE3 for
    /// constant-time shared-secret comparison (`x-webhook-secret` header), not
    /// HMAC. The plan was written before implementation. Tests above and below
    /// cover the real implementation.
    #[tokio::test]
    async fn port_already_held_in_registry_returns_port_in_use() {
        // Port 59_901 — high ephemeral range, unlikely to be held by any real listener.
        // We insert it directly into the global registry to simulate a concurrent
        // Webhook node having claimed it, without needing to actually bind the socket.
        const TEST_PORT: u16 = 59_901;
        ACTIVE_PORTS.insert(TEST_PORT);
        let input = make_input("n1", TEST_PORT as u64, "/hook", HashMap::new());
        let out = WebhookNode.execute(input).await;
        // Manual cleanup: execute() returns early on PORT_IN_USE before the
        // scopeguard that would normally remove the entry is created.
        ACTIVE_PORTS.remove(&TEST_PORT);
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "PORT_IN_USE");
    }
}
