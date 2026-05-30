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

pub(super) mod routes;

use base64::Engine;
use axum::{
    extract::{DefaultBodyLimit, Request},
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
    routing::{delete, get, post},
    Router,
};
use tower_http::cors::AllowOrigin;
use flowo_engine::{
    db::WorkflowDb,
    executor::{CredentialResolver, WorkflowExecutor},
    node::NodeRegistry,
    nodes::register_builtins,
    scheduler::SchedulerDaemon,
    store::{CredentialStore, KeySource, StoreCredentialResolver},
    EventSink,
};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};
use dashmap::DashMap;
use tokio::sync::{broadcast, Semaphore};
use rand::RngCore;
use base64;

use crate::event_bridge::BroadcastEventSink;
use crate::token_store::TokenStore;
use crate::util::extract_client_ip;

pub use routes::state::ApiState;
pub use routes::state::SSE_MAX_CONNECTIONS;

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
    server_max_duration_secs: Option<u64>,
    db_pool_size: usize,
) {
    crate::init_tracing();
    let data_dir = PathBuf::from(if data_dir.starts_with('~') {
        data_dir.replacen('~',
            &std::env::var("HOME").unwrap_or_else(|_| "/root".to_string()), 1)
    } else {
        data_dir
    });
    std::fs::create_dir_all(&data_dir).expect("Cannot create data dir");

    let token_key = load_or_create_token_key(&data_dir.join("tokens.key"));
    let token_store = Arc::new(
        TokenStore::open(&data_dir.join("tokens.db"), token_key)
            .expect("Cannot open token store")
    );

    match &token {
        Some(t) => {
            token_store
                .import_token(t, "default", &["read", "write", "admin"])
                .expect("Cannot import token into token store");
        }
        None => {
            if token_store.is_empty() {
                let mut raw_bytes = [0u8; 32];
                rand::rngs::OsRng.fill_bytes(&mut raw_bytes);
                let generated = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw_bytes);
                eprintln!();
                eprintln!("┌──────────────────────────────────────────────────────────┐");
                eprintln!("│  Flowo Server — API Token (save this somewhere safe)      │");
                eprintln!("│                                                            │");
                eprintln!("│  {}  │", generated);
                eprintln!("│                                                            │");
                eprintln!("│  Set FLOWO_TOKEN env var to skip this on restart.         │");
                eprintln!("└──────────────────────────────────────────────────────────┘");
                eprintln!();
                token_store
                    .import_token(&generated, "default", &["read", "write", "admin"])
                    .expect("Cannot import generated token");
            }
        }
    }

    let db = Arc::new(
        WorkflowDb::open(&data_dir.join("flowo.db"), db_pool_size)
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
        if shell_exec_disabled { daemon = daemon.with_shell_disabled(true); }
        if code_exec_disabled  { daemon = daemon.with_code_disabled(true); }
        if parallel_execution {
            daemon = daemon
                .with_parallel_execution(true)
                .with_max_concurrent_nodes(max_concurrent_nodes);
        }
        if server_max_duration_secs.is_some() {
            daemon = daemon.with_server_max_duration_secs(server_max_duration_secs);
        }
        if let Some(ref sandbox) = file_sandbox_dir {
            daemon = daemon.with_file_sandbox_dir(sandbox.clone());
        }
        Arc::new(daemon)
    };
    scheduler.start(&tokio::runtime::Handle::current());

    let file_sandbox_dir: std::path::PathBuf = match file_sandbox_dir {
        Some(d) => { tracing::info!("File node sandbox directory: {:?}", d); d }
        None => {
            let default = data_dir.join("files");
            std::fs::create_dir_all(&default)
                .expect("Cannot create default file sandbox directory");
            tracing::info!(
                "File node sandbox not set — defaulting to {:?}. \
                 Pass --file-sandbox-dir to use a different path.",
                default
            );
            default
        }
    };

    let base_executor = {
        let res  = Arc::new(StoreCredentialResolver { store: Arc::clone(&creds) });
        let sink = Arc::new(BroadcastEventSink { tx: sse_tx.clone() });
        let mut ex = WorkflowExecutor::new(
            Arc::clone(&registry),
            Arc::clone(&res) as Arc<dyn CredentialResolver>,
        ).with_event_sink(Arc::clone(&sink) as Arc<dyn EventSink>)
         .with_file_sandbox_dir(file_sandbox_dir.clone());
        if let Some(ref allowlist) = env_allowlist {
            ex = ex.with_env_allowlist(allowlist.iter().cloned().collect());
        }
        if shell_exec_disabled { ex = ex.with_shell_disabled(true); }
        if code_exec_disabled  { ex = ex.with_code_disabled(true); }
        if parallel_execution {
            ex = ex.with_parallel_execution(true)
                   .with_max_concurrent_nodes(max_concurrent_nodes);
        }
        if server_max_duration_secs.is_some() {
            ex = ex.with_server_max_duration_secs(server_max_duration_secs);
        }
        ex
    };

    let state = ApiState {
        db:               Arc::clone(&db),
        scheduler:        Arc::clone(&scheduler),
        creds:            Arc::clone(&creds),
        registry:         Arc::clone(&registry),
        sse_tx:           sse_tx.clone(),
        token_store:      Arc::clone(&token_store),
        exec_locks:       Arc::new(DashMap::new()),
        env_allowlist:    env_allowlist.clone(),
        file_sandbox_dir: Some(Arc::new(file_sandbox_dir)),
        shell_exec_disabled,
        code_exec_disabled,
        parallel_execution,
        max_concurrent_nodes,
        server_max_duration_secs,
        base_executor,
        sse_semaphore: Arc::new(Semaphore::new(SSE_MAX_CONNECTIONS)),
    };

    let protected = Router::new()
        .route("/api/workflows",           get(routes::workflows::list_workflows).post(routes::workflows::save_workflow))
        .route("/api/workflows/:id",       get(routes::workflows::get_workflow).delete(routes::workflows::delete_workflow))
        .route("/api/workflows/:id/run",   post(routes::workflows::run_workflow))
        .route("/api/scheduler",           get(routes::scheduler::list_scheduler))
        .route("/api/scheduler/:id/start", post(routes::scheduler::start_job))
        .route("/api/scheduler/:id/stop",  post(routes::scheduler::stop_job))
        .route("/api/credentials",         get(routes::credentials::list_creds).post(routes::credentials::save_cred))
        .route("/api/credentials/:id",     delete(routes::credentials::delete_cred))
        .route("/api/events",              get(routes::workflows::sse_events))
        .route("/api/tokens",              get(routes::tokens::list_tokens_handler).post(routes::tokens::create_token_handler))
        .route("/api/tokens/:id",          delete(routes::tokens::revoke_token_handler))
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
        .allow_headers([axum::http::header::AUTHORIZATION, axum::http::header::CONTENT_TYPE]);

    let rl = Arc::new(crate::middleware::RateLimiter::new(300, 60));
    let tpc = trusted_proxy_count;
    let rate_limit_layer = middleware::from_fn(move |req: Request, next: Next| {
        let rl = Arc::clone(&rl);
        async move {
            let ip: IpAddr = extract_client_ip(&req, tpc);
            if rl.is_allowed(ip) {
                next.run(req).await
            } else {
                StatusCode::TOO_MANY_REQUESTS.into_response()
            }
        }
    });

    let app = Router::new()
        .route("/api/health", get(routes::workflows::health))
        .merge(protected)
        .with_state(state)
        .layer(DefaultBodyLimit::max(5 * 1024 * 1024))
        .layer(cors)
        .layer(rate_limit_layer)
        .layer(middleware::from_fn(crate::middleware::api_security_headers));

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

    let shutdown = async {
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
        tracing::info!("Shutdown signal received — draining in-flight requests (10s max)");
    };

    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown)
        .await
        .unwrap_or_else(|e| tracing::error!("Server error: {}", e));

    scheduler.stop_all();
}

async fn auth_middleware(
    axum::extract::State(s): axum::extract::State<ApiState>,
    headers:     axum::http::HeaderMap,
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
                axum::Json(serde_json::json!({"error":"Invalid or missing token"}))).into_response()
        }
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
