//! Embeddable chat widget: a static JS asset plus a public (no-Bearer-auth)
//! trigger gateway that relays a browser POST to the target workflow's
//! already-running Webhook listener.
//!
//! # Why this gateway exists instead of POSTing the browser straight to the
//! webhook port
//! `aerini-engine/src/nodes/webhook.rs`'s raw hyper listener sets no
//! `Access-Control-*` headers and has no `OPTIONS` handling on any response
//! path (confirmed by a full read of that file this session) — a
//! cross-origin browser POST straight to the webhook URL is unconditionally
//! blocked by the browser's CORS preflight. Rather than add CORS handling to
//! that listener (touching the already-once-fixed, concurrency-sensitive
//! trigger code), this route reuses the `CorsLayer` already on the main API
//! router (`api_server/mod.rs`) and forwards the validated request as a
//! loopback call. `webhook.rs` itself is untouched by this patch.
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
    /// author and passed through by the widget's `data-secret` attribute.
    /// Forwarded verbatim as `x-webhook-secret`; never compared in this
    /// handler (see module doc — single source of truth stays in
    /// `webhook.rs`).
    pub secret: String,
    /// Forwarded verbatim as the loopback request's JSON body. Shape is
    /// whatever the target workflow's Webhook node expects — the chat
    /// widget sends `{"message": ..., "session_id": ...}`, mirroring the
    /// desktop Chat Panel (`src/panels/ChatPanel.ts::handleSend`).
    #[serde(default)]
    pub body: Value,
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
    // scheduler_get / load are synchronous rusqlite calls — spawn_blocking
    // to avoid stalling the async reactor, matching the pattern every
    // workflows.rs CRUD handler already uses for `s.db.*` calls.
    //
    // NOTICED BUT NOT FIXED (Rule 6 — out of scope, pre-existing,
    // untouched by this patch): `routes/scheduler.rs`'s list_scheduler /
    // start_job / stop_job call `s.scheduler.*` (which hits the same
    // blocking rusqlite pool) directly on the async handler, with no
    // spawn_blocking. This file does not follow that precedent — it
    // follows workflows.rs's safer one instead, since both precedents
    // already coexist in the shipped codebase and the safer one is the
    // defensible default for new code.
    let row = {
        let db = Arc::clone(&s.db);
        let wf_id = workflow_id.clone();
        match tokio::task::spawn_blocking(move || db.scheduler_get(&wf_id)).await {
            Ok(Ok(Some(r))) => r,
            Ok(Ok(None)) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "workflow is not scheduled — start it first"})),
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

    if row.status != "active" {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!("workflow is not running (status: {})", row.status)
            })),
        )
            .into_response();
    }

    let trigger: TriggerKind = match serde_json::from_str(&row.trigger_kind) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("corrupt trigger_kind in scheduler row: {}", e)
                })),
            )
                .into_response();
        }
    };

    let (port, path) = match trigger {
        TriggerKind::Webhook { port, path, .. } => (port, path),
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!(
                        "workflow's trigger is '{}', not Webhook — the embeddable widget only works with a Webhook-triggered workflow",
                        other.label()
                    )
                })),
            )
                .into_response();
        }
    };

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

    // Only computed on the success path — an unauthenticated caller with the
    // wrong secret never learns the workflow's internal node IDs.
    let output_node_id = {
        let db = Arc::clone(&s.db);
        let wf_id = workflow_id.clone();
        match tokio::task::spawn_blocking(move || db.load(&wf_id)).await {
            Ok(Ok(Some(wf))) => wf
                .nodes
                .iter()
                .find(|n| n.node_type_id.as_str() == "output")
                .map(|n| n.id.clone()),
            _ => None,
        }
    };

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "output_node_id": output_node_id
        })),
    )
        .into_response()
}
