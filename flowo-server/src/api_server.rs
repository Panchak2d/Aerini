//! Multi-workflow REST API server.
//!
//! All routes require:  Authorization: Bearer <token>
//! Except:             GET /api/health  (no auth)
//!
//! Routes:
//!   GET    /api/health
//!   GET    /api/workflows
//!   POST   /api/workflows
//!   GET    /api/workflows/:id
//!   DELETE /api/workflows/:id
//!   POST   /api/workflows/:id/run
//!   GET    /api/scheduler
//!   POST   /api/scheduler/:id/start
//!   POST   /api/scheduler/:id/stop
//!   GET    /api/credentials
//!   POST   /api/credentials
//!   DELETE /api/credentials/:id
//!   GET    /api/events   (SSE)
//!   GET    /api/tokens   (admin scope required)
//!   POST   /api/tokens   (admin scope required)
//!   DELETE /api/tokens/:id  (admin scope required)

use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Extension, Path, Query, Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Sse},
    routing::{delete, get, post},
    Json, Router,
};
use tower_http::cors::AllowOrigin;
use flowo_engine::{
    db::WorkflowDb,
    model::Workflow,
    node::NodeRegistry,
    nodes::register_builtins,
    scheduler::SchedulerDaemon,
    store::{CredentialStore, CreateCredentialRequest, KeySource, StoreCredentialResolver},
    EventSink,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use dashmap::DashMap;
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use uuid::Uuid;

use rand::RngCore;

use crate::event_bridge::BroadcastEventSink;
use crate::token_store::{TokenRecord, TokenStore};

#[derive(Clone)]
pub struct ApiState {
    pub db:               Arc<WorkflowDb>,
    pub scheduler:        Arc<SchedulerDaemon>,
    pub creds:            Arc<CredentialStore>,
    pub registry:         Arc<NodeRegistry>,
    pub sse_tx:           broadcast::Sender<String>,
    pub token_store:      Arc<TokenStore>,
    pub exec_locks:       Arc<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    pub env_allowlist:    Option<Arc<std::collections::HashSet<String>>>,
    pub file_sandbox_dir: Option<Arc<std::path::PathBuf>>,
    pub shell_exec_disabled:  bool,
    pub code_exec_disabled:   bool,
    pub parallel_execution:   bool,
    pub max_concurrent_nodes: usize,
}

/// Extracts the real client IP from X-Forwarded-For when behind trusted proxies.
///
/// `trusted_proxy_count` = how many proxy hops sit between the internet and
/// this server. With count=1 and header "1.2.3.4, 10.0.0.1", returns 1.2.3.4.
/// Falls back to the TCP source IP if: count is 0, header is absent/malformed,
/// or fewer IPs are present than the trust count.
fn extract_client_ip(req: &Request, trusted_proxy_count: usize) -> IpAddr {
    let tcp_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));

    if trusted_proxy_count == 0 {
        return tcp_ip;
    }

    let xff = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let ips: Vec<&str> = xff.split(',').map(|s| s.trim()).collect();
    if ips.len() < trusted_proxy_count {
        return tcp_ip;
    }

    let idx = ips.len().saturating_sub(trusted_proxy_count + 1);
    ips[idx].parse::<IpAddr>().unwrap_or(tcp_ip)
}

/// Loads the 32-byte HMAC key from `path`, or generates and persists it on first run.
///
/// Hard-exits (not panics) on any I/O error or wrong key length — the server
/// cannot safely operate without a consistent key.
fn load_or_create_token_key(path: &std::path::Path) -> [u8; 32] {
    if path.exists() {
        let bytes = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("FATAL: Cannot read token key file {:?}: {}", path, e);
            std::process::exit(1);
        });
        if bytes.len() != 32 {
            eprintln!(
                "FATAL: Token key file {:?} has wrong length ({} bytes, expected 32). \
                 If you intentionally want to invalidate all tokens, delete the file and restart.",
                path,
                bytes.len()
            );
            std::process::exit(1);
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        key
    } else {
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        std::fs::write(path, &key).unwrap_or_else(|e| {
            eprintln!("FATAL: Cannot write token key file {:?}: {}", path, e);
            std::process::exit(1);
        });
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, perms).unwrap_or_else(|e| {
                eprintln!("FATAL: Cannot set permissions on token key file {:?}: {}", path, e);
                std::process::exit(1);
            });
        }
        key
    }
}

pub async fn run(
    token: Option<String>,
    port: u16,
    data_dir: String,
    allow_origins: Vec<String>,
    allow_env_vars: Vec<String>,
    bind: String,
    file_sandbox_dir: Option<std::path::PathBuf>,
    trusted_proxy_count: usize,
    shell_exec_disabled: bool,
    code_exec_disabled: bool,
    use_keychain: bool,
    parallel_execution:   bool,
    max_concurrent_nodes: usize,
) {
    crate::init_tracing();
    let data_dir = PathBuf::from(if data_dir.starts_with('~') {
        data_dir.replacen('~',
            &std::env::var("HOME").unwrap_or_else(|_| "/root".to_string()), 1)
    } else {
        data_dir
    });
    std::fs::create_dir_all(&data_dir).expect("Cannot create data dir");

    // ── Token store ──────────────────────────────────────────────────────────
    let token_key = load_or_create_token_key(&data_dir.join("tokens.key"));
    let token_store = Arc::new(
        TokenStore::open(&data_dir.join("tokens.db"), token_key)
            .expect("Cannot open token store")
    );

    // Bootstrap: import --token / FLOWO_TOKEN as admin if provided.
    // If no token provided and store is empty, generate a new one.
    match &token {
        Some(t) => {
            token_store
                .import_token(t, "default", &["read", "write", "admin"])
                .expect("Cannot import token into token store");
        }
        None => {
            if token_store.is_empty() {
                let generated = Uuid::new_v4().to_string().replace('-', "");
                eprintln!();
                eprintln!("┌─────────────────────────────────────────────────────┐");
                eprintln!("│  Flowo Server — API Token (save this somewhere safe) │");
                eprintln!("│                                                       │");
                eprintln!("│  {}  │", generated);
                eprintln!("│                                                       │");
                eprintln!("│  Set FLOWO_TOKEN env var to skip this on restart.    │");
                eprintln!("└─────────────────────────────────────────────────────┘");
                eprintln!();
                token_store
                    .import_token(&generated, "default", &["read", "write", "admin"])
                    .expect("Cannot import generated token");
            }
        }
    }

    let db = Arc::new(
        WorkflowDb::open(&data_dir.join("flowo.db"))
            .expect("Cannot open workflow database")
    );
    let creds = Arc::new(
        CredentialStore::open(
            &data_dir.join("credentials.db"),
            if use_keychain {
                KeySource::OsKeychain { fallback: data_dir.join("flowo.key") }
            } else {
                KeySource::File(data_dir.join("flowo.key"))
            },
        ).expect("Cannot open credential store")
    );

    let mut registry = NodeRegistry::new();
    register_builtins(&mut registry, &data_dir, Some(Arc::clone(&db)));
    let registry = Arc::new(registry);

    let (sse_tx, _) = broadcast::channel::<String>(256);
    let event_sink  = Arc::new(BroadcastEventSink { tx: sse_tx.clone() });
    let resolver    = Arc::new(StoreCredentialResolver { store: Arc::clone(&creds) });

    let env_allowlist: Option<Arc<std::collections::HashSet<String>>> = if allow_env_vars.is_empty() {
        None
    } else {
        Some(Arc::new(allow_env_vars.into_iter().collect()))
    };

    let scheduler = {
        let mut daemon = SchedulerDaemon::new(
            Arc::clone(&db) as Arc<dyn flowo_engine::scheduler::SchedulerDb>,
            Arc::clone(&registry),
            Arc::clone(&resolver) as Arc<dyn flowo_engine::executor::CredentialResolver>,
            Arc::clone(&event_sink) as Arc<dyn EventSink>,
        );
        if let Some(ref allowlist) = env_allowlist {
            daemon = daemon.with_env_allowlist(allowlist.iter().cloned().collect());
        }
        if shell_exec_disabled {
            daemon = daemon.with_shell_disabled(true);
        }
        if code_exec_disabled {
            daemon = daemon.with_code_disabled(true);
        }
        if parallel_execution {
            daemon = daemon
                .with_parallel_execution(true)
                .with_max_concurrent_nodes(max_concurrent_nodes);
        }
        Arc::new(daemon)
    };
    scheduler.start(&tokio::runtime::Handle::current());

    let state = ApiState {
        db:               Arc::clone(&db),
        scheduler:        Arc::clone(&scheduler),
        creds:            Arc::clone(&creds),
        registry:         Arc::clone(&registry),
        sse_tx:           sse_tx.clone(),
        token_store:      Arc::clone(&token_store),
        exec_locks:       Arc::new(std::sync::Mutex::new(HashMap::new())),
        env_allowlist:    env_allowlist.clone(),
        file_sandbox_dir: file_sandbox_dir.map(|d| Arc::new(d)),
        shell_exec_disabled,
        code_exec_disabled,
        parallel_execution,
        max_concurrent_nodes,
    };

    let protected = Router::new()
        .route("/api/workflows",           get(list_workflows).post(save_workflow))
        .route("/api/workflows/:id",       get(get_workflow).delete(delete_workflow))
        .route("/api/workflows/:id/run",   post(run_workflow))
        .route("/api/scheduler",           get(list_scheduler))
        .route("/api/scheduler/:id/start", post(start_job))
        .route("/api/scheduler/:id/stop",  post(stop_job))
        .route("/api/credentials",         get(list_creds).post(save_cred))
        .route("/api/credentials/:id",     delete(delete_cred))
        .route("/api/events",              get(sse_events))
        .route("/api/tokens",              get(list_tokens_handler).post(create_token_handler))
        .route("/api/tokens/:id",          delete(revoke_token_handler))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    let extra_origins: std::sync::Arc<Vec<Vec<u8>>> = std::sync::Arc::new(
        allow_origins.into_iter().map(|o| o.into_bytes()).collect()
    );
    let extra_c = std::sync::Arc::clone(&extra_origins);
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin: &axum::http::HeaderValue, _| {
            let b = origin.as_bytes();
            b.starts_with(b"http://localhost")
                || b.starts_with(b"http://127.0.0.1")
                || extra_c.iter().any(|o| o.as_slice() == b)
        }))
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers(tower_http::cors::Any);

    // Per-IP rate limiter: 300 requests per 60-second window per client address.
    let rl_state: Arc<DashMap<IpAddr, (u32, Instant)>> = Arc::new(DashMap::new());
    let rl = Arc::clone(&rl_state);
    let tpc = trusted_proxy_count;
    let rate_limit_layer = middleware::from_fn(move |req: Request, next: Next| {
        let rl = Arc::clone(&rl);
        async move {
            let ip: IpAddr = extract_client_ip(&req, tpc);
            let allowed = {
                let now = Instant::now();
                let mut entry = rl.entry(ip).or_insert((0u32, now));
                if now.duration_since(entry.1).as_secs() >= 60 {
                    *entry = (1, now);
                    true
                } else if entry.0 < 300 {
                    entry.0 += 1;
                    true
                } else {
                    false
                }
            };
            // Evict stale entries to prevent unbounded map growth under IP rotation or DDoS.
            if rl.len() > 10_000 {
                let cutoff = Instant::now() - Duration::from_secs(120);
                rl.retain(|_, v| v.1 > cutoff);
            }
            if allowed {
                next.run(req).await
            } else {
                StatusCode::TOO_MANY_REQUESTS.into_response()
            }
        }
    });

    let app = Router::new()
        .route("/api/health", get(health))
        .merge(protected)
        .with_state(state)
        .layer(DefaultBodyLimit::max(5 * 1024 * 1024))
        .layer(cors)
        .layer(rate_limit_layer);

    let addr     = format!("{}:{}", bind, port);
    let listener = tokio::net::TcpListener::bind(&addr).await
        .unwrap_or_else(|e| { tracing::error!("Cannot bind {}: {}", addr, e); std::process::exit(1); });

    tracing::info!(
        api_url    = %format!("http://{}:{}", bind, port),
        health_url = %format!("http://{}:{}/api/health", bind, port),
        "Flowo Server started in API mode"
    );
    eprintln!("Control commands (from another terminal):");
    eprintln!("  flowo-server list    --token YOUR_TOKEN");
    eprintln!("  flowo-server stop    <workflow-name> --token YOUR_TOKEN");
    eprintln!("  flowo-server start   <workflow-name> --token YOUR_TOKEN");
    eprintln!("  flowo-server restart <workflow-name> --token YOUR_TOKEN");
    eprintln!("Token management:");
    eprintln!("  flowo-server tokens list   --token YOUR_TOKEN");
    eprintln!("  flowo-server tokens create --label ci --scopes read,write --token YOUR_TOKEN");

    let scheduler_sd = Arc::clone(&scheduler);
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = sigterm.recv() => {},
            }
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c().await.ok();

        tracing::info!("Shutting down");
        scheduler_sd.stop_all();
        std::process::exit(0);
    });

    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.expect("API server error");
}

/// Verifies the bearer token and attaches `TokenRecord` to request extensions.
async fn auth_middleware(
    State(s):    State<ApiState>,
    headers:     HeaderMap,
    mut request: Request,
    next:        Next,
) -> axum::response::Response {
    let provided = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    match s.token_store.verify_token(provided) {
        Some(record) => {
            request.extensions_mut().insert(record);
            next.run(request).await
        }
        None => {
            tracing::debug!("auth failed — invalid or missing token");
            (StatusCode::UNAUTHORIZED,
                Json(json!({"error":"Invalid or missing token"}))).into_response()
        }
    }
}

async fn health() -> Json<Value> {
    Json(json!({"status":"ok","version":flowo_engine::ENGINE_VERSION}))
}

#[derive(Deserialize)]
struct PaginationParams {
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
}

fn default_limit() -> usize { 100 }

async fn list_workflows(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Query(p):          Query<PaginationParams>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }
    let limit = p.limit.min(500);
    match tokio::task::spawn_blocking(move || s.db.list_paginated(limit, p.offset)).await {
        Ok(Ok((items, total))) => {
            (StatusCode::OK, Json(json!({"items": items, "total": total, "limit": limit, "offset": p.offset}))).into_response()
        },
        Ok(Err(e))   => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)       => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

async fn get_workflow(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }
    match tokio::task::spawn_blocking(move || s.db.load(&id)).await {
        Ok(Ok(Some(wf))) => (StatusCode::OK, Json(json!(wf.to_json_pretty().unwrap_or_default()))).into_response(),
        Ok(Ok(None))     => (StatusCode::NOT_FOUND, Json(json!({"error":"Not found"}))).into_response(),
        Ok(Err(e))       => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)           => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
struct SaveWorkflowBody { workflow_json: String }

async fn save_workflow(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Json(b):           Json<SaveWorkflowBody>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    let wf = match Workflow::from_json(&b.workflow_json) {
        Ok(w)  => w,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error":e.to_string()}))).into_response(),
    };
    match tokio::task::spawn_blocking(move || s.db.save(&wf)).await {
        Ok(Ok(())) => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

async fn delete_workflow(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    let _ = s.scheduler.stop_job(&id);
    let exec_locks  = Arc::clone(&s.exec_locks);
    let id_for_lock = id.clone();

    match tokio::task::spawn_blocking(move || {
        s.db.delete_runs_for_workflow(&id)?;
        s.db.delete_scheduled_job(&id)?;
        s.db.delete(&id)
    }).await {
        Ok(Ok(())) => {
            let mut locks = exec_locks.lock().expect("exec_locks mutex poisoned");
            locks.remove(&id_for_lock);
            (StatusCode::OK, Json(json!({"ok":true}))).into_response()
        },
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
struct RunBody { #[serde(default)] initial_variables: HashMap<String, Value> }

async fn run_workflow(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
    Json(b):           Json<RunBody>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    let wf_json = match tokio::task::spawn_blocking({
        let db = Arc::clone(&s.db);
        let id = id.clone();
        move || db.load(&id)
    }).await {
        Ok(Ok(Some(wf))) => wf.to_json_pretty().unwrap_or_default(),
        Ok(Ok(None))     => return (StatusCode::NOT_FOUND, Json(json!({"error":"Not found"}))).into_response(),
        Ok(Err(e))       => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
        Err(e)           => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    };
    let wf = match Workflow::from_json(&wf_json) {
        Ok(w)  => w,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error":e.to_string()}))).into_response(),
    };

    // Prune exec_locks if the map has grown large, removing entries for workflows
    // that no longer exist. delete_workflow removes on explicit delete; this covers
    // workflows that accumulated entries over a long server lifetime without deletion.
    {
        let len = s.exec_locks.lock().expect("exec_locks mutex poisoned").len();
        if len > 1000 {
            let db = Arc::clone(&s.db);
            if let Ok(Ok(summaries)) = tokio::task::spawn_blocking(move || db.list()).await {
                let live_ids: std::collections::HashSet<String> =
                    summaries.into_iter().map(|w| w.id).collect();
                s.exec_locks
                    .lock()
                    .expect("exec_locks mutex poisoned")
                    .retain(|k, _| live_ids.contains(k));
            }
        }
    }

    let lock = {
        let mut locks = s.exec_locks.lock().expect("exec_locks mutex poisoned");
        Arc::clone(locks.entry(id.clone()).or_insert_with(|| {
            Arc::new(tokio::sync::Mutex::new(()))
        }))
    };
    let _guard = match lock.try_lock() {
        Ok(g)  => g,
        Err(_) => return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": "workflow already running"})),
        ).into_response(),
    };

    let resolver   = Arc::new(StoreCredentialResolver { store: Arc::clone(&s.creds) });
    let event_sink = Arc::new(BroadcastEventSink { tx: s.sse_tx.clone() });
    let mut executor = flowo_engine::executor::WorkflowExecutor::new(
        Arc::clone(&s.registry),
        Arc::clone(&resolver) as Arc<dyn flowo_engine::executor::CredentialResolver>,
    ).with_event_sink(Arc::clone(&event_sink) as Arc<dyn EventSink>);
    if let Some(ref allowlist) = s.env_allowlist {
        executor = executor.with_env_allowlist(allowlist.iter().cloned().collect());
    }
    if let Some(ref sandbox) = s.file_sandbox_dir {
        executor = executor.with_file_sandbox_dir(sandbox.as_ref().clone());
    }
    if s.shell_exec_disabled {
        executor = executor.with_shell_disabled(true);
    }
    if s.code_exec_disabled {
        executor = executor.with_code_disabled(true);
    }
    if s.parallel_execution {
        executor = executor
            .with_parallel_execution(true)
            .with_max_concurrent_nodes(s.max_concurrent_nodes);
    }

    let run_result = executor.run(&wf, b.initial_variables).await;

    // If the workflow was deleted while this run was in progress, remove its
    // lock entry now rather than waiting for another explicit delete call.
    {
        let id_check = id.clone();
        let db = Arc::clone(&s.db);
        if let Ok(Ok(None)) = tokio::task::spawn_blocking(move || db.load(&id_check)).await {
            s.exec_locks
                .lock()
                .expect("exec_locks mutex poisoned")
                .remove(&id);
        }
    }

    match run_result {
        Ok(result) => (StatusCode::OK, Json(json!(result))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

async fn list_scheduler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Query(p):          Query<PaginationParams>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }
    match s.scheduler.list_jobs() {
        Ok(jobs) => {
            let total = jobs.len();
            let limit = p.limit.min(500);
            let items: Vec<_> = jobs.into_iter().skip(p.offset).take(limit).collect();
            (StatusCode::OK, Json(json!({"items": items, "total": total, "limit": limit, "offset": p.offset}))).into_response()
        },
        Err(e)   => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
    }
}

#[derive(Deserialize)]
struct StartBody { #[serde(default)] always_on: bool, port_override: Option<u16> }

async fn start_job(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
    Json(b):           Json<StartBody>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    match s.scheduler.start_job(&id, b.port_override, Some(b.always_on)) {
        Ok(())  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Err(e)  => (StatusCode::BAD_REQUEST, Json(json!({"error":format!("{:?}",e)}))).into_response(),
    }
}

async fn stop_job(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    match s.scheduler.stop_job(&id) {
        Ok(())  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Err(e)  => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
    }
}

#[derive(Deserialize)]
struct CredBody {
    id: String,
    name: String,
    value: String,
    #[serde(default = "default_cred_type")]
    cred_type: String,
}

fn default_cred_type() -> String { "api_key".to_string() }

async fn save_cred(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Json(b):           Json<CredBody>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    match s.creds.store(&CreateCredentialRequest { id: b.id, name: b.name, value: b.value, cred_type: b.cred_type }) {
        Ok(())  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Err(e)  => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

async fn list_creds(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }
    match s.creds.list() {
        Ok(list) => (StatusCode::OK, Json(json!(list))).into_response(),
        Err(e)   => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

async fn delete_cred(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_write(&caller) { return e.into_response(); }
    match s.creds.delete(&id) {
        Ok(())  => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Err(e)  => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

async fn sse_events(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) {
        return e.into_response();
    }
    let rx     = s.sse_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| {
        msg.ok().map(|data| Ok::<axum::response::sse::Event, std::convert::Infallible>(axum::response::sse::Event::default().data(data)))
    });
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("ping"),
    ).into_response()
}

// ── Token management ──────────────────────────────────────────────────────────

fn require_admin(record: &TokenRecord) -> Result<(), (StatusCode, Json<Value>)> {
    if record.has_scope("admin") {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error":"admin scope required for token management"})),
        ))
    }
}

fn require_read(record: &TokenRecord) -> Result<(), (StatusCode, Json<Value>)> {
    if record.has_scope("read") {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, Json(json!({"error":"read scope required"}))))
    }
}

fn require_write(record: &TokenRecord) -> Result<(), (StatusCode, Json<Value>)> {
    if record.has_scope("write") {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, Json(json!({"error":"write scope required"}))))
    }
}

async fn list_tokens_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    match s.token_store.list_tokens() {
        Ok(tokens) => (StatusCode::OK, Json(json!(tokens))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
struct CreateTokenBody {
    label:  String,
    #[serde(default = "default_token_scopes")]
    scopes: Vec<String>,
}

fn default_token_scopes() -> Vec<String> {
    vec!["read".to_string(), "write".to_string()]
}

async fn create_token_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Json(b):           Json<CreateTokenBody>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    let scopes_ref: Vec<&str> = b.scopes.iter().map(|s| s.as_str()).collect();
    match s.token_store.create_token(&b.label, &scopes_ref) {
        Ok(raw) => (StatusCode::CREATED, Json(json!({
            "token":  raw,
            "label":  b.label,
            "scopes": b.scopes,
            "note":   "Save this token — it will not be shown again."
        }))).into_response(),
        Err(e)  => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

async fn revoke_token_handler(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Path(id):          Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }
    match s.token_store.revoke_token(&id) {
        Ok(()) => (StatusCode::OK, Json(json!({"ok":true}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e.to_string()}))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::extract_client_ip;
    use axum::body::Body;
    use axum::extract::Request;
    use std::net::{IpAddr, Ipv4Addr};

    fn req_with_xff(xff: &str) -> Request<Body> {
        Request::builder()
            .header("x-forwarded-for", xff)
            .body(Body::empty())
            .unwrap()
    }

    #[test]
    fn rate_limiter_uses_forwarded_ip() {
        let req = req_with_xff("1.2.3.4, 10.0.0.1");
        let ip = extract_client_ip(&req, 1);
        assert_eq!(ip, "1.2.3.4".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn rate_limiter_ignores_spoofed_xff_when_untrusted() {
        let req = req_with_xff("1.2.3.4, 10.0.0.1");
        let ip = extract_client_ip(&req, 0);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }
}
