// Static CSS for the status page. No template variables — content must stay
// byte-for-byte stable so the CSP sha256 hash in middleware::status_security_headers
// remains valid. The hash is computed automatically at runtime from this
// constant, so no manual recomputation is needed when this changes.
pub(crate) const STATUS_PAGE_CSS: &str = concat!(
    "*{box-sizing:border-box;margin:0;padding:0}\n",
    "    body{background:#0f172a;color:#e2e8f0;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;padding:24px;font-size:14px}\n",
    "    h1{font-size:20px;font-weight:600;margin-bottom:4px}\n",
    "    .sub{color:#64748b;font-size:13px;margin-bottom:24px}\n",
    "    .cards{display:flex;gap:16px;flex-wrap:wrap;margin-bottom:24px}\n",
    "    .card{background:#1e293b;border:1px solid #334155;border-radius:8px;padding:16px;min-width:160px}\n",
    "    .card-label{font-size:11px;text-transform:uppercase;letter-spacing:.06em;color:#64748b;margin-bottom:6px}\n",
    "    .card-value{font-size:16px;font-weight:500}\n",
    "    .card-value-sm{font-size:13px}\n",
    "    .status-dot{display:inline-block;width:8px;height:8px;border-radius:50%;margin-right:6px}\n",
    "    .status-dot.running{background:#f59e0b}\n",
    "    .status-dot.error{background:#ef4444}\n",
    "    .status-dot.done{background:#6b7280}\n",
    "    .status-dot.idle{background:#22c55e}\n",
    "    .log-section{background:#1e293b;border:1px solid #334155;border-radius:8px;padding:16px}\n",
    "    .log-title{font-size:12px;text-transform:uppercase;letter-spacing:.06em;color:#64748b;margin-bottom:12px}\n",
    "    table{width:100%;border-collapse:collapse;font-family:'SF Mono','Fira Code',monospace;font-size:12px}\n",
    "    tr+tr td{border-top:1px solid #1e293b}\n",
    "    td{padding:3px 0;color:#94a3b8;vertical-align:top}\n",
    "    .error-banner{background:#450a0a;border:1px solid #7f1d1d;border-radius:6px;padding:10px 14px;margin-bottom:16px;color:#fca5a5;font-size:13px}\n",
    "    form{margin-top:16px}\n",
    "    button{background:#3b82f6;color:#fff;border:none;border-radius:6px;padding:8px 16px;cursor:pointer;font-size:13px}\n",
    "    button:hover{background:#2563eb}\n",
    "    .refresh-note{color:#475569;font-size:11px;margin-top:16px}\n",
    "    .run-ts{color:#6b7280;white-space:nowrap;padding-right:16px}\n",
    "    .run-status{padding-right:16px}\n",
    "    .run-success{color:#22c55e}\n",
    "    .run-failed{color:#ef4444}\n",
    "    .run-dur{color:#64748b}\n",
    "    .history-link{color:#3b82f6;font-size:11px}\n",
    "    .logs-empty{color:#475569;font-style:italic;padding:8px 0}\n",
    "    .run-now-wrap{margin-top:16px;display:flex;gap:8px;align-items:center}\n",
    "    .run-now-input{background:#0f172a;border:1px solid #334155;border-radius:6px;padding:7px 12px;color:#e2e8f0;font-size:13px;width:220px}\n",
    "    .history-section{margin-bottom:16px}",
);

/// Inline script content rendered between `<script>` tags in the status page.
/// This const is the exact byte sequence hashed by the CSP script-src directive.
/// The hash is computed automatically at runtime from this constant, so no
/// manual recomputation is needed when this changes.
pub(crate) const STATUS_PAGE_SCRIPT: &str = concat!(
    "async function _aeriniRun(){",
    "var s=document.getElementById('_aerini_rs').value;",
    "var r=await fetch('/api/run',{method:'POST',",
    "headers:{'Authorization':'Bearer '+s,'Content-Type':'application/json'},",
    "body:'{}'});",
    "if(r.ok){document.getElementById('_aerini_rs').value='';alert('Run triggered.');}",
    "else{alert('Invalid run secret.');}}"
);

use axum::{
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use subtle::ConstantTimeEq;
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use serde_json::{json, Value};
use std::net::IpAddr;
use std::sync::Arc;

use aerini_engine::db::WorkflowDb;

use crate::util::extract_client_ip;

use crate::event_bridge::SharedRunState;
use crate::log_buffer::LogBuffer;

#[derive(Clone)]
pub struct StatusState {
    pub workflow_name: String,
    pub workflow_id:   String,
    pub trigger_desc:  String,
    pub run_state:     SharedRunState,
    pub log_buffer:    LogBuffer,
    pub run_trigger:   Arc<tokio::sync::Notify>,
    /// Token required in the "Run Now" form. None = button hidden on status page.
    pub run_secret:    Option<String>,
    /// Per-IP rate limiter: maps client address → (request_count, window_start).
    pub rate_limiter:  Arc<crate::middleware::RateLimiter>,
    /// Persistent run history database. None in API mode (status server not used).
    pub run_history:   Option<Arc<WorkflowDb>>,
    /// Number of reverse-proxy hops to trust when reading X-Forwarded-For.
    /// 0 = use TCP source IP directly (default, safe for direct deployments).
    pub trusted_proxy_count: usize,
}

pub fn router(state: StatusState) -> Router {
    // Explicit CORS: reject all cross-origin requests.  The status server
    // serves HTML and JSON to same-origin browser tabs only.  An explicit deny
    // policy prevents a future wildcard regression if this router is modified.
    let cors = tower_http::cors::CorsLayer::new();

    Router::new()
        .route("/",              get(status_page))
        .route("/api/status",    get(api_status))
        .route("/api/logs",      get(api_logs))
        .route("/api/run",       post(api_run))
        .route("/api/runs",      get(api_runs))
        .route("/api/runs/{id}",  get(api_run_detail))
        .layer(axum::middleware::from_fn_with_state(state.clone(), rate_limit_middleware))
        .layer(middleware::from_fn(crate::middleware::status_security_headers))
        .layer(cors)
        .with_state(state)
}


/// Per-IP rate limiter: 120 requests per 60-second window per client address.
async fn rate_limit_middleware(
    State(s): State<StatusState>,
    req: Request,
    next: Next,
) -> Response {
    let ip: IpAddr = extract_client_ip(&req, s.trusted_proxy_count);
    if s.rate_limiter.is_allowed(ip) {
        next.run(req).await
    } else {
        (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded").into_response()
    }
}

/// GET / — HTML status page, auto-refreshes every 30 seconds.
async fn status_page(State(s): State<StatusState>) -> Html<String> {
    let rs = s.run_state.read().expect("run_state RwLock poisoned").clone();
    let status_class = match rs.status.as_str() {
        "running" => "running",
        "error"   => "error",
        "done"    => "done",
        _         => "idle",
    };

    let last_run   = rs.last_run_at.as_deref().unwrap_or("—");
    let next_run   = rs.next_run_at.as_deref().unwrap_or("—");
    let last_error = rs.last_error.as_deref().unwrap_or("");

    // Logs are never rendered in the public HTML page regardless of whether a
    // run_secret is set. Execution logs can contain API responses, database values,
    // or internal error details that must not be publicly accessible.
    // The authenticated GET /api/logs endpoint is the only way to read log entries.
    let logs_html: &'static str =
        "<tr><td colspan='3' class='logs-empty'>\
         Logs are only available via the authenticated endpoint. Use \
         <code>GET /api/logs</code> with \
         <code>Authorization: Bearer &lt;run_secret&gt;</code>.\
         </td></tr>";

    // Recent run history from persistent DB (last 10 runs).
    let history_html: String = if let Some(ref db) = s.run_history {
        match db.list_runs(&s.workflow_id, 0, 10, "") {
            Ok(runs) if !runs.is_empty() => {
                let rows: String = runs.iter().map(|r| {
                    let ts    = &r.ran_at[..19.min(r.ran_at.len())];
                    let ts    = ts.replace('T', " ");
                    let color_class = if r.success { "run-success" } else { "run-failed" };
                    let label = if r.success { "success" } else { "failed" };
                    let dur   = format!("{:.1}s", r.duration_ms as f64 / 1000.0);
                    format!(
                        "<tr>\
                         <td class='run-ts'>{}</td>\
                         <td class='run-status {}'>{}</td>\
                         <td class='run-dur'>{}</td>\
                         </tr>",
                        html_escape(&ts), color_class, label, html_escape(&dur)
                    )
                }).collect();
                format!(
                    "<div class='log-section history-section'>\
                       <div class='log-title'>Run history (last 10) &nbsp;\
                         {}\
                       </div>\
                       <table><tbody>{}</tbody></table>\
                     </div>",
                    if s.run_secret.is_some() {
                        "<a href='/api/runs' class='history-link'>JSON ↗</a>"
                    } else {
                        ""
                    },
                    rows
                )
            }
            _ => String::new(),
        }
    } else {
        String::new()
    };

    Html(format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta http-equiv="refresh" content="30">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Aerini — {wf}</title>
  <style>{css}</style>
</head>
<body>
  <h1>{wf}</h1>
  <div class="sub">{trigger} · exported {exported}</div>

  {error_banner}

  <div class="cards">
    <div class="card">
      <div class="card-label">Status</div>
      <div class="card-value"><span class="status-dot {status_class}"></span>{status}</div>
    </div>
    <div class="card">
      <div class="card-label">Total Runs</div>
      <div class="card-value">{runs}</div>
    </div>
    <div class="card">
      <div class="card-label">Last Run</div>
      <div class="card-value card-value-sm">{last_run}</div>
    </div>
    <div class="card">
      <div class="card-label">Next Run</div>
      <div class="card-value card-value-sm">{next_run}</div>
    </div>
  </div>

  {history}

  <div class="log-section">
    <div class="log-title">{log_section_title}</div>
    <table><tbody>{logs}</tbody></table>
  </div>

  {run_now_form}
  <p class="refresh-note">Auto-refreshes every 30 seconds.</p>
</body>
</html>"#,
        css      = STATUS_PAGE_CSS,
        wf       = html_escape(&s.workflow_name),
        trigger  = html_escape(&s.trigger_desc),
        exported = "—",
        status_class = status_class,
        status   = html_escape(&rs.status),
        runs     = rs.run_count,
        last_run = html_escape(last_run),
        next_run = html_escape(next_run),
        error_banner = if !last_error.is_empty() && s.run_secret.is_some() {
            // Only show the error banner when the deployment is secured with a run_secret.
            // Without a secret the page is public — error messages can expose internal details.
            // The authenticated GET /api/status JSON endpoint always includes last_error.
            format!("<div class='error-banner'>Last error: {}</div>",
                html_escape(last_error))
        } else if !last_error.is_empty() {
            // Public page: indicate an error occurred without exposing the message.
            "<div class='error-banner'>Last run ended with an error. \
             Set a run_secret and use <code>/api/status</code> to see details.</div>".to_string()
        } else { String::new() },
        history  = history_html,
        log_section_title = "Logs (authenticated only)",
        logs     = logs_html,
        run_now_form = match &s.run_secret {
            Some(_) => format!(
                concat!(
                    "<div class='run-now-wrap'>",
                    "<input type='password' id='_aerini_rs' placeholder='Run secret' ",
                    "class='run-now-input'>",
                    "<button onclick='_aeriniRun()'>&#9654; Run Now</button>",
                    "</div>",
                    "<script>{}</script>"
                ),
                STATUS_PAGE_SCRIPT
            ),
            None => String::new(),
        },
    ))
}

/// GET /api/status — JSON status.
/// `last_error` is only included when the request carries a valid run_secret.
/// Unauthenticated callers receive all non-sensitive fields (counts, timestamps, status)
/// but not the error string, which may contain internal paths or scrubbed credential URLs.
async fn api_status(
    State(s): State<StatusState>,
    headers:  HeaderMap,
) -> Json<Value> {
    let rs     = s.run_state.read().expect("run_state RwLock poisoned").clone();
    let authed = check_run_secret(&s, &headers).is_ok();
    Json(json!({
        "workflow_name": s.workflow_name,
        "trigger":       s.trigger_desc,
        "status":        rs.status,
        "run_count":     rs.run_count,
        "last_run_at":   rs.last_run_at,
        "next_run_at":   rs.next_run_at,
        "last_duration_ms": rs.last_duration_ms,
        "last_error": if authed { rs.last_error } else { None },
    }))
}

/// GET /api/logs?n=50 — JSON log array.
/// Requires: Authorization: Bearer <run_secret>
async fn api_logs(
    State(s): State<StatusState>,
    headers:  HeaderMap,
) -> impl IntoResponse {
    if let Err((status, msg)) = check_run_secret(&s, &headers) {
        return (status, msg).into_response();
    }
    Json(json!(s.log_buffer.last_n(50))).into_response()
}

#[derive(Deserialize)]
struct RunsQuery {
    #[serde(default = "default_runs_limit")]
    limit:  i64,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    filter: String,
}

fn default_runs_limit() -> i64 { 50 }

/// Validates run_secret from the Authorization: Bearer <secret> header.
///
/// Returns Ok(()) if authorized, Err((status, message)) otherwise.
///
/// Query-parameter auth is intentionally not supported: secrets in URLs
/// are recorded in server logs, proxy logs, and browser history (RFC 6750 §5.3).
fn check_run_secret(
    state:   &StatusState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, &'static str)> {
    let stored = match &state.run_secret {
        Some(s) => s,
        None => return Err((StatusCode::FORBIDDEN,
            "Run history requires a run secret. Set one at export time.")),
    };
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    // Since Aerini 0.3 the config stores an argon2id PHC string.
    // Backward-compat: Aerini 0.2 configs stored a 64-char BLAKE3 hex digest.
    // Detect by whether the value starts with the argon2 PHC prefix "$argon2".
    // For legacy configs, fall back to the old double-hash comparison path.
    let ok: bool = if stored.starts_with("$argon2") {
        // New path (0.3+): verify against argon2id PHC hash.
        // This is brute-force resistant; BLAKE3 was not.
        match PasswordHash::new(stored) {
            Ok(parsed_hash) => Argon2::default()
                .verify_password(provided.as_bytes(), &parsed_hash)
                .is_ok(),
            Err(_) => false,
        }
    } else if stored.len() == 64 {
        // Legacy path (0.2): stored is blake3_hex(raw); compare against blake3_hex(submitted).
        // kept for configs exported before 0.3 — re-export to upgrade.
        use blake3::hash as b3;
        let submitted_hash = b3(provided.as_bytes()).to_hex();
        stored.as_bytes().ct_eq(submitted_hash.as_bytes()).into()
    } else {
        // Very old path (pre-0.2): stored is raw secret; hash both sides.
        use blake3::hash as b3;
        b3(stored.as_bytes())
            .as_bytes()
            .ct_eq(b3(provided.as_bytes()).as_bytes())
            .into()
    };
    if ok { Ok(()) } else { Err((StatusCode::FORBIDDEN, "Invalid secret")) }
}

/// GET /api/runs?limit=N&offset=N&filter=success|failed — paginated run history.
/// Requires: Authorization: Bearer <run_secret>
async fn api_runs(
    State(s):  State<StatusState>,
    headers:   HeaderMap,
    Query(q):  Query<RunsQuery>,
) -> impl IntoResponse {
    if let Err((status, msg)) = check_run_secret(&s, &headers) {
        return (status, msg).into_response();
    }
    let Some(ref db) = s.run_history else {
        return Json(json!({"items": [], "limit": 0, "offset": 0})).into_response();
    };
    let limit  = q.limit.clamp(1, 500);
    let offset = q.offset.max(0);
    match db.list_runs(&s.workflow_id, offset, limit, &q.filter) {
        Ok(items) => Json(json!({
            "items":  items,
            "limit":  limit,
            "offset": offset,
        })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// GET /api/runs/:id — single run detail by execution ID.
/// Requires: Authorization: Bearer <run_secret>
async fn api_run_detail(
    State(s):  State<StatusState>,
    headers:   HeaderMap,
    Path(id):  Path<String>,
) -> impl IntoResponse {
    if let Err((status, msg)) = check_run_secret(&s, &headers) {
        return (status, msg).into_response();
    }
    let Some(ref db) = s.run_history else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match db.get_run(&id) {
        Ok(Some(record)) => Json(record).into_response(),
        Ok(None)         => StatusCode::NOT_FOUND.into_response(),
        Err(e)           => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// POST /api/run — manual one-shot trigger from the status page run button.
/// Requires: Authorization: Bearer <run_secret>
/// The secret is read from the Authorization header, not the request body,
/// to block cross-origin form-based CSRF attacks (JSON Content-Type triggers
/// a CORS preflight on cross-origin requests, which the status server rejects).
async fn api_run(
    State(s): State<StatusState>,
    headers:  HeaderMap,
) -> impl IntoResponse {
    if s.run_secret.is_none() {
        return (StatusCode::FORBIDDEN, "Manual trigger is disabled").into_response();
    }
    if let Err((status, msg)) = check_run_secret(&s, &headers) {
        return (status, msg).into_response();
    }
    s.run_trigger.notify_one();
    (StatusCode::ACCEPTED, "Run triggered").into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
     .replace('\'', "&#x27;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use crate::event_bridge::RunState;
    use tower::ServiceExt;

    fn make_state(run_secret: Option<&str>) -> StatusState {
        StatusState {
            workflow_name:       "test-workflow".to_string(),
            workflow_id:         "wf-test".to_string(),
            trigger_desc:        "manual".to_string(),
            run_state:           std::sync::Arc::new(std::sync::RwLock::new(RunState::default())),
            log_buffer:          LogBuffer::new(10),
            run_trigger:         std::sync::Arc::new(tokio::sync::Notify::new()),
            run_secret:          run_secret.map(|s| s.to_string()),
            rate_limiter:        std::sync::Arc::new(crate::middleware::RateLimiter::new(120, 60)),
            run_history:         None,
            trusted_proxy_count: 0,
        }
    }

    #[tokio::test]
    async fn api_status_last_error_requires_auth() {
        let state = std::sync::Arc::new(std::sync::RwLock::new(RunState {
            last_error: Some("postgres://admin:secret@internal-db:5432/prod — connection refused".into()),
            ..Default::default()
        }));
        let mut s = make_state(Some("secret"));
        s.run_state = state;
        let app = router(s);

        // No auth — last_error must be absent (null)
        let res = app
            .clone()
            .oneshot(Request::builder().uri("/api/status").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["last_error"].is_null(), "unauthenticated caller must not receive last_error");

        // With auth — last_error must be present
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(!v["last_error"].is_null(), "authenticated caller must receive last_error");
    }

    #[tokio::test]
    async fn run_history_requires_secret() {
        let app = router(make_state(Some("test")));

        let res = app
            .clone()
            .oneshot(Request::builder().uri("/api/runs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/runs")
                    .header("authorization", "Bearer test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn run_history_forbidden_when_no_secret() {
        let app = router(make_state(None));

        let res = app
            .oneshot(Request::builder().uri("/api/runs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn api_logs_requires_secret() {
        let app = router(make_state(Some("logsecret")));

        // No auth → 403
        let res = app
            .clone()
            .oneshot(Request::builder().uri("/api/logs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // Wrong secret → 403
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/logs")
                    .header("authorization", "Bearer wrongsecret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // Correct secret → 200
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/logs")
                    .header("authorization", "Bearer logsecret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_run_requires_bearer_header() {
        let app = router(make_state(Some("runsecret")));

        // No auth → 403
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/run")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // Wrong secret → 403
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/run")
                    .header("authorization", "Bearer badsecret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // Correct secret → 202
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/run")
                    .header("authorization", "Bearer runsecret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
    }

}
