use axum::{
    extract::{Form, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dashmap::DashMap;
use serde::Deserialize;
use subtle::ConstantTimeEq;
use blake3;
use serde_json::{json, Value};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;

use flowo_engine::db::WorkflowDb;

use crate::util::extract_client_ip;

use crate::event_bridge::SharedRunState;
use crate::log_buffer::LogBuffer;

#[derive(Clone)]
pub struct StatusState {
    pub workflow_name: String,
    pub workflow_id:   String,
    pub trigger_desc:  String,
    #[allow(dead_code)]
    pub status_port:   u16,
    pub run_state:     SharedRunState,
    pub log_buffer:    LogBuffer,
    pub run_trigger:   Arc<tokio::sync::Notify>,
    /// Token required in the "Run Now" form. None = button hidden on status page.
    pub run_secret:    Option<String>,
    /// Per-IP rate limiter: maps client address → (request_count, window_start).
    pub rate_limiter:  Arc<DashMap<IpAddr, (u32, Instant)>>,
    /// Persistent run history database. None in API mode (status server not used).
    pub run_history:   Option<Arc<WorkflowDb>>,
    /// Number of reverse-proxy hops to trust when reading X-Forwarded-For.
    /// 0 = use TCP source IP directly (default, safe for direct deployments).
    pub trusted_proxy_count: usize,
}

pub fn router(state: StatusState) -> Router {
    Router::new()
        .route("/",              get(status_page))
        .route("/api/status",    get(api_status))
        .route("/api/logs",      get(api_logs))
        .route("/api/run",       post(api_run))
        .route("/api/runs",      get(api_runs))
        .route("/api/runs/:id",  get(api_run_detail))
        .layer(axum::middleware::from_fn_with_state(state.clone(), rate_limit_middleware))
        .layer(middleware::from_fn(security_headers_middleware))
        .with_state(state)
}

/// Adds defensive HTTP response headers to every status server response.
async fn security_headers_middleware(req: Request, next: Next) -> impl IntoResponse {
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("x-frame-options",        HeaderValue::from_static("DENY"));
    h.insert("referrer-policy",        HeaderValue::from_static("strict-origin-when-cross-origin"));
    h.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'"),
    );
    response
}

/// Per-IP rate limiter: 120 requests per 60-second window per client address.
async fn rate_limit_middleware(
    State(s): State<StatusState>,
    req: Request,
    next: Next,
) -> Response {
    let ip: IpAddr = extract_client_ip(&req, s.trusted_proxy_count);
    let allowed = {
        let now = Instant::now();
        let mut entry = s.rate_limiter.entry(ip).or_insert((0u32, now));
        if now.duration_since(entry.1).as_secs() >= 60 {
            *entry = (1, now);
            true
        } else if entry.0 < 120 {
            entry.0 += 1;
            true
        } else {
            false
        }
    };
    // Evict stale entries to prevent unbounded map growth under IP rotation or DDoS.
    if s.rate_limiter.len() > 10_000 {
        let cutoff = Instant::now() - std::time::Duration::from_secs(120);
        s.rate_limiter.retain(|_, v| v.1 > cutoff);
    }
    if allowed {
        next.run(req).await
    } else {
        (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded").into_response()
    }
}

/// GET / — HTML status page, auto-refreshes every 30 seconds.
async fn status_page(State(s): State<StatusState>) -> Html<String> {
    let rs = s.run_state.read().expect("run_state RwLock poisoned").clone();
    let status_color = match rs.status.as_str() {
        "running"  => "#f59e0b",
        "error"    => "#ef4444",
        "done"     => "#6b7280",
        _          => "#22c55e",
    };

    let last_run   = rs.last_run_at.as_deref().unwrap_or("—");
    let next_run   = rs.next_run_at.as_deref().unwrap_or("—");
    let last_error = rs.last_error.as_deref().unwrap_or("");

    let logs_html: String = s.log_buffer.last_n(50).iter().rev().map(|e| {
        let color = match e.level.as_str() {
            "ERROR" => "#ef4444",
            "WARN"  => "#f59e0b",
            "DEBUG" => "#6b7280",
            _       => "#d1d5db",
        };
        let node = e.node_id.as_deref()
            .map(|n| format!("<span style='color:#818cf8'>[{}]</span> ", n))
            .unwrap_or_default();
        format!(
            "<tr><td style='color:#6b7280;white-space:nowrap;padding-right:16px'>{}</td>\
             <td style='color:{};padding-right:8px'>{}</td>\
             <td>{}{}</td></tr>",
            &e.timestamp[..19].replace('T', " "),
            color, e.level, node,
            html_escape(&e.message)
        )
    }).collect();

    // Recent run history from persistent DB (last 10 runs).
    let history_html: String = if let Some(ref db) = s.run_history {
        match db.list_runs(&s.workflow_id, 0, 10, "") {
            Ok(runs) if !runs.is_empty() => {
                let rows: String = runs.iter().map(|r| {
                    let ts    = &r.ran_at[..19.min(r.ran_at.len())];
                    let ts    = ts.replace('T', " ");
                    let color = if r.success { "#22c55e" } else { "#ef4444" };
                    let label = if r.success { "success" } else { "failed" };
                    let dur   = format!("{:.1}s", r.duration_ms as f64 / 1000.0);
                    format!(
                        "<tr>\
                         <td style='color:#6b7280;white-space:nowrap;padding-right:16px'>{}</td>\
                         <td style='color:{};padding-right:16px'>{}</td>\
                         <td style='color:#64748b'>{}</td>\
                         </tr>",
                        html_escape(&ts), color, label, html_escape(&dur)
                    )
                }).collect();
                format!(
                    "<div class='log-section' style='margin-bottom:16px'>\
                       <div class='log-title'>Run history (last 10) &nbsp;\
                         {}\
                       </div>\
                       <table><tbody>{}</tbody></table>\
                     </div>",
                    if s.run_secret.is_some() {
                        "<a href='/api/runs' style='color:#3b82f6;font-size:11px'>JSON ↗</a>"
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
  <title>Flowo — {wf}</title>
  <style>
    *{{box-sizing:border-box;margin:0;padding:0}}
    body{{background:#0f172a;color:#e2e8f0;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;padding:24px;font-size:14px}}
    h1{{font-size:20px;font-weight:600;margin-bottom:4px}}
    .sub{{color:#64748b;font-size:13px;margin-bottom:24px}}
    .cards{{display:flex;gap:16px;flex-wrap:wrap;margin-bottom:24px}}
    .card{{background:#1e293b;border:1px solid #334155;border-radius:8px;padding:16px;min-width:160px}}
    .card-label{{font-size:11px;text-transform:uppercase;letter-spacing:.06em;color:#64748b;margin-bottom:6px}}
    .card-value{{font-size:16px;font-weight:500}}
    .status-dot{{display:inline-block;width:8px;height:8px;border-radius:50%;background:{sc};margin-right:6px}}
    .log-section{{background:#1e293b;border:1px solid #334155;border-radius:8px;padding:16px}}
    .log-title{{font-size:12px;text-transform:uppercase;letter-spacing:.06em;color:#64748b;margin-bottom:12px}}
    table{{width:100%;border-collapse:collapse;font-family:'SF Mono','Fira Code',monospace;font-size:12px}}
    tr+tr td{{border-top:1px solid #1e293b}}
    td{{padding:3px 0;color:#94a3b8;vertical-align:top}}
    .error-banner{{background:#450a0a;border:1px solid #7f1d1d;border-radius:6px;padding:10px 14px;margin-bottom:16px;color:#fca5a5;font-size:13px}}
    form{{margin-top:16px}}
    button{{background:#3b82f6;color:#fff;border:none;border-radius:6px;padding:8px 16px;cursor:pointer;font-size:13px}}
    button:hover{{background:#2563eb}}
    .refresh-note{{color:#475569;font-size:11px;margin-top:16px}}
  </style>
</head>
<body>
  <h1>{wf}</h1>
  <div class="sub">{trigger} · exported {exported}</div>

  {error_banner}

  <div class="cards">
    <div class="card">
      <div class="card-label">Status</div>
      <div class="card-value"><span class="status-dot"></span>{status}</div>
    </div>
    <div class="card">
      <div class="card-label">Total Runs</div>
      <div class="card-value">{runs}</div>
    </div>
    <div class="card">
      <div class="card-label">Last Run</div>
      <div class="card-value" style="font-size:13px">{last_run}</div>
    </div>
    <div class="card">
      <div class="card-label">Next Run</div>
      <div class="card-value" style="font-size:13px">{next_run}</div>
    </div>
  </div>

  {history}

  <div class="log-section">
    <div class="log-title">Last 50 log entries</div>
    <table><tbody>{logs}</tbody></table>
  </div>

  {run_now_form}
  <p class="refresh-note">Auto-refreshes every 30 seconds.</p>
</body>
</html>"#,
        wf       = html_escape(&s.workflow_name),
        trigger  = html_escape(&s.trigger_desc),
        exported = "—",
        sc       = status_color,
        status   = html_escape(&rs.status),
        runs     = rs.run_count,
        last_run = html_escape(last_run),
        next_run = html_escape(next_run),
        error_banner = if !last_error.is_empty() {
            format!("<div class='error-banner'>Last error: {}</div>",
                html_escape(last_error))
        } else { String::new() },
        history  = history_html,
        logs     = logs_html,
        run_now_form = match &s.run_secret {
            Some(_) => "<form method=\"POST\" action=\"/api/run\" style=\"margin-top:16px;display:flex;gap:8px;align-items:center\">\
                <input type=\"password\" name=\"secret\" placeholder=\"Run secret\" required \
                    style=\"background:#0f172a;border:1px solid #334155;border-radius:6px;padding:7px 12px;\
                           color:#e2e8f0;font-size:13px;width:220px\">\
                <button type=\"submit\">&#9654; Run Now</button>\
                </form>".to_string(),
            None => String::new(),
        },
    ))
}

/// GET /api/status — JSON status.
async fn api_status(State(s): State<StatusState>) -> Json<Value> {
    let rs = s.run_state.read().expect("run_state RwLock poisoned").clone();
    Json(json!({
        "workflow_name": s.workflow_name,
        "trigger":       s.trigger_desc,
        "status":        rs.status,
        "run_count":     rs.run_count,
        "last_run_at":   rs.last_run_at,
        "next_run_at":   rs.next_run_at,
        "last_error":    rs.last_error,
        "last_duration_ms": rs.last_duration_ms,
    }))
}

/// GET /api/logs?n=50 — JSON log array.
async fn api_logs(State(s): State<StatusState>) -> Json<Value> {
    Json(json!(s.log_buffer.last_n(50)))
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
    let expected = match &state.run_secret {
        Some(s) => s,
        None => return Err((StatusCode::FORBIDDEN,
            "Run history requires a run secret. Set one at export time.")),
    };
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    // Hash both sides with BLAKE3 before constant-time comparison to prevent
    // length oracle attacks (same pattern as api_run).
    let ok: bool = blake3::hash(expected.as_bytes())
        .as_bytes()
        .ct_eq(blake3::hash(provided.as_bytes()).as_bytes())
        .into();
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

#[derive(Deserialize)]
struct RunForm {
    secret: Option<String>,
}

/// POST /api/run — manual one-shot trigger from the status page button.
/// Requires the correct secret token when one is configured.
async fn api_run(
    State(s): State<StatusState>,
    Form(form): Form<RunForm>,
) -> impl IntoResponse {
    match &s.run_secret {
        Some(expected) => {
            let provided = form.secret.as_deref().unwrap_or("");
            // Hash both sides with BLAKE3 before constant-time comparison.
            // ct_eq on byte slices of different lengths returns 0 immediately,
            // leaking the expected length via timing. Hashing normalises both
            // to a fixed 32-byte output, eliminating the length oracle.
            let ok: bool = blake3::hash(expected.as_bytes())
                .as_bytes()
                .ct_eq(blake3::hash(provided.as_bytes()).as_bytes())
                .into();
            if !ok {
                return (StatusCode::FORBIDDEN, "Invalid secret");
            }
        }
        None => return (StatusCode::FORBIDDEN, "Manual trigger is disabled"),
    }
    s.run_trigger.notify_one();
    (StatusCode::ACCEPTED, "Run triggered")
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
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
            status_port:         0,
            run_state:           std::sync::Arc::new(std::sync::RwLock::new(RunState::default())),
            log_buffer:          LogBuffer::new(10),
            run_trigger:         std::sync::Arc::new(tokio::sync::Notify::new()),
            run_secret:          run_secret.map(|s| s.to_string()),
            rate_limiter:        std::sync::Arc::new(DashMap::new()),
            run_history:         None,
            trusted_proxy_count: 0,
        }
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
}
