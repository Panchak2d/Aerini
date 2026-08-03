//! Multi-workflow REST API server.
//!
//! All routes require:  Authorization: Bearer <token>
//! Except:             GET /api/health  (no auth)
//!                      GET /aerini-widget.js  (no auth — static asset, see routes::widget)
//!                      POST /api/widget/:workflow_id/trigger  (no auth — gated by the
//!                           workflow's own Webhook secret instead, see routes::widget)
//!
//! Routes:
//!   GET    /api/health
//!   GET    /aerini-widget.js
//!   POST   /api/widget/:workflow_id/trigger
//!   GET    /api/workflows
//!   POST   /api/workflows
//!   GET    /api/workflows/:id
//!   DELETE /api/workflows/:id
//!   POST   /api/workflows/:id/run
//!   GET    /api/scheduler
//!   POST   /api/scheduler/:id/start
//!   POST   /api/scheduler/:id/stop
//!   GET    /api/memory
//!   GET    /api/performance/live
//!   GET    /api/performance/reports         (?workflow_id=&limit=&offset=)
//!   DELETE /api/performance/reports         (?workflow_id=)
//!   GET    /api/performance/reports/:run_id
//!   DELETE /api/performance/reports/:run_id
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
use aerini_engine::{
    db::WorkflowDb,
    executor::{CredentialResolver, WorkflowExecutor},
    node::NodeRegistry,
    nodes::register_builtins,
    plugin_loader::load_plugins,
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
use rand::TryRng;

use crate::event_bridge::BroadcastEventSink;
use crate::token_store::TokenStore;
use crate::util::extract_client_ip;

/// Configuration bundle for [`run`]. Groups the 17 server parameters to stay
/// under the clippy `too_many_arguments` limit.
pub struct ServerConfig {
    pub token:                    Option<String>,
    pub port:                     u16,
    pub data_dir:                 String,
    pub allow_origins:            Vec<String>,
    pub allow_env_vars:           Vec<String>,
    pub bind:                     String,
    pub file_sandbox_dir:         Option<std::path::PathBuf>,
    pub trusted_proxy_count:      usize,
    pub shell_exec_disabled:      bool,
    pub code_exec_disabled:       bool,
    pub database_exec_disabled:   bool,
    pub code_sandbox:             bool,
    pub ssrf_firewall_acknowledged: bool,
    pub use_keychain:             bool,
    pub parallel_execution:       bool,
    pub max_concurrent_nodes:     usize,
    pub server_max_duration_secs: Option<u64>,
    pub db_pool_size:             usize,
    pub max_concurrent_runs:      usize,
    pub max_queue_wait_secs:      u64,
    pub max_code_memory_mb:       Option<u64>,
    pub plugin_dir:               Option<PathBuf>,
}

pub use routes::state::ApiState;
pub use routes::state::SSE_MAX_CONNECTIONS;


/// Sets a DACL on `path` that grants full control to the file owner only,
/// mirroring Unix `chmod 0600`. Only compiled on Windows targets.
#[cfg(windows)]
fn set_owner_only_acl(path: &std::path::Path) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Authorization::{
        SetNamedSecurityInfoW, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows_sys::Win32::Security::{
        ACL, InitializeAcl, AddAccessAllowedAce, GetTokenInformation,
        TOKEN_USER, TokenUser, ACL_REVISION,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, CloseHandle};

    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0u16))
        .collect();

    unsafe {
        let mut token: HANDLE = INVALID_HANDLE_VALUE;
        if windows_sys::Win32::Security::OpenProcessToken(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            windows_sys::Win32::Security::TOKEN_QUERY,
            &mut token,
        ) == 0 {
            return Err(format!(
                "OpenProcessToken failed: {}",
                windows_sys::Win32::Foundation::GetLastError()
            ));
        }

        let mut needed: u32 = 0;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);

        if needed == 0 {
            CloseHandle(token);
            return Err("GetTokenInformation returned zero size — unexpected security configuration".to_string());
        }

        let mut buf = vec![0u8; needed as usize];
        if GetTokenInformation(
            token, TokenUser, buf.as_mut_ptr() as *mut _, needed, &mut needed,
        ) == 0 {
            CloseHandle(token);
            return Err(format!(
                "GetTokenInformation (second call) failed: {}",
                windows_sys::Win32::Foundation::GetLastError()
            ));
        }
        CloseHandle(token);

        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let sid = user.User.Sid;

        // Use Vec<u32> to guarantee 4-byte alignment required by ACL header.
        const ACL_BUF_BYTES: usize = 256;
        let mut acl_buf = vec![0u32; ACL_BUF_BYTES / 4];
        if InitializeAcl(acl_buf.as_mut_ptr() as *mut ACL, ACL_BUF_BYTES as u32, ACL_REVISION as u32) == 0 {
            return Err(format!(
                "InitializeAcl failed: {}",
                windows_sys::Win32::Foundation::GetLastError()
            ));
        }
        if AddAccessAllowedAce(
            acl_buf.as_mut_ptr() as *mut ACL, ACL_REVISION as u32, FILE_ALL_ACCESS, sid,
        ) == 0 {
            return Err(format!(
                "AddAccessAllowedAce failed: {}",
                windows_sys::Win32::Foundation::GetLastError()
            ));
        }

        let rc = SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl_buf.as_mut_ptr() as *mut ACL,
            std::ptr::null_mut(),
        );
        if rc != 0 {
            return Err(format!("SetNamedSecurityInfoW failed: {}", rc));
        }
    }
    Ok(())
}

fn load_or_create_token_key(path: &std::path::Path) -> [u8; 32] {
    if path.exists() {
        let bytes = std::fs::read(path).unwrap_or_else(|e| {
            tracing::error!("FATAL: Cannot read token key file {:?}: {}", path, e);
            std::process::exit(1);
        });
        if bytes.len() != 32 {
            tracing::error!(
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
        rand::rngs::SysRng.try_fill_bytes(&mut key).expect("OS RNG failure");
        std::fs::write(path, key).unwrap_or_else(|e| {
            tracing::error!("FATAL: Cannot write token key file {:?}: {}", path, e);
            std::process::exit(1);
        });
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, perms).unwrap_or_else(|e| {
                tracing::error!("FATAL: Cannot set permissions on token key file {:?}: {}", path, e);
                std::process::exit(1);
            });
        }

        #[cfg(windows)]
        set_owner_only_acl(path).unwrap_or_else(|e| {
            tracing::error!("FATAL: Cannot set ACL on token key file {:?}: {}", path, e);
            std::process::exit(1);
        });

        key
    }
}

/// Returns true only for exact localhost origins with an optional port number.
/// Rejects subdomain lookalikes such as `http://localhost.evil.com`.
fn is_localhost_origin(b: &[u8]) -> bool {
    // Exact: http://localhost  or  http://localhost:<digits>
    if b == b"http://localhost" { return true; }
    if let Some(rest) = b.strip_prefix(b"http://localhost:") {
        return !rest.is_empty() && rest.iter().all(|c| c.is_ascii_digit());
    }
    // Exact: http://127.0.0.1  or  http://127.0.0.1:<digits>
    if b == b"http://127.0.0.1" { return true; }
    if let Some(rest) = b.strip_prefix(b"http://127.0.0.1:") {
        return !rest.is_empty() && rest.iter().all(|c| c.is_ascii_digit());
    }
    false
}

pub async fn run(cfg: ServerConfig) {
    let ServerConfig {
        token, port, data_dir, allow_origins, allow_env_vars, bind,
        file_sandbox_dir, trusted_proxy_count, shell_exec_disabled,
        code_exec_disabled, database_exec_disabled, code_sandbox, ssrf_firewall_acknowledged: _,
        use_keychain, parallel_execution,
        max_concurrent_nodes, server_max_duration_secs, db_pool_size,
        max_concurrent_runs, max_queue_wait_secs, max_code_memory_mb,
        plugin_dir,
    } = cfg;
    crate::init_tracing();

    // Enforce minimum token length for user-supplied tokens.
    // Auto-generated tokens are always 43 chars (URL_SAFE_NO_PAD of 32 bytes).
    if let Some(ref t) = token {
        if t.len() < 32 {
            eprintln!("ERROR: AERINI_TOKEN / --token is too short ({} chars). Minimum is 32 characters.", t.len());
            eprintln!("       Generate a strong token with: openssl rand -base64 32");
            std::process::exit(1);
        }
    }
    let data_dir = PathBuf::from(if data_dir.starts_with('~') {
        data_dir.replacen('~',
            &std::env::var("HOME").unwrap_or_else(|_| "/root".to_string()), 1)
    } else {
        data_dir
    });
    std::fs::create_dir_all(&data_dir).expect("Cannot create data dir");

    // Restrict data dir to owner-only on Unix. Default umask typically produces
    // 0755 which makes the key file discoverable even though it is 0600.
    // Setting the directory to 0700 prevents other users from listing its contents.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|e| tracing::warn!(
                "Could not set permissions on data dir {:?}: {}",
                data_dir, e
            ));
    }

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
                rand::rngs::SysRng.try_fill_bytes(&mut raw_bytes).expect("OS RNG failure");
                let generated = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw_bytes);
                eprintln!();
                eprintln!("┌──────────────────────────────────────────────────────────┐");
                eprintln!("│  Aerini Server — API Token (save this somewhere safe)      │");
                eprintln!("│                                                            │");
                eprintln!("│  {}  │", generated);
                eprintln!("│                                                            │");
                eprintln!("│  Set AERINI_TOKEN env var to skip this on restart.         │");
                eprintln!("└──────────────────────────────────────────────────────────┘");
                eprintln!();
                token_store
                    .import_token(&generated, "default", &["read", "write", "admin"])
                    .expect("Cannot import generated token");
            }
        }
    }

    let db = Arc::new(
        WorkflowDb::open(&data_dir.join("aerini.db"), db_pool_size)
            .expect("Cannot open workflow database")
    );
    let creds = Arc::new(
        CredentialStore::open(
            &data_dir.join("credentials.db"),
            if use_keychain {
                KeySource::OsKeychain { fallback: data_dir.join("aerini.key") }
            } else {
                KeySource::File(data_dir.join("aerini.key"))
            },
        ).expect("Cannot open credential store")
    );

    let mut registry = NodeRegistry::new();
    register_builtins(&mut registry, &data_dir, Some(Arc::clone(&db)));
    if let Some(ref dir) = plugin_dir {
        if !dir.exists() {
            tracing::warn!(
                "plugin_dir {:?} does not exist — no plugins loaded",
                dir
            );
        } else {
            load_plugins(&mut registry, dir);
        }
    }
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
            Arc::clone(&db) as Arc<dyn aerini_engine::scheduler::SchedulerDb>,
            Arc::clone(&registry),
            Arc::clone(&resolver) as Arc<dyn aerini_engine::executor::CredentialResolver>,
            Arc::clone(&event_sink) as Arc<dyn EventSink>,
        );
        if let Some(ref allowlist) = env_allowlist {
            daemon = daemon.with_env_allowlist(allowlist.iter().cloned().collect());
        }
        if shell_exec_disabled { daemon = daemon.with_shell_disabled(true); }
        if code_exec_disabled  { daemon = daemon.with_code_disabled(true); }
        if database_exec_disabled { daemon = daemon.with_database_disabled(true); }
        if code_sandbox        { daemon = daemon.with_code_sandbox(true); }
        if max_code_memory_mb.is_some() { daemon = daemon.with_code_max_memory_mb(max_code_memory_mb); }
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
        if database_exec_disabled { ex = ex.with_database_disabled(true); }
        if code_sandbox        { ex = ex.with_code_sandbox(true); }
        if max_code_memory_mb.is_some() { ex = ex.with_code_max_memory_mb(max_code_memory_mb); }
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
        database_exec_disabled,
        parallel_execution,
        max_concurrent_nodes,
        server_max_duration_secs,
        base_executor,
        sse_semaphore: Arc::new(Semaphore::new(SSE_MAX_CONNECTIONS)),
        run_semaphore: Arc::new(Semaphore::new(max_concurrent_runs)),
        queue_timeout: std::time::Duration::from_secs(max_queue_wait_secs),
    };

    let protected = Router::new()
        .route("/api/workflows",           get(routes::workflows::list_workflows).post(routes::workflows::save_workflow))
        .route("/api/workflows/{id}",       get(routes::workflows::get_workflow).delete(routes::workflows::delete_workflow))
        .route("/api/workflows/{id}/run",   post(routes::workflows::run_workflow))
        .route("/api/scheduler",           get(routes::scheduler::list_scheduler))
        .route("/api/scheduler/{id}/start", post(routes::scheduler::start_job))
        .route("/api/scheduler/{id}/stop",  post(routes::scheduler::stop_job))
        .route("/api/memory",              get(routes::memory::get_memory))
        .route("/api/performance/live",     get(routes::performance::get_live))
        .route("/api/performance/reports",  get(routes::performance::list_reports).delete(routes::performance::clear_reports))
        .route("/api/performance/reports/{run_id}", get(routes::performance::get_report).delete(routes::performance::delete_report))
        .route("/api/credentials",         get(routes::credentials::list_creds).post(routes::credentials::save_cred))
        .route("/api/credentials/{id}",     delete(routes::credentials::delete_cred))
        .route("/api/events",              get(routes::workflows::sse_events))
        .route("/api/tokens",              get(routes::tokens::list_tokens_handler).post(routes::tokens::create_token_handler))
        .route("/api/tokens/{id}",          delete(routes::tokens::revoke_token_handler))
        .route("/api/tokens/{id}/workflows",             get(routes::tokens::list_token_workflows_handler))
        .route("/api/tokens/{id}/workflows/{wf_id}",      post(routes::tokens::grant_token_workflow_handler).delete(routes::tokens::revoke_token_workflow_handler))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    let extra_origins: std::sync::Arc<Vec<Vec<u8>>> = std::sync::Arc::new(
        allow_origins.into_iter().map(|o| o.into_bytes()).collect()
    );
    let extra_c = std::sync::Arc::clone(&extra_origins);
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin: &axum::http::HeaderValue, _| {
            let b = origin.as_bytes();
            is_localhost_origin(b)
                || extra_c.iter().any(|o| o.as_slice() == b)
        }))
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([axum::http::header::AUTHORIZATION, axum::http::header::CONTENT_TYPE]);

    let rl = Arc::new(crate::middleware::RateLimiter::new(300, 60));
    Arc::clone(&rl).spawn_eviction_task();
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
        // Unauthenticated by design — see routes::widget module doc.
        // Sits here (not inside `protected`) so neither requires a Bearer
        // token: browsers loading `<script src>` send no Authorization
        // header, and a third-party page embedding the widget cannot hold
        // a server admin/write token.
        .route("/aerini-widget.js", get(routes::widget::serve_widget_js))
        .route("/api/widget/{workflow_id}/trigger", post(routes::widget::trigger_widget))
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
        "Aerini Server started in API mode"
    );
    // HTTP Request nodes perform DNS pre-validation for SSRF, but a TOCTOU gap
    // (DNS rebinding) means application-layer checks are defense-in-depth only.
    // In API mode any token holder with 'write' scope can submit workflows with
    // HTTP Request nodes pointing at attacker-controlled domains.
    // Required: configure a network-level egress firewall that blocks outbound
    // connections to RFC-1918, loopback, link-local, and cloud metadata ranges.
    // See docs/security.md for recommended iptables/nftables rules.
    tracing::warn!(
        "API mode active: HTTP Request nodes are available to all token holders with 'write' scope. \
         The built-in SSRF protection has a DNS rebinding (TOCTOU) gap that cannot be closed at the \
         application layer. Configure a network-level egress firewall to block connections to private, \
         loopback, link-local, and cloud metadata ranges (e.g. 169.254.169.254). \
         See docs/security.md for details."
    );
    eprintln!("Control commands (from another terminal):");
    eprintln!("  aerini-server list    --token YOUR_TOKEN");
    eprintln!("  aerini-server stop    <workflow-name> --token YOUR_TOKEN");
    eprintln!("  aerini-server start   <workflow-name> --token YOUR_TOKEN");
    eprintln!("  aerini-server restart <workflow-name> --token YOUR_TOKEN");
    eprintln!("Token management:");
    eprintln!("  aerini-server tokens list   --token YOUR_TOKEN");
    eprintln!("  aerini-server tokens create --label ci --scopes read,write --token YOUR_TOKEN");

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

    let in_flight = scheduler.active_runs();
    if in_flight > 0 {
        tracing::info!("HTTP drained — waiting for {} in-flight workflow run(s)", in_flight);
    }
    scheduler.drain_all().await;
    tracing::info!("Shutdown complete");
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
