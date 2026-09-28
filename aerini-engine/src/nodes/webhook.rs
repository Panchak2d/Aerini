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
use base64::Engine as _;
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;
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

/// Prefix marking a signed, time-limited trigger token rather than the raw
/// static secret. A caller who already holds the secret (typically the
/// workflow author's own backend) can mint one of these with
/// [`mint_signed_token`] and hand only the token — not the secret itself —
/// to an untrusted context such as a browser. Unlike the raw secret, a
/// captured token self-invalidates at its embedded expiry instead of
/// granting the holder standing access forever. See
/// docs/guide/widget-embedding.md, "Recommended for production" section.
const SIGNED_TOKEN_PREFIX: &str = "awh1.";

/// Hard ceiling on a minted token's lifetime, enforced at verification time
/// regardless of what `ttl_secs` a caller passes to [`mint_signed_token`] —
/// bounds the damage of a minting bug or an overly generous caller-chosen TTL.
const SIGNED_TOKEN_MAX_LIFETIME_SECS: i64 = 24 * 60 * 60;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// MAC over `path` and `expiry`, keyed by the webhook's own secret. `path`
/// binds a token to one specific Webhook node's URL — a token minted for one
/// node's secret+path can't be replayed against a different node that
/// happens to share the same secret string.
fn signed_token_mac(secret: &str, path: &str, expiry: i64) -> blake3::Hash {
    let key: [u8; 32] = *blake3::hash(secret.as_bytes()).as_bytes();
    blake3::keyed_hash(&key, format!("{path}|{expiry}").as_bytes())
}

/// Mint a signed trigger token for a webhook configured with `secret` at
/// `path`, valid for `ttl_secs` (clamped to `[1, SIGNED_TOKEN_MAX_LIFETIME_SECS]`).
/// Called from `routes::widget`'s token-minting endpoint.
pub fn mint_signed_token(secret: &str, path: &str, ttl_secs: i64) -> String {
    let ttl    = ttl_secs.clamp(1, SIGNED_TOKEN_MAX_LIFETIME_SECS);
    let expiry = unix_now() + ttl;
    let mac    = signed_token_mac(secret, path, expiry);
    let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.as_bytes());
    format!("{SIGNED_TOKEN_PREFIX}{expiry}.{sig_b64}")
}

/// Validates a token of the form `awh1.<expiry_unix>.<sig>` against `secret`/`path`.
/// Constant-time signature comparison, same rationale as the raw-secret path below.
fn validate_signed_token(secret: &str, path: &str, rest: &str) -> bool {
    let Some((expiry_str, sig_b64)) = rest.split_once('.') else { return false };
    let Ok(expiry) = expiry_str.parse::<i64>() else { return false };
    let now = unix_now();
    // Rejects both expired tokens and ones whose expiry sits further out than
    // the mint-time ceiling could ever have produced (a forged huge expiry
    // would also fail the signature check below, but reject it up front too).
    if expiry <= now || expiry - now > SIGNED_TOKEN_MAX_LIFETIME_SECS {
        return false;
    }
    let Ok(provided_sig) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig_b64) else { return false };
    if provided_sig.len() != 32 {
        return false;
    }
    let expected = signed_token_mac(secret, path, expiry);
    expected.as_bytes().ct_eq(provided_sig.as_slice()).unwrap_u8() == 1
}

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
                "dedup_window_secs": {
                    "type": "number",
                    "description": concat!(
                        "When set above 0, a request whose body was already seen within this many ",
                        "seconds does not re-run the workflow (the caller still gets a 200 OK — this ",
                        "only suppresses the re-run). Providers that retry on timeout or a non-2xx ",
                        "response (Stripe, GitHub, ...) resend the same event body byte-for-byte, so ",
                        "this catches that case. Only applies to requests with a non-empty body — ",
                        "GET requests and empty-body POSTs are never deduped. Default: 0 (disabled). ",
                        "Only takes effect for a workflow set to run in the background (Active in the ",
                        "scheduler) — an ad-hoc 'Run' press always executes once per press."
                    )
                },
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
        let timeout_secs       = clamp_timeout_secs(input.input["timeout_secs"].as_u64().unwrap_or(60));

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

        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(timeout_secs);

        // Owned in place — not tokio::spawn'ed — so `listener` shares this future's
        // drop timing with `_port_guard` above. If this future is dropped (cancellation
        // at the executor level), both the registry entry and the real OS socket release
        // together instead of the socket staying bound in an orphaned detached task.
        // Mirrors oauth_listener.rs's listen_for_callback/accept_one_callback pair.
        run_accept_loop(&listener, Arc::clone(&state), deadline, input.cancel_token.clone()).await;

        match rx.await {
            Err(_) => NodeOutput::failure(NodeError::unrecoverable("CHANNEL_ERR", "Internal error")),
            Ok(Err(e)) if e == "TIMEOUT" => NodeOutput::failure(NodeError::recoverable(
                "TIMEOUT",
                format!("No request in {}s", timeout_secs),
            )),
            Ok(Err(e)) if e == "CANCELLED" => NodeOutput::failure(NodeError::unrecoverable(
                crate::executor::CANCEL_ERROR_CODE,
                "Run cancelled by user",
            )),
            Ok(Err(e)) => NodeOutput::failure(NodeError::unrecoverable("WEBHOOK_ERR", e)),
            Ok(Ok(data)) => NodeOutput::success_with_logs(
                data,
                vec![format!("Webhook received on :{}{}", port_for_log, path_for_log)],
            ),
        }
    }
}

/// Clamps a user-supplied `timeout_secs` to the same 1-3600s bounds
/// `wait_node.rs`'s condition-mode timeout uses. Scheduler-driven runs never
/// reach this — they return via the reserved-payload short-circuit above —
/// so this only bounds the ad-hoc/manual bind-and-wait path.
fn clamp_timeout_secs(raw: u64) -> u64 {
    raw.clamp(1, 3600)
}

/// Drives the accept loop for one Webhook node invocation, owned in place by
/// the caller rather than `tokio::spawn`'ed — see the call site's comment.
///
/// Races each wait for the next connection against `cancel_token` (when set)
/// so a cancelled run stops promptly instead of running until `timeout_secs`
/// elapses, per `Node::execute`'s cancellation contract (node.rs). An
/// already-in-flight connection (accepted, being served) is not raced against
/// cancellation — it is already bounded by its own per-connection deadline
/// below, and this is the wrapper-level fix; per-connection handling stays
/// as-is.
async fn run_accept_loop(
    listener:     &tokio::net::TcpListener,
    state:        Arc<Mutex<HandlerState>>,
    deadline:     std::time::Instant,
    cancel_token: Option<CancellationToken>,
) {
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            let mut st = state.lock().await;
            if let Some(s) = st.tx.take() {
                let _ = s.send(Err("TIMEOUT".into()));
            }
            return;
        }

        let accept_result = if let Some(ref token) = cancel_token {
            tokio::select! {
                r = tokio::time::timeout(remaining, listener.accept()) => r,
                _ = token.cancelled() => {
                    let mut st = state.lock().await;
                    if let Some(s) = st.tx.take() {
                        let _ = s.send(Err("CANCELLED".into()));
                    }
                    return;
                }
            }
        } else {
            tokio::time::timeout(remaining, listener.accept()).await
        };

        let (stream, _) = match accept_result {
            Err(_) => {
                let mut st = state.lock().await;
                if let Some(s) = st.tx.take() {
                    let _ = s.send(Err("TIMEOUT".into()));
                }
                return;
            }
            Ok(Err(e)) => {
                let mut st = state.lock().await;
                if let Some(s) = st.tx.take() {
                    let _ = s.send(Err(e.to_string()));
                }
                return;
            }
            Ok(Ok(s)) => s,
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
}

/// Every response this handler returns — success, preflight, or error — must
/// carry these, or a browser-based caller's `fetch()` (the desktop Chat
/// panel's own webview, or any other in-browser integration) is rejected by
/// CORS before the caller ever sees a status code, indistinguishable from
/// the port not being reachable at all. Wildcard origin is deliberate: this
/// endpoint accepts arbitrary third-party callers (Stripe, GitHub, ...), not
/// just the app's own UI, so there is no single origin to allow instead.
/// Takes the already-built `Response` rather than a `Builder` so the header
/// values can be inferred from `HeaderMap::insert`'s own signature instead
/// of naming hyper's re-exported `http` builder type directly.
fn with_cors(mut resp: Response<Full<Bytes>>) -> Response<Full<Bytes>> {
    let headers = resp.headers_mut();
    headers.insert("access-control-allow-origin", "*".parse().expect("static header value"));
    headers.insert("access-control-allow-methods", "GET, POST, PUT, OPTIONS".parse().expect("static header value"));
    headers.insert("access-control-allow-headers", "content-type, x-webhook-secret, x-webhook-timestamp".parse().expect("static header value"));
    resp
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

    // A browser preflights any cross-origin request whose Content-Type isn't
    // form-encoded (the Chat panel sends application/json) with an OPTIONS
    // request before it will send the real one. Answered here, ahead of the
    // method/path/secret checks below, and without touching `state.tx` —
    // this is not a trigger attempt, so it must not consume the wait or
    // count as the request the caller is waiting for.
    if req_method == "OPTIONS" {
        return Ok(with_cors(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Full::new(Bytes::new()))
            .expect("static response builder parameters are infallible")));
    }

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
            return Ok(with_cors(Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .body(Full::new(Bytes::new()))
                .expect("static response builder parameters are infallible")));
        }
        Err(_) => {
            return Ok(with_cors(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::new()))
                .expect("static response builder parameters are infallible")));
        }
        Ok(b) => b.to_bytes(),
    };

    let body_str = String::from_utf8_lossy(&body_bytes).to_string();

    let mut st = state.lock().await;

    // Method mismatch: 405. Keep listening — caller used the wrong method.
    if st.method != "ANY" && req_method != st.method {
        return Ok(with_cors(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Full::new(Bytes::new()))
            .expect("static response builder parameters are infallible")));
    }

    // Path mismatch: 401 (not 404). Returning 404 would confirm the port is
    // active and reveal that the guessed path did not match.
    if req_path != st.path {
        return Ok(with_cors(Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Full::new(Bytes::new()))
            .expect("static response builder parameters are infallible")));
    }

    // Secret validation. Accepts either the raw static secret (constant-time
    // compare, both sides BLAKE3-hashed first to normalise length and avoid
    // a length oracle), or a signed short-lived token minted from that same
    // secret (see `mint_signed_token` / `SIGNED_TOKEN_PREFIX`) — the latter
    // lets a caller hand out a credential that self-expires instead of the
    // permanent secret itself.
    //
    // SECURITY NOTE — replay attacks: neither form cryptographically binds
    // the credential to the request body. A captured valid request can be
    // replayed in full until the credential (secret, or token expiry) is no
    // longer valid. For integrations that send HMAC-SHA256 body signatures
    // (Stripe: Stripe-Signature, GitHub: X-Hub-Signature-256), add a
    // downstream Code node that verifies the platform's native signature
    // header against the raw body bytes.
    if !st.secret.is_empty() {
        let provided = headers
            .get("x-webhook-secret")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let ok = if let Some(rest) = provided.strip_prefix(SIGNED_TOKEN_PREFIX) {
            // Structurally a token attempt — but if it doesn't validate as
            // one, still fall back to plain equality: a secret whose literal
            // value happens to start with "awh1." must keep working exactly
            // as it always has, not silently start rejecting valid callers.
            validate_signed_token(&st.secret, &st.path, rest) || {
                let expected_hash = blake3::hash(st.secret.as_bytes());
                let provided_hash = blake3::hash(provided.as_bytes());
                expected_hash.as_bytes().ct_eq(provided_hash.as_bytes()).unwrap_u8() == 1
            }
        } else {
            let expected_hash = blake3::hash(st.secret.as_bytes());
            let provided_hash = blake3::hash(provided.as_bytes());
            expected_hash.as_bytes().ct_eq(provided_hash.as_bytes()).unwrap_u8() == 1
        };
        if !ok {
            return Ok(with_cors(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Full::new(Bytes::new()))
                .expect("static response builder parameters are infallible")));
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
                return Ok(with_cors(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from_static(b"x-webhook-timestamp required")))
                    .expect("static response builder parameters are infallible")));
            }
            Some(ts) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                if (now - ts).abs() > WINDOW_SECS {
                    return Ok(with_cors(Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .body(Full::new(Bytes::from_static(b"Timestamp too old or too far in future")))
                        .expect("static response builder parameters are infallible")));
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

    Ok(with_cors(Response::builder()
        .status(StatusCode::OK)
        .header("content-length", "2")
        .body(Full::new(Bytes::from_static(b"OK")))
        .expect("static response builder parameters are infallible")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_input(node_id: &str, port: u64, path: &str, variables: HashMap<String, Value>) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
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

    /// A scheduler-driven webhook run must not let `WebhookNode::execute()` try
    /// to bind the port a second time while the daemon's own listener is still
    /// holding it. This asserts the reserved context-variable short-circuit
    /// fires instead — completes immediately with the handed-off payload, no
    /// bind attempt at all.
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
    /// Design-spec discrepancy (flagged, not fixed): the original design called
    /// for HMAC-SHA256/SHA1 tests, but the actual code uses BLAKE3 for
    /// constant-time shared-secret comparison (`x-webhook-secret` header), not
    /// HMAC — the spec predates the implementation. Tests above and below
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

    /// A `timeout_secs` far above the cap must clamp to the 3600s ceiling —
    /// same convention as wait_node.rs's condition-mode timeout.
    #[test]
    fn timeout_secs_clamped_to_3600_ceiling() {
        assert_eq!(clamp_timeout_secs(999_999), 3600);
    }

    /// A `timeout_secs` already within bounds is unaffected by the clamp.
    #[test]
    fn timeout_secs_within_bounds_unaffected() {
        assert_eq!(clamp_timeout_secs(45), 45);
    }

    /// Cancelling a run while `execute()` is waiting for a connection must
    /// return promptly with the shared cancellation error code, instead of
    /// running until `timeout_secs` elapses — and must release the real OS
    /// socket, not just the port registry entry, so a bind on the same port
    /// immediately afterward succeeds instead of hitting a raw address-in-use
    /// error. This is the behavior the detached tokio::spawn accept loop
    /// broke: the registry entry released on this future's drop while the
    /// orphaned spawned task kept the socket bound.
    #[tokio::test]
    async fn cancel_during_wait_returns_cancelled_and_releases_port() {
        // Real free ephemeral port: bind then drop, same technique
        // oauth_listener.rs's own bind_oauth_listener fallback uses. Avoids a
        // hardcoded port literal colliding with the other tests in this file.
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test setup: must be able to bind an OS-assigned port");
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let token = CancellationToken::new();
        token.cancel(); // pre-cancelled: deterministic, no sleep-then-cancel race

        let mut input = make_input("n1", port as u64, "/hook", HashMap::new());
        input.input["timeout_secs"] = json!(60);
        input.cancel_token = Some(token);

        let out = tokio::time::timeout(std::time::Duration::from_secs(5), WebhookNode.execute(input))
            .await
            .expect("execute() did not return promptly after cancellation");

        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, crate::executor::CANCEL_ERROR_CODE);

        tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("port must be free immediately after a cancelled run");
    }

    /// A token minted by `mint_signed_token` must validate against the same
    /// secret/path it was minted for. Unlike the header-integration secret
    /// check above, these are plain functions with no `hyper::Request`
    /// dependency, so they're directly unit-testable.
    #[test]
    fn signed_token_roundtrip_succeeds() {
        let token = mint_signed_token("s3cr3t", "/hook", 60);
        let rest = token.strip_prefix(SIGNED_TOKEN_PREFIX).expect("token must carry the expected prefix");
        assert!(validate_signed_token("s3cr3t", "/hook", rest));
    }

    /// A token minted for one secret must not validate against a different
    /// one — the whole point of signing rather than embedding the secret
    /// itself in the token.
    #[test]
    fn signed_token_rejects_wrong_secret() {
        let token = mint_signed_token("s3cr3t", "/hook", 60);
        let rest = token.strip_prefix(SIGNED_TOKEN_PREFIX).unwrap();
        assert!(!validate_signed_token("wrong-secret", "/hook", rest));
    }

    /// An expired token (ttl clamped to 1s, then checked after it's elapsed)
    /// must be rejected even though the signature itself is valid — this is
    /// the entire reason to prefer a token over the raw secret.
    #[test]
    fn signed_token_rejects_after_expiry() {
        let token = mint_signed_token("s3cr3t", "/hook", 1);
        let rest = token.strip_prefix(SIGNED_TOKEN_PREFIX).unwrap();
        let (expiry_str, _) = rest.split_once('.').unwrap();
        let expiry: i64 = expiry_str.parse().unwrap();
        // Directly re-derive with a past expiry rather than sleeping in a
        // unit test: same code path (`validate_signed_token`'s expiry
        // check), deterministic, no real time dependency.
        let past = expiry - 120;
        let rest_expired = signed_token_mac("s3cr3t", "/hook", past);
        let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rest_expired.as_bytes());
        assert!(!validate_signed_token("s3cr3t", "/hook", &format!("{past}.{sig_b64}")));
    }
}
