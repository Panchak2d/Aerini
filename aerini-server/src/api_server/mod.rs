//! Multi-workflow REST API server.
//!
//! All routes require:  Authorization: Bearer <token>
//! Except:             GET /api/health  (no auth)
//!                      GET /aerini-widget.js  (no auth — static asset, see routes::widget)
//!                      POST /api/widget/:workflow_id/trigger  (no auth — gated by the
//!                           workflow's own Webhook secret instead, see routes::widget)
//!                      POST /api/widget/:workflow_id/mint-token  (no Bearer auth — gated
//!                           by the workflow's own Webhook secret; caller is the workflow
//!                           author's own backend, see routes::widget)
//!
//! Routes:
//!   GET    /api/health
//!   GET    /aerini-widget.js
//!   POST   /api/widget/:workflow_id/trigger
//!   POST   /api/widget/:workflow_id/mint-token
//!   GET    /api/workflows
//!   POST   /api/workflows           (optional `If-Match: "<row_version>"` for
//!                                    optimistic-concurrency protection — 409
//!                                    on a stale version, 412 if If-Match is
//!                                    given but the workflow doesn't exist)
//!   GET    /api/workflows/:id       (returns `ETag: "<row_version>"`)
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
//!   POST   /api/plugins/reload  (admin scope required)
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
    node::{NodeRegistry, Reloadable},
    nodes::register_builtins,
    plugin_loader::{load_plugins, PluginLoadReport},
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
use tokio_util::sync::CancellationToken;
use rand::TryRng;

use crate::event_bridge::BroadcastEventSink;
use crate::token_store::TokenStore;
use crate::util::{extract_client_ip, parse_bearer};

/// Configuration bundle for [`run`]. Groups the server parameters to stay
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
    use windows_sys::Win32::Security::Authorization::{SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACL, InitializeAcl, AddAccessAllowedAce, GetTokenInformation,
        TOKEN_USER, TokenUser, ACL_REVISION, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, CloseHandle};

    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0u16))
        .collect();

    unsafe {
        let mut token: HANDLE = INVALID_HANDLE_VALUE;
        if windows_sys::Win32::System::Threading::OpenProcessToken(
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

/// Logs `msg` and exits with status 1. Startup failures are operator errors,
/// so they get one clear line instead of a panic backtrace.
fn fatal(msg: impl std::fmt::Display) -> ! {
    tracing::error!("FATAL: {msg}");
    std::process::exit(1);
}

fn read_token_key(path: &std::path::Path) -> [u8; 32] {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| fatal(format_args!("Cannot read token key file {path:?}: {e}")));
    let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) else {
        fatal(format_args!(
            "Token key file {path:?} has wrong length ({} bytes, expected 32). \
             If you intentionally want to invalidate all tokens, delete the file and restart.",
            bytes.len()
        ));
    };
    key
}

fn load_or_create_token_key(path: &std::path::Path) -> [u8; 32] {
    if path.exists() {
        return read_token_key(path);
    }
    let mut key = [0u8; 32];
    rand::rngs::SysRng.try_fill_bytes(&mut key).expect("OS RNG failure");

    // `create_new` makes creation atomic, and on Unix the file is born 0600,
    // so the key is never readable by other users, even briefly.
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    match opts.open(path) {
        Ok(mut file) => {
            use std::io::Write;
            if let Err(e) = file.write_all(&key).and_then(|()| file.sync_all()) {
                drop(file);
                let _ = std::fs::remove_file(path);
                fatal(format_args!("Cannot write token key file {path:?}: {e}"));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return read_token_key(path),
        Err(e) => fatal(format_args!("Cannot create token key file {path:?}: {e}")),
    }

    #[cfg(windows)]
    {
        if let Err(e) = set_owner_only_acl(path) {
            let _ = std::fs::remove_file(path);
            fatal(format_args!("Cannot set ACL on token key file {path:?}: {e}"));
        }
    }

    key
}

/// Expands a leading `~` (alone, or followed by a path separator) to `home`.
/// `~user/...` is left as written: resolving another user's home is not
/// supported, and prefixing it with our own would point at the wrong place.
fn expand_home(path: &str, home: Option<&std::path::Path>) -> Result<PathBuf, String> {
    let rest = if path == "~" {
        ""
    } else if let Some(r) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        r
    } else {
        return Ok(PathBuf::from(path));
    };
    match home {
        Some(h) => Ok(h.join(rest)),
        None => Err(format!(
            "Cannot expand '~' in '{path}': neither HOME nor USERPROFILE is set. \
             Pass an absolute --data-dir."
        )),
    }
}

fn home_dir() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"].iter()
        .filter_map(|k| std::env::var_os(k))
        .find(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Returns true only for exact loopback origins with an optional port number.
/// Rejects subdomain lookalikes such as `http://localhost.evil.com`.
fn is_localhost_origin(b: &[u8]) -> bool {
    const HOSTS: [&[u8]; 3] = [b"http://localhost", b"http://127.0.0.1", b"http://[::1]"];
    HOSTS.iter().any(|host| match b.strip_prefix(*host) {
        Some([]) => true,
        Some([b':', port @ ..]) => !port.is_empty() && port.iter().all(u8::is_ascii_digit),
        _ => false,
    })
}

/// True for `/api/widget/{workflow_id}/mint-token`. Its caller is the embedding
/// site's own backend, which sends no `Origin`, so browsers are never granted
/// cross-origin access to it.
fn is_mint_token_path(path: &str) -> bool {
    path.strip_prefix("/api/widget/")
        .and_then(|rest| rest.strip_suffix("/mint-token"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

fn cors_origin_allowed(origin: &[u8], path: &str, extra_origins: &[Vec<u8>]) -> bool {
    if is_mint_token_path(path) {
        return false;
    }
    is_localhost_origin(origin) || extra_origins.iter().any(|o| o.as_slice() == origin)
}

/// Upper bound on how long shutdown waits for open HTTP connections to close.
const SHUTDOWN_DRAIN_SECS: u64 = 10;

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
    let data_dir = expand_home(&data_dir, home_dir().as_deref()).unwrap_or_else(|e| fatal(e));
    std::fs::create_dir_all(&data_dir)
        .unwrap_or_else(|e| fatal(format_args!("Cannot create data dir {data_dir:?}: {e}")));

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
            .unwrap_or_else(|e| fatal(format_args!("Cannot open token store: {e}")))
    );

    match &token {
        Some(t) => {
            token_store
                .import_token(t, "default", &["read", "write", "admin"])
                .unwrap_or_else(|e| fatal(format_args!("Cannot import token into token store: {e}")));
        }
        None => {
            if token_store.is_empty() {
                let mut raw_bytes = [0u8; 32];
                rand::rngs::SysRng.try_fill_bytes(&mut raw_bytes).expect("OS RNG failure");
                let generated = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw_bytes);
                let rule = "─".repeat(58);
                eprintln!();
                eprintln!("┌{rule}┐");
                eprintln!("│  {:<54}  │", "Aerini Server — API Token (save this somewhere safe)");
                eprintln!("│  {:<54}  │", "");
                eprintln!("│  {:<54}  │", generated);
                eprintln!("│  {:<54}  │", "");
                eprintln!("│  {:<54}  │", "Set AERINI_TOKEN env var to skip this on restart.");
                eprintln!("└{rule}┘");
                eprintln!();
                token_store
                    .import_token(&generated, "default", &["read", "write", "admin"])
                    .unwrap_or_else(|e| fatal(format_args!("Cannot import generated token: {e}")));
            }
        }
    }

    let db = Arc::new(
        WorkflowDb::open(&data_dir.join("aerini.db"), db_pool_size)
            .unwrap_or_else(|e| fatal(format_args!("Cannot open workflow database: {e}")))
    );
    // Spawned onto a blocking-pool thread: with keyring's async-secret-service
    // backend, the OsKeychain path makes a blocking D-Bus round trip, and
    // keyring's own docs warn that calling it directly on a thread already
    // driving a Tokio runtime (this fn runs under #[tokio::main]) can
    // deadlock or panic the runtime. See keyring::secret_service module docs,
    // "Tokio runtime caution".
    let creds_db_path = data_dir.join("credentials.db");
    let creds_key_source = if use_keychain {
        KeySource::OsKeychain { fallback: data_dir.join("aerini.key") }
    } else {
        KeySource::File(data_dir.join("aerini.key"))
    };
    let creds = Arc::new(
        tokio::task::spawn_blocking(move || CredentialStore::open(&creds_db_path, creds_key_source))
            .await
            .unwrap_or_else(|e| fatal(format_args!("Credential store init task failed: {e}")))
            .unwrap_or_else(|e| fatal(format_args!("Cannot open credential store: {e}")))
    );

    let mut registry = NodeRegistry::new();
    register_builtins(&mut registry, &data_dir, Some(Arc::clone(&db)));
    let startup_load_report = match plugin_dir {
        Some(ref dir) if dir.exists() => load_plugins(&mut registry, dir),
        Some(ref dir) => {
            tracing::warn!(
                "plugin_dir {:?} does not exist — no plugins loaded",
                dir
            );
            PluginLoadReport::default()
        }
        None => PluginLoadReport::default(),
    };
    let registry = Arc::new(Reloadable::new(registry));
    let last_load_report = Arc::new(tokio::sync::RwLock::new(startup_load_report));
    let reload_lock = Arc::new(tokio::sync::Mutex::new(()));

    let (sse_tx, _) = broadcast::channel::<String>(256);
    let shutdown_token = CancellationToken::new();
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
        if let Some(ref dir) = plugin_dir {
            daemon = daemon.with_plugin_dir(dir.clone());
        }
        Arc::new(daemon)
    };
    scheduler.start(&tokio::runtime::Handle::current());

    let file_sandbox_dir: std::path::PathBuf = match file_sandbox_dir {
        Some(d) => { tracing::info!("File node sandbox directory: {:?}", d); d }
        None => {
            let default = data_dir.join("files");
            std::fs::create_dir_all(&default).unwrap_or_else(|e| fatal(format_args!(
                "Cannot create default file sandbox directory {default:?}: {e}"
            )));
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
        // Snapshot only — `base_executor` is a template `.clone()`-d once per
        // request (see routes/workflows.rs), and each of those clones calls
        // `.with_registry(state.registry.current())` to pick up the latest
        // reload; the snapshot baked in here only matters for a request that
        // races the very first reload before that call.
        let mut ex = WorkflowExecutor::new(
            registry.current(),
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
        reload_lock:      Arc::clone(&reload_lock),
        last_load_report: Arc::clone(&last_load_report),
        data_dir:         Arc::new(data_dir),
        plugin_dir:       plugin_dir.map(Arc::new),
        sse_tx:           sse_tx.clone(),
        shutdown:         shutdown_token.clone(),
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
        .route("/api/plugins/reload",      post(routes::plugins::reload_plugins))
        .route("/api/plugins/load-report", get(routes::plugins::load_report))
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
        .allow_origin(AllowOrigin::predicate(move |origin: &axum::http::HeaderValue, parts| {
            cors_origin_allowed(origin.as_bytes(), parts.uri.path(), &extra_c)
        }))
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([axum::http::header::AUTHORIZATION, axum::http::header::CONTENT_TYPE, axum::http::header::IF_MATCH])
        .expose_headers([axum::http::header::ETAG, axum::http::header::RETRY_AFTER])
        .max_age(std::time::Duration::from_secs(3600));

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
                crate::middleware::too_many_requests()
            }
        }
    });

    // Widget-specific rate limiting, stricter than the blanket 300/60s above.
    // Both public widget routes are gated only by the workflow's own Webhook
    // secret (routes::widget module doc) rather than a server Bearer token,
    // so unlike every other route here, a leaked/guessed credential lets an
    // arbitrary internet caller reach them at will. Two axes:
    //   - per-IP:       bounds a single leaked credential used from one place.
    //   - per-workflow: bounds total cost/abuse regardless of how many
    //                   different IPs a leaked credential gets used from
    //                   (VPN rotation, botnet) — the blast radius that
    //                   matters to the workflow owner is "how many times did
    //                   MY workflow run", not "from how many IPs".
    // mint-token has no per-workflow axis: it never runs the workflow, only
    // checks a secret, so the per-IP guess-throttle is the relevant bound.
    let widget_trigger_ip_rl = Arc::new(crate::middleware::RateLimiter::new(20, 60));
    Arc::clone(&widget_trigger_ip_rl).spawn_eviction_task();
    let widget_trigger_wf_rl = Arc::new(crate::middleware::KeyedRateLimiter::new(60, 60));
    Arc::clone(&widget_trigger_wf_rl).spawn_eviction_task();
    let tpc_wt = trusted_proxy_count;
    let widget_trigger_rate_limit = middleware::from_fn(
        move |axum::extract::Path(workflow_id): axum::extract::Path<String>, req: Request, next: Next| {
            let ip_rl = Arc::clone(&widget_trigger_ip_rl);
            let wf_rl = Arc::clone(&widget_trigger_wf_rl);
            async move {
                let ip: IpAddr = extract_client_ip(&req, tpc_wt);
                if !ip_rl.is_allowed(ip) || !wf_rl.is_allowed(&workflow_id) {
                    return crate::middleware::too_many_requests();
                }
                next.run(req).await
            }
        },
    );

    let widget_mint_ip_rl = Arc::new(crate::middleware::RateLimiter::new(10, 60));
    Arc::clone(&widget_mint_ip_rl).spawn_eviction_task();
    let tpc_wm = trusted_proxy_count;
    let widget_mint_rate_limit = middleware::from_fn(move |req: Request, next: Next| {
        let ip_rl = Arc::clone(&widget_mint_ip_rl);
        async move {
            let ip: IpAddr = extract_client_ip(&req, tpc_wm);
            if !ip_rl.is_allowed(ip) {
                return crate::middleware::too_many_requests();
            }
            next.run(req).await
        }
    });

    // Two separately-layered routers, merged — not one router with two
    // `.route().layer()` pairs chained in sequence. `Router::layer` wraps
    // every route already present at the time it's called, not just the one
    // added immediately before it (confirmed against axum 0.8's own
    // `Router::layer` doc example, which uses this same merge pattern for
    // exactly this reason). Chaining them directly would apply
    // `widget_mint_rate_limit` on top of `trigger` as well, silently
    // dropping its effective limit to mint-token's stricter 10/min/IP
    // instead of the documented 20/min/IP + 60/min/workflow.
    let trigger_router = Router::new()
        .route("/api/widget/{workflow_id}/trigger", post(routes::widget::trigger_widget))
        .layer(widget_trigger_rate_limit);
    let mint_router = Router::new()
        .route("/api/widget/{workflow_id}/mint-token", post(routes::widget::mint_widget_token))
        .layer(widget_mint_rate_limit);
    let widget_routes = trigger_router.merge(mint_router);

    let app = Router::new()
        .route("/api/health", get(routes::workflows::health))
        // Unauthenticated by design — see routes::widget module doc.
        // Sits here (not inside `protected`) so neither requires a Bearer
        // token: browsers loading `<script src>` send no Authorization
        // header, and a third-party page embedding the widget cannot hold
        // a server admin/write token.
        .route("/aerini-widget.js", get(routes::widget::serve_widget_js))
        .merge(widget_routes)
        .merge(protected)
        .with_state(state)
        .layer(DefaultBodyLimit::max(5 * 1024 * 1024))
        // `cors` must wrap `rate_limit_layer`: a 429 built inside it still
        // gets CORS headers. `cors` answers every OPTIONS itself, so preflights
        // never reach the limiter; `max_age` keeps browsers from sending many.
        .layer(rate_limit_layer)
        .layer(cors)
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

    let (signal_tx, signal_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown = async move {
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
        tracing::info!("Shutdown signal received — draining in-flight requests ({}s max)", SHUTDOWN_DRAIN_SECS);
        shutdown_token.cancel();
        let _ = signal_tx.send(());
    };

    // Graceful shutdown waits for every open connection, and an SSE stream
    // never finishes on its own. The deadline is armed only once the signal
    // fires, so it bounds the drain and never a healthy running server.
    let drain_deadline = async move {
        if signal_rx.await.is_ok() {
            tokio::time::sleep(std::time::Duration::from_secs(SHUTDOWN_DRAIN_SECS)).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    let serve = std::future::IntoFuture::into_future(
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(shutdown),
    );
    tokio::select! {
        res = serve => res.unwrap_or_else(|e| tracing::error!("Server error: {}", e)),
        _ = drain_deadline => tracing::warn!(
            "HTTP drain exceeded {}s (open SSE streams?) — closing remaining connections",
            SHUTDOWN_DRAIN_SECS
        ),
    }

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
        .and_then(parse_bearer)
        .unwrap_or("");

    match s.verify_token(provided).await {
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
    use super::{cors_origin_allowed, expand_home, extract_client_ip, is_mint_token_path, parse_bearer};
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
        let req = req_with_xff("1.2.3.4");
        let ip = extract_client_ip(&req, 1);
        assert_eq!(ip, "1.2.3.4".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn client_supplied_leading_xff_entries_are_ignored() {
        let req = req_with_xff("9.9.9.9, 1.2.3.4");
        assert_eq!(extract_client_ip(&req, 1), "1.2.3.4".parse::<IpAddr>().unwrap());
        let req = req_with_xff("9.9.9.9, 1.2.3.4, 10.0.0.1");
        assert_eq!(extract_client_ip(&req, 2), "1.2.3.4".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn repeated_xff_header_lines_are_read_as_one_list() {
        let req = Request::builder()
            .header("x-forwarded-for", "9.9.9.9")
            .header("x-forwarded-for", "1.2.3.4")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_client_ip(&req, 1), "1.2.3.4".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn fewer_xff_entries_than_trusted_hops_falls_back_to_tcp_ip() {
        let req = req_with_xff("1.2.3.4");
        assert_eq!(extract_client_ip(&req, 2), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn rate_limiter_ignores_spoofed_xff_when_untrusted() {
        let req = req_with_xff("1.2.3.4, 10.0.0.1");
        let ip = extract_client_ip(&req, 0);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        assert_eq!(parse_bearer("Bearer abc"), Some("abc"));
        assert_eq!(parse_bearer("bearer abc"), Some("abc"));
        assert_eq!(parse_bearer("BEARER abc"), Some("abc"));
    }

    #[test]
    fn non_bearer_or_malformed_authorization_is_rejected() {
        assert_eq!(parse_bearer("Basic abc"), None);
        assert_eq!(parse_bearer("Bearer"), None);
        assert_eq!(parse_bearer("Bearer "), None);
    }

    #[test]
    fn extra_spaces_after_the_bearer_scheme_are_skipped() {
        assert_eq!(parse_bearer("Bearer   abc"), Some("abc"));
    }

    #[test]
    fn mint_token_path_matches_only_the_mint_route() {
        assert!(is_mint_token_path("/api/widget/wf_abc123/mint-token"));
        assert!(!is_mint_token_path("/api/widget/wf_abc123/trigger"));
        assert!(!is_mint_token_path("/api/widget//mint-token"));
        assert!(!is_mint_token_path("/api/widget/a/b/mint-token"));
        assert!(!is_mint_token_path("/api/workflows/wf_abc123/mint-token"));
    }

    #[test]
    fn cors_refuses_every_browser_origin_on_mint_token() {
        let extra = vec![b"https://site.example".to_vec()];
        let path = "/api/widget/wf_abc123/mint-token";
        assert!(!cors_origin_allowed(b"http://localhost:3000", path, &extra));
        assert!(!cors_origin_allowed(b"https://site.example", path, &extra));
    }

    #[test]
    fn cors_allows_localhost_and_listed_origins_elsewhere() {
        let extra = vec![b"https://site.example".to_vec()];
        let path = "/api/widget/wf_abc123/trigger";
        assert!(cors_origin_allowed(b"http://localhost:3000", path, &extra));
        assert!(cors_origin_allowed(b"http://[::1]:3000", path, &extra));
        assert!(cors_origin_allowed(b"http://[::1]", path, &extra));
        assert!(!cors_origin_allowed(b"http://[::1]:", path, &extra));
        assert!(!cors_origin_allowed(b"http://[::1].evil.com", path, &extra));
        assert!(cors_origin_allowed(b"https://site.example", path, &extra));
        assert!(!cors_origin_allowed(b"https://other.example", path, &extra));
    }

    #[test]
    fn tilde_expands_only_as_a_whole_leading_component() {
        let home = std::path::Path::new("/home/me");
        assert_eq!(expand_home("~", Some(home)).unwrap(), home);
        assert_eq!(expand_home("~/.aerini-server", Some(home)).unwrap(), home.join(".aerini-server"));
        assert_eq!(expand_home("~other/x", Some(home)).unwrap(), std::path::PathBuf::from("~other/x"));
        assert_eq!(expand_home("/data/a~b", Some(home)).unwrap(), std::path::PathBuf::from("/data/a~b"));
        assert!(expand_home("~/x", None).is_err());
        assert!(expand_home("/abs", None).is_ok());
    }
}
