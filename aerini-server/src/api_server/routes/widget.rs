//! Embeddable chat widget: a static JS asset plus a public (no-Bearer-auth)
//! trigger gateway that relays a browser POST to the target workflow's
//! already-running Webhook listener.
//!
//! # Why this gateway exists instead of POSTing the browser straight to the
//! webhook port
//! Not CORS: the listener this relays to for an active background job
//! (`scheduler/runner.rs`'s `run_job_loop`) already answers `OPTIONS`
//! preflights and sets a wildcard `Access-Control-*` response of its own
//! (`CORS_HEADER_LINES`). The actual reason is that listener only ever binds
//! `127.0.0.1` — unreachable from a visitor's browser on another machine no
//! matter what headers it sends. `aerini-server` itself can bind publicly
//! (`--bind 0.0.0.0`), so this route runs on that public-facing side and
//! forwards the validated request to the trigger listener as a loopback
//! call, the same way any other reverse-proxy-in-front-of-a-loopback-service
//! setup works.
//!
//! # Auth model
//! This route is deliberately registered outside `auth_middleware` (no
//! Bearer token required) — a widget embedded on a third-party page cannot
//! hold a server admin/write API token without exposing it to every visitor.
//! Authorization is instead the same shared webhook secret the workflow
//! author already configured on the Webhook node. The secret is forwarded
//! verbatim as `x-webhook-secret`; this handler never compares it itself —
//! `webhook.rs`'s existing constant-time check remains the single source of
//! truth for whether a request is accepted.
//!
//! # Effective port, not configured port
//! The target port is read from `ScheduledJobRow.trigger_kind` (via
//! `SchedulerDb::scheduler_get`), not from the workflow JSON's own Webhook
//! node config. `SchedulerDaemon::start_job` persists the *effective* port
//! (after an optional `port_override`) into that row
//! (`aerini-engine/src/scheduler/mod.rs::start_job`), so this is the only
//! place guaranteed to reflect the port actually bound right now.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use aerini_engine::scheduler::{SchedulerDb, TriggerKind};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use super::state::ApiState;

/// `aerini-server/src/static/aerini-widget.js`, embedded at compile time —
/// matches the `include_str!` convention `status_server.rs` uses for
/// `STATUS_PAGE_CSS`/`STATUS_PAGE_SCRIPT` (those are inlined into an HTML
/// page; this is a standalone JS response, so the serving code below is new
/// rather than a verbatim copy of that pattern).
const AERINI_WIDGET_JS: &str = include_str!("../../static/aerini-widget.js");

/// GET /aerini-widget.js — unauthenticated static asset. Browsers loading
/// `<script src>` send no Authorization header, so this must sit in the
/// public tier of the router alongside `/api/health`, not inside `protected`.
pub async fn serve_widget_js() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        AERINI_WIDGET_JS,
    )
}

/// Dedicated client for the loopback relay, mirroring the
/// `CLI_HTTP_CLIENT` pattern in `main.rs` (separate `OnceLock` rather than
/// reusing that one — different timeout requirements, and it lives in a
/// different module with no shared state to justify coupling them).
///
/// 15s timeout: generous relative to the actual wait. The target listener
/// (`webhook.rs` / `scheduler/runner.rs`) replies as soon as it accepts and
/// validates the request — before the workflow body runs — so this is not
/// waiting on full workflow execution, only on the local loopback hop.
fn relay_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("Failed to build widget relay HTTP client")
    })
}

#[derive(Deserialize)]
pub struct WidgetTriggerBody {
    /// The Webhook node's shared secret, as configured by the workflow
    /// author and passed through by the widget's `data-secret` attribute —
    /// or a short-lived signed token from `mint_widget_token` (see there for
    /// when to prefer that over embedding the raw secret). Forwarded
    /// verbatim as `x-webhook-secret`; never compared in this handler (see
    /// module doc — single source of truth stays in `webhook.rs`).
    pub secret: String,
    /// Forwarded verbatim as the loopback request's JSON body. Shape is
    /// whatever the target workflow's Webhook node expects — the chat
    /// widget sends `{"message": ..., "session_id": ...}`, mirroring the
    /// desktop Chat Panel (`src/panels/ChatPanel.ts::handleSend`).
    #[serde(default)]
    pub body: Value,
}

/// Shared by `trigger_widget` and `mint_widget_token`: loads the workflow's
/// live scheduler row and extracts the Webhook trigger's effective
/// `(port, path, secret)`, or a boxed `Err(response)` the caller should
/// dereference (`*resp`) and return immediately as-is.
async fn load_webhook_trigger(
    s: &ApiState,
    workflow_id: &str,
) -> Result<(u16, String, String), Box<axum::response::Response>> {
    let row = {
        let db = Arc::clone(&s.db);
        let wf_id = workflow_id.to_string();
        match tokio::task::spawn_blocking(move || db.scheduler_get(&wf_id)).await {
            Ok(Ok(Some(r))) => r,
            Ok(Ok(None)) => {
                return Err(Box::new(
                    (
                        StatusCode::NOT_FOUND,
                        Json(json!({"error": "workflow is not scheduled — start it first"})),
                    )
                        .into_response(),
                ));
            }
            Ok(Err(e)) => {
                return Err(Box::new(
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
                ));
            }
            Err(e) => {
                return Err(Box::new(
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": e.to_string()})),
                    )
                        .into_response(),
                ));
            }
        }
    };

    if row.status != "active" {
        return Err(Box::new(
            (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": format!("workflow is not running (status: {})", row.status)
                })),
            )
                .into_response(),
        ));
    }

    let trigger: TriggerKind = match serde_json::from_str(&row.trigger_kind) {
        Ok(t) => t,
        Err(e) => {
            return Err(Box::new(
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("corrupt trigger_kind in scheduler row: {}", e)
                    })),
                )
                    .into_response(),
            ));
        }
    };

    match trigger {
        TriggerKind::Webhook { port, path, secret, .. } => Ok((port, path, secret)),
        other => Err(Box::new(
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!(
                        "workflow's trigger is '{}', not Webhook — the embeddable widget only works with a Webhook-triggered workflow",
                        other.label()
                    )
                })),
            )
                .into_response(),
        )),
    }
}

/// `--allow-shell`/`--allow-code`/`--allow-database` (`main.rs`) are process-wide:
/// turning one on for an internal, admin-only workflow also unlocks that node
/// type for every other workflow the same server process runs. Every other
/// trigger path requires a server Bearer token, so that's an accepted
/// tradeoff for an operator who already trusts their token holders. This
/// route doesn't — it's reachable by any anonymous visitor of a page that
/// embeds the widget — so it applies its own gate on top, independent of
/// the flags: a dangerous node blocks the public relay regardless of
/// whether the flag enabling it was meant for this workflow or a different
/// one. Returns `None` when `nodes` contains none of
/// `aerini_engine::nodes::DANGEROUS_NODE_TYPE_IDS`.
fn dangerous_node_block(nodes: &[aerini_engine::model::WorkflowNode]) -> Option<axum::response::Response> {
    if aerini_engine::nodes::dangerous_node_types_present(nodes).is_empty() {
        return None;
    }
    Some(
        (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "this workflow contains a Shell Command, Code, or Database node — \
                          the public widget relay refuses to trigger it, regardless of the \
                          server's --allow-shell/--allow-code/--allow-database flags"
            })),
        )
            .into_response(),
    )
}

/// POST /api/widget/:workflow_id/trigger
///
/// Looks up the workflow's live scheduler row, confirms it's an active
/// Webhook-triggered job, and relays the request to
/// `127.0.0.1:<effective_port><path>` with the caller-supplied secret. The
/// actual workflow result is NOT returned here — `webhook.rs` only ever
/// acks "OK"; the result arrives later as a `scheduler-status` SSE event on
/// `/api/events` (see `aerini-widget.js`, and the doc comment at the top of
/// `ChatPanel.ts` for the same discrepancy already documented against the
/// original plan).
pub async fn trigger_widget(
    State(s): State<ApiState>,
    Path(workflow_id): Path<String>,
    Json(b): Json<WidgetTriggerBody>,
) -> impl IntoResponse {
    let (port, path, _secret) = match load_webhook_trigger(&s, &workflow_id).await {
        Ok(t) => t,
        Err(resp) => return *resp,
    };

    // Loaded once, ahead of the relay, so the dangerous-node gate can run
    // before anything is forwarded to the workflow's listener — the listener
    // acks (and the scheduler starts the run) as soon as it accepts the
    // request, before the workflow body executes, so checking after relaying
    // would already be too late to stop it. Reused below for output_node_id,
    // avoiding a second workflow load.
    let workflow = {
        let db = Arc::clone(&s.db);
        let wf_id = workflow_id.clone();
        match tokio::task::spawn_blocking(move || db.load(&wf_id)).await {
            Ok(Ok(Some(wf))) => wf,
            Ok(Ok(None)) => {
                // Fail closed: a scheduler row can outlive its workflow
                // definition (e.g. deleted without stopping its job first).
                // The dangerous-node gate below has nothing to check in that
                // case, so refuse rather than relay against an unknown workflow.
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "workflow definition not found"})),
                )
                    .into_response();
            }
            Ok(Err(e)) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e})))
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
                    .into_response();
            }
        }
    };

    if let Some(resp) = dangerous_node_block(&workflow.nodes) {
        return resp;
    }

    let target = format!("http://127.0.0.1:{}{}", port, path);
    let relay_result = relay_client()
        .post(&target)
        // `.as_str()` rather than `&b.secret`: avoids depending on whether
        // `&String` itself satisfies reqwest's `V: TryInto<HeaderValue>`
        // bound (unconfirmed — `&str` definitely does, so this is the
        // zero-ambiguity choice).
        .header("x-webhook-secret", b.secret.as_str())
        .json(&b.body)
        .send()
        .await;

    let upstream = match relay_result {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": format!(
                        "could not reach the workflow's webhook listener — it may have just stopped: {}",
                        e
                    )
                })),
            )
                .into_response();
        }
    };

    let upstream_status = upstream.status();
    if !upstream_status.is_success() {
        let code =
            StatusCode::from_u16(upstream_status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let hint = match upstream_status.as_u16() {
            401 => "the secret did not match the Webhook node's configured secret",
            404 | 405 => {
                "the request path or HTTP method did not match the Webhook node's configuration \
                 (the widget always sends POST — set the Webhook node's method to POST or ANY)"
            }
            413 => "message too large",
            _ => "the webhook listener rejected the request",
        };
        return (code, Json(json!({"error": hint}))).into_response();
    }

    // Computed from the `workflow` already loaded above for the dangerous-node
    // gate — an unauthenticated caller with the wrong secret only reaches this
    // line after a successful upstream relay, so a failed attempt never
    // reveals the workflow's internal node IDs.
    let output_node_id = workflow
        .nodes
        .iter()
        .find(|n| n.node_type_id.as_str() == "output")
        .map(|n| n.id.clone());

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "output_node_id": output_node_id
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct MintTokenBody {
    /// The Webhook node's real, static, configured secret. This must be the
    /// raw secret, never a previously-minted token — minting a token from a
    /// token would defeat the expiry (a caller could keep re-minting to stay
    /// permanently valid). Enforced below by comparing directly against the
    /// scheduler row's stored secret rather than going through `webhook.rs`'s
    /// dual-mode check.
    pub secret: String,
    /// Requested lifetime in seconds. Clamped server-side to
    /// `[1, SIGNED_TOKEN_MAX_LIFETIME_SECS]` (currently 24h) regardless of
    /// what's requested here — see `webhook::mint_signed_token`.
    #[serde(default = "default_ttl_secs")]
    pub ttl_secs: i64,
}

fn default_ttl_secs() -> i64 {
    300 // 5 minutes — short enough that a page-source leak of the minted
        // token (as opposed to the raw secret) is only useful briefly.
}

/// POST /api/widget/:workflow_id/mint-token
///
/// For the "recommended for production" embedding pattern in
/// widget-embedding.md: the workflow author's own backend — which already
/// holds the real secret to have configured the Webhook node in the first
/// place — calls this **server-side** (never from a browser) to exchange
/// that secret for a short-lived signed token, then serves only the token to
/// visitors. Unlike the raw secret, a token captured from page source
/// self-expires instead of granting standing access.
///
/// Deliberately outside `auth_middleware` like `trigger_widget` — this is
/// authenticated by the workflow's own secret, not a server Bearer token,
/// for the same reason `trigger_widget` is: the caller here is the site
/// owner's backend, which has no server admin token of its own to send.
pub async fn mint_widget_token(
    State(s): State<ApiState>,
    Path(workflow_id): Path<String>,
    Json(b): Json<MintTokenBody>,
) -> impl IntoResponse {
    let (_port, path, configured_secret) = match load_webhook_trigger(&s, &workflow_id).await {
        Ok(t) => t,
        Err(resp) => return *resp,
    };

    if configured_secret.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "this Webhook node has no secret configured — nothing to mint a token from"})),
        )
            .into_response();
    }

    // Constant-time compare, same rationale as webhook.rs's own check: this
    // endpoint is exactly as much of a secret-guessing oracle (401 vs 200)
    // as the trigger endpoint, and sits behind the same rate limiter.
    use subtle::ConstantTimeEq as _;
    let expected_hash = blake3::hash(configured_secret.as_bytes());
    let provided_hash = blake3::hash(b.secret.as_bytes());
    if expected_hash.as_bytes().ct_eq(provided_hash.as_bytes()).unwrap_u8() != 1 {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "secret did not match this workflow's Webhook node"})),
        )
            .into_response();
    }

    let ttl = b.ttl_secs.clamp(1, 24 * 60 * 60);
    let token = aerini_engine::nodes::webhook::mint_signed_token(&configured_secret, &path, ttl);
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(ttl);

    (
        StatusCode::OK,
        Json(json!({
            "token": token,
            "expires_at": expires_at.to_rfc3339(),
            "ttl_secs": ttl
        })),
    )
        .into_response()
}

#[cfg(test)]
mod dangerous_node_gate_tests {
    use super::*;
    use aerini_engine::model::{NodeType, WorkflowNode};
    use std::collections::HashMap;

    fn node(id: &str, node_type_id: &str) -> WorkflowNode {
        WorkflowNode {
            id:            id.to_string(),
            node_type_id:  node_type_id.to_string(),
            node_type:     NodeType::Utility,
            name:          id.to_string(),
            config:        serde_json::json!({}),
            credentials:   HashMap::new(),
            input_schema:  serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry:         Default::default(),
            fallback_node: None,
            disabled:      false,
            position:      Default::default(),
        }
    }

    // Normal case: an ordinary chat-workflow shape (Webhook + Output, no
    // dangerous node type) must relay through untouched.
    #[test]
    fn allows_workflow_with_no_dangerous_nodes() {
        let nodes = vec![node("n1", "webhook"), node("n2", "output")];
        assert!(dangerous_node_block(&nodes).is_none());
    }

    // Shell/Code/Database node present: --allow-shell/--allow-code/
    // --allow-database are process-wide, so this must block regardless of
    // whether one of those flags happens to be on for a different workflow
    // on the same server.
    #[test]
    fn blocks_workflow_containing_a_dangerous_node() {
        let nodes = vec![node("n1", "webhook"), node("n2", "shell_exec")];
        let resp = dangerous_node_block(&nodes).expect("must block");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
