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
//!   GET    /api/plugins/load-report  (admin scope required)
//!   GET    /api/events   (SSE)
//!   GET    /api/tokens   (admin scope required)
//!   POST   /api/tokens   (admin scope required)
//!   DELETE /api/tokens/:id  (admin scope required)
//!   GET    /api/tokens/:id/workflows  (admin scope required)
//!   POST   /api/tokens/:id/workflows/:wf_id  (admin scope required)
//!   DELETE /api/tokens/:id/workflows/:wf_id  (admin scope required)

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
    executor::{CredentialResolver, SecretString, WorkflowExecutor},
    node::{NodeRegistry, Reloadable},
    nodes::register_builtins,
    plugin_loader::{load_plugins, PluginLoadReport},
    scheduler::SchedulerDaemon,
    store::{CredentialStore, KeySource, StoreCredentialResolver, MAX_VALUE_BYTES},
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
use crate::token_store::{ImportOutcome, TokenStore};
use crate::util::{extract_client_ip, join_host_port, parse_bearer};

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


/// Prints one error line and exits. Used for failures during startup, where a
/// panic backtrace would only bury the cause.
fn fatal(msg: &str) -> ! {
    eprintln!("ERROR: {msg}");
    std::process::exit(1);
}

fn read_token_key(path: &std::path::Path) -> [u8; 32] {
    aerini_engine::store::tighten_key_file_permissions(path);
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| fatal(&format!("cannot read token key file {}: {e}", path.display())));
    if bytes.len() != 32 {
        fatal(&format!(
            "token key file {} has wrong length ({} bytes, expected 32). \
             If you intentionally want to invalidate all tokens, delete the file and restart.",
            path.display(),
            bytes.len()
        ));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    key
}

#[cfg(unix)]
fn is_group_or_world_accessible(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(dir).is_ok_and(|m| m.permissions().mode() & 0o077 != 0)
}

#[cfg(not(unix))]
fn is_group_or_world_accessible(_dir: &std::path::Path) -> bool {
    false
}

fn load_or_create_token_key(path: &std::path::Path) -> [u8; 32] {
    if path.exists() {
        return read_token_key(path);
    }
    let _lock = aerini_engine::store::lock_beside(path).unwrap_or_else(|e| {
        fatal(&format!("cannot lock token key file {}: {e}", path.display()))
    });
    if path.exists() {
        return read_token_key(path);
    }

    let mut key = [0u8; 32];
    rand::rngs::SysRng
        .try_fill_bytes(&mut key)
        .unwrap_or_else(|_| fatal("the operating system's random number generator failed"));
    aerini_engine::store::publish_secret_file(path, &key).unwrap_or_else(|e| {
        fatal(&format!("cannot write token key file {}: {e}", path.display()))
    });
    key
}

/// The current user's home directory: `HOME`, or on Windows `USERPROFILE`
/// when `HOME` is unset. An empty value counts as unset.
fn home_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    var("HOME").or_else(|| if cfg!(windows) { var("USERPROFILE") } else { None })
}

/// Expands `~` and `~/...` against `home`. Anything else, including `~name/...`,
/// is returned as written and needs no home directory.
fn expand_home(raw: &str, home: Option<&std::path::Path>) -> Result<PathBuf, String> {
    let rest = if raw == "~" {
        ""
    } else if let Some(rest) = raw.strip_prefix("~/") {
        rest
    } else {
        return Ok(PathBuf::from(raw));
    };
    let Some(home) = home else {
        return Err(format!(
            "cannot expand `~` in --data-dir {raw:?}: no home directory found. \
             Set {} or pass an absolute --data-dir.",
            if cfg!(windows) { "HOME or USERPROFILE" } else { "HOME" },
        ));
    };
    // `join` with a rooted argument would discard `home`, so `~//etc` must not become `/etc`.
    let rest = rest.trim_start_matches(std::path::is_separator);
    Ok(if rest.is_empty() { home.to_path_buf() } else { home.join(rest) })
}

/// Longest backup key `restore_key` reads. A key is 44 base64 characters; the
/// headroom is for whitespace around it.
const MAX_BACKUP_KEY_BYTES: u64 = 1024;

/// Reads a backup key from `reader`: UTF-8 text, at most `MAX_BACKUP_KEY_BYTES`,
/// not blank. The text is returned as read; the engine trims it.
fn read_backup_key(reader: impl std::io::Read) -> Result<SecretString, String> {
    use std::io::Read;
    let mut text = SecretString::new(String::new());
    reader
        .take(MAX_BACKUP_KEY_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::InvalidData => "the backup key must be plain text".to_string(),
            _ => format!("cannot read the backup key: {e}"),
        })?;
    if text.len() as u64 > MAX_BACKUP_KEY_BYTES {
        return Err(format!(
            "that is not a backup key: it is longer than {MAX_BACKUP_KEY_BYTES} bytes"
        ));
    }
    if text.trim().is_empty() {
        return Err("the backup key is empty".to_string());
    }
    Ok(text)
}

/// `aerini-server restore-key`: puts a backed-up credential encryption key back
/// in `data_dir`, after checking that it decrypts that folder's credentials.
/// Run it while the server is stopped.
pub async fn restore_key(data_dir: &str, key_file: Option<&std::path::Path>, keychain: bool) {
    use std::io::IsTerminal;
    crate::init_tracing();
    let data_dir = expand_home(data_dir, home_dir().as_deref()).unwrap_or_else(|e| fatal(&e));
    let db_path = data_dir.join("credentials.db");
    let fallback = data_dir.join("aerini.key");
    let key_source = if keychain {
        KeySource::OsKeychain { fallback }
    } else {
        KeySource::File(fallback)
    };

    let backup = match key_file {
        Some(path) => std::fs::File::open(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))
            .and_then(read_backup_key),
        None if std::io::stdin().is_terminal() => Err(
            "pass the backup key with --key-file, or pipe it in; a key typed at a terminal stays on screen"
                .to_string(),
        ),
        None => read_backup_key(std::io::stdin().lock()),
    }
    .unwrap_or_else(|e| fatal(&e));

    let outcome = tokio::task::spawn_blocking(move || {
        CredentialStore::restore_key_base64(&db_path, key_source, backup.trim())
    })
    .await;
    match outcome {
        Ok(Ok(())) => println!("Key restored. Start the server again."),
        Ok(Err(e)) => fatal(&e.to_string()),
        Err(e) => fatal(&format!("the restore task failed: {e}")),
    }
}

/// The first-run banner around the generated API token. Every line has the
/// same width, so the right border lines up.
fn token_banner(token: &str) -> String {
    const INNER: usize = 60;
    let bar = "─".repeat(INNER);
    let row = |text: &str| format!("│{:<INNER$}│", format!("  {text}"));
    [
        format!("┌{bar}┐"),
        row("Aerini Server — API Token (save this somewhere safe)"),
        row(""),
        row(token),
        row(""),
        row("Set AERINI_TOKEN env var to skip this on restart."),
        format!("└{bar}┘"),
    ]
    .join("\n")
}

/// Returns true only for exact loopback origins with an optional port number:
/// `http://localhost`, `http://127.0.0.1` and `http://[::1]`.
/// Rejects subdomain lookalikes such as `http://localhost.evil.com`.
fn is_localhost_origin(b: &[u8]) -> bool {
    [&b"http://localhost"[..], b"http://127.0.0.1", b"http://[::1]"]
        .iter()
        .any(|host| match b.strip_prefix(*host) {
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

/// Largest request body the public widget routes accept: the 1,000,000-byte
/// cap on what the Webhook listener takes, plus room for the JSON wrapper
/// around it.
const WIDGET_BODY_LIMIT_BYTES: usize = 1024 * 1024;

/// Largest request body the credential routes accept: twice the stored-value
/// cap (JSON escapes a quote or newline as two bytes), plus 16 KiB for the
/// other fields and their JSON syntax.
const CREDENTIAL_BODY_LIMIT_BYTES: usize = 2 * MAX_VALUE_BYTES + 16 * 1024;

/// True when the server listens beyond loopback with no proxy count set: a
/// reverse proxy in front would then make every client share one rate-limit
/// bucket, because each request appears to come from the proxy's address.
fn needs_proxy_count_notice(bind: &str, trusted_proxy_count: usize) -> bool {
    trusted_proxy_count == 0 && !matches!(bind, "127.0.0.1" | "::1" | "localhost")
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
    let token = token.map(SecretString::new);

    // Enforce minimum token length for user-supplied tokens.
    // Auto-generated tokens are always 43 chars (URL_SAFE_NO_PAD of 32 bytes).
    if let Some(ref t) = token {
        if t.len() < 32 {
            eprintln!("ERROR: AERINI_TOKEN / --token is too short ({} chars). Minimum is 32 characters.", t.len());
            eprintln!("       Generate a strong token with: openssl rand -base64 32");
            std::process::exit(1);
        }
    }
    let data_dir = expand_home(&data_dir, home_dir().as_deref()).unwrap_or_else(|e| fatal(&e));
    let data_dir_existed = data_dir.is_dir();
    aerini_engine::store::create_private_dir_all(&data_dir).unwrap_or_else(|e| {
        fatal(&format!("cannot create data directory {}: {e}", data_dir.display()))
    });
    if data_dir_existed && is_group_or_world_accessible(&data_dir) {
        tracing::warn!(
            "Data directory {:?} is accessible to group or other users. It holds the token \
             database, the token key and the credential store. Restrict it with: chmod 700 {:?}",
            data_dir, data_dir
        );
    }

    let token_key = load_or_create_token_key(&data_dir.join("tokens.key"));
    let tokens_db = data_dir.join("tokens.db");
    let token_store = Arc::new(
        TokenStore::open(&tokens_db, token_key).unwrap_or_else(|e| {
            fatal(&format!("cannot open token store {}: {e}", tokens_db.display()))
        })
    );

    let store_was_empty = token_store.is_empty()
        .unwrap_or_else(|e| fatal(&format!("cannot read token store {}: {e}", tokens_db.display())));
    match &token {
        Some(t) => {
            match token_store
                .import_token(t, "default", &["read", "write", "admin"])
                .unwrap_or_else(|e| fatal(&format!("cannot import token into token store: {e}")))
            {
                ImportOutcome::Added if store_was_empty => {}
                ImportOutcome::Added => tracing::warn!(
                    "--token / AERINI_TOKEN is not a token this server already knows, so it was added \
                     as a new admin token. Unrevoked tokens already stored in {} stay active. List them with \
                     `aerini-server tokens list` and revoke any you no longer want with \
                     `aerini-server tokens revoke`.",
                    tokens_db.display()
                ),
                ImportOutcome::AlreadyActive => {}
                ImportOutcome::Revoked => fatal(
                    "the token given by --token / AERINI_TOKEN has been revoked. Start without it to \
                     use the tokens already stored, or give a new token (openssl rand -base64 32).",
                ),
            }
        }
        None => {
            if store_was_empty {
                let mut raw_bytes = [0u8; 32];
                rand::rngs::SysRng
                    .try_fill_bytes(&mut raw_bytes)
                    .unwrap_or_else(|_| fatal("the operating system's random number generator failed"));
                let generated = SecretString::new(
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw_bytes),
                );
                eprintln!();
                eprintln!("{}", token_banner(&generated));
                eprintln!();
                token_store
                    .import_token(&generated, "default", &["read", "write", "admin"])
                    .unwrap_or_else(|e| fatal(&format!("cannot import generated token: {e}")));
            }
        }
    }
    if !token_store
        .has_active_admin()
        .unwrap_or_else(|e| fatal(&format!("cannot read token store {}: {e}", tokens_db.display())))
    {
        fatal(&format!(
            "no active admin token in {}: every admin token is revoked or expired. Start once with \
             --token <new token> (or AERINI_TOKEN, at least 32 characters) to add one.",
            tokens_db.display()
        ));
    }

    let workflow_db_path = data_dir.join("aerini.db");
    let db = Arc::new(
        WorkflowDb::open(&workflow_db_path, db_pool_size).unwrap_or_else(|e| {
            fatal(&format!("cannot open workflow database {}: {e}", workflow_db_path.display()))
        })
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
            .unwrap_or_else(|e| fatal(&format!("credential store init task failed: {e}")))
            .unwrap_or_else(|e| {
                fatal(&format!("cannot open credential store in {}: {e}", data_dir.display()))
            })
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
            std::fs::create_dir_all(&default).unwrap_or_else(|e| {
                fatal(&format!("cannot create file sandbox directory {}: {e}", default.display()))
            });
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
        run_semaphore: Arc::new(Semaphore::new(max_concurrent_runs.max(1))),
        queue_timeout: std::time::Duration::from_secs(max_queue_wait_secs),
    };

    let credential_routes = Router::new()
        .route("/api/credentials",         get(routes::credentials::list_creds).post(routes::credentials::save_cred))
        .route("/api/credentials/{id}",     delete(routes::credentials::delete_cred))
        .layer(DefaultBodyLimit::max(CREDENTIAL_BODY_LIMIT_BYTES));

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
        .route("/api/plugins/reload",      post(routes::plugins::reload_plugins))
        .route("/api/plugins/load-report", get(routes::plugins::load_report))
        .route("/api/events",              get(routes::workflows::sse_events))
        .route("/api/tokens",              get(routes::tokens::list_tokens_handler).post(routes::tokens::create_token_handler))
        .route("/api/tokens/{id}",          delete(routes::tokens::revoke_token_handler))
        .route("/api/tokens/{id}/workflows",             get(routes::tokens::list_token_workflows_handler))
        .route("/api/tokens/{id}/workflows/{wf_id}",      post(routes::tokens::grant_token_workflow_handler).delete(routes::tokens::revoke_token_workflow_handler))
        .merge(credential_routes)
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
    // arbitrary internet caller reach them at will. Two axes on `trigger`:
    //   - per-IP:       a layer here, applied to every request including ones
    //                   with a wrong secret; bounds a single leaked credential
    //                   used from one place and secret guessing.
    //   - per-workflow: checked inside the handler, after the credential is
    //                   verified. It bounds total cost/abuse however many IPs a
    //                   leaked credential is used from (VPN rotation, botnet),
    //                   and because unauthenticated requests never reach it,
    //                   they cannot use up the budget real visitors share.
    // mint-token has no per-workflow axis: it never runs the workflow, only
    // checks a secret, so the per-IP guess-throttle is the relevant bound.
    let widget_trigger_ip_rl = Arc::new(crate::middleware::RateLimiter::new(20, 60));
    Arc::clone(&widget_trigger_ip_rl).spawn_eviction_task();
    let widget_trigger_wf_rl = Arc::new(crate::middleware::KeyedRateLimiter::new(60, 60));
    Arc::clone(&widget_trigger_wf_rl).spawn_eviction_task();
    let tpc_wt = trusted_proxy_count;
    let widget_trigger_rate_limit = middleware::from_fn(move |req: Request, next: Next| {
        let ip_rl = Arc::clone(&widget_trigger_ip_rl);
        async move {
            let ip: IpAddr = extract_client_ip(&req, tpc_wt);
            if !ip_rl.is_allowed(ip) {
                return crate::middleware::too_many_requests();
            }
            next.run(req).await
        }
    });

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
    //
    // Both public routers also cap the request body at `WIDGET_BODY_LIMIT_BYTES`
    // (the global 5 MiB limit is applied further out; the innermost
    // `DefaultBodyLimit` wins). These routes parse the body before any
    // credential is checked, and their listener refuses anything over 1 MB.
    let trigger_router = Router::new()
        .route("/api/widget/{workflow_id}/trigger", post(routes::widget::trigger_widget))
        .layer(widget_trigger_rate_limit)
        .layer(axum::Extension(widget_trigger_wf_rl))
        .layer(DefaultBodyLimit::max(WIDGET_BODY_LIMIT_BYTES));
    let mint_router = Router::new()
        .route("/api/widget/{workflow_id}/mint-token", post(routes::widget::mint_widget_token))
        .layer(widget_mint_rate_limit)
        .layer(DefaultBodyLimit::max(WIDGET_BODY_LIMIT_BYTES));
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

    if needs_proxy_count_notice(&bind, trusted_proxy_count) {
        tracing::warn!(
            "aerini-server is bound to {} with --trusted-proxy-count 0 (the default). \
             If a reverse proxy (nginx, Caddy, a load balancer) sits in front of it, every \
             client shares one rate-limit allowance, including the widget's 20 requests per \
             minute. Set --trusted-proxy-count (AERINI_TRUSTED_PROXY_COUNT) to the number of \
             proxies in front of this server. See docs/guide/security.md.",
            bind
        );
    }
    let addr     = join_host_port(&bind, port);
    let listener = tokio::net::TcpListener::bind(&addr).await
        .unwrap_or_else(|e| { tracing::error!("Cannot bind {}: {}", addr, e); std::process::exit(1); });

    tracing::info!(
        api_url    = %format!("http://{addr}"),
        health_url = %format!("http://{addr}/api/health"),
        "Aerini Server started in API mode"
    );
    // In API mode any token holder with 'write' scope can submit workflows with
    // HTTP Request nodes pointing at attacker-controlled domains. HTTP Request,
    // AI, integration, Redis, Send Email, S3 and plugin HTTP re-check the address
    // when they connect; Postgres and MySQL connections are only checked
    // beforehand, which leaves a DNS rebinding window that needs a
    // network-level egress firewall (or sslmode=verify-full / VERIFY_IDENTITY).
    // See docs/guide/security.md for recommended iptables/nftables rules.
    tracing::warn!(
        "API mode active: HTTP Request nodes are available to all token holders with 'write' scope. \
         HTTP Request, AI, integration, Redis, Send Email, S3 and plugin HTTP check addresses when they \
         connect, but Postgres and MySQL connections are only checked before they \
         connect, so DNS rebinding can bypass that check unless the URL uses sslmode=verify-full \
         (Postgres) or ssl-mode=VERIFY_IDENTITY (MySQL). Configure a network-level egress firewall to block \
         connections to private, loopback, link-local, and cloud metadata ranges \
         (e.g. 169.254.169.254). See docs/guide/security.md for details."
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

    #[cfg(unix)]
    #[test]
    fn existing_directory_is_judged_by_its_mode_and_left_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("aerini_data_dir_mode_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(super::is_group_or_world_accessible(&dir));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!super::is_group_or_world_accessible(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
    use super::{
        cors_origin_allowed, expand_home, read_backup_key, extract_client_ip, is_localhost_origin, is_mint_token_path,
        needs_proxy_count_notice, token_banner,
    };
    use axum::body::Body;
    use axum::extract::Request;
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::{Path, PathBuf};

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
        assert!(cors_origin_allowed(b"https://site.example", path, &extra));
        assert!(!cors_origin_allowed(b"https://other.example", path, &extra));
    }

    #[test]
    fn localhost_origin_accepts_loopback_hosts_with_optional_numeric_port() {
        for ok in [
            "http://localhost", "http://localhost:3000",
            "http://127.0.0.1", "http://127.0.0.1:8080",
            "http://[::1]", "http://[::1]:5173",
        ] {
            assert!(is_localhost_origin(ok.as_bytes()), "{ok}");
        }
        for bad in [
            "https://[::1]", "http://[::1]:", "http://[::1]:80a", "http://[::1]x",
            "http://[::1].evil.com", "http://[::2]", "http://[::1", "http://localhost.evil.com",
            "http://localhost:", "http://127.0.0.1.evil.com",
        ] {
            assert!(!is_localhost_origin(bad.as_bytes()), "{bad}");
        }
    }

    #[test]
    fn data_dir_expands_tilde_only_as_tilde_or_tilde_slash() {
        let home = Path::new("/home/u");
        assert_eq!(expand_home("~", Some(home)), Ok(PathBuf::from("/home/u")));
        assert_eq!(expand_home("~/", Some(home)), Ok(PathBuf::from("/home/u")));
        assert_eq!(expand_home("~/.aerini-server", Some(home)), Ok(PathBuf::from("/home/u/.aerini-server")));
        assert_eq!(expand_home("~//etc", Some(home)), Ok(PathBuf::from("/home/u/etc")));
        assert_eq!(expand_home("~other/x", Some(home)), Ok(PathBuf::from("~other/x")));
        assert_eq!(expand_home("/data/~", Some(home)), Ok(PathBuf::from("/data/~")));
    }

    #[test]
    fn data_dir_needs_a_home_only_when_it_expands_tilde() {
        assert_eq!(expand_home("/abs/dir", None), Ok(PathBuf::from("/abs/dir")));
        assert_eq!(expand_home("~other/x", None), Ok(PathBuf::from("~other/x")));
        assert!(expand_home("~", None).unwrap_err().contains("no home directory"));
        assert!(expand_home("~/x", None).is_err());
    }

    #[test]
    fn token_banner_lines_all_have_the_same_width_and_show_the_token() {
        let token = "A".repeat(43);
        let banner = token_banner(&token);
        let widths: Vec<usize> = banner.lines().map(|l| l.chars().count()).collect();
        assert!(widths.iter().all(|&w| w == widths[0]), "{widths:?}");
        assert!(banner.lines().any(|l| l.contains(&token) && l.starts_with('│') && l.ends_with('│')));
    }

    #[tokio::test]
    async fn widget_body_limit_overrides_the_wider_global_limit() {
        use axum::{extract::DefaultBodyLimit, http::StatusCode, routing::post, Json, Router};
        use tower::ServiceExt as _;

        let app = Router::new()
            .route("/", post(|Json(_): Json<serde_json::Value>| async {}))
            .layer(DefaultBodyLimit::max(super::WIDGET_BODY_LIMIT_BYTES))
            .layer(DefaultBodyLimit::max(5 * 1024 * 1024));
        let post_json = |n: usize| {
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from(format!("\"{}\"", "a".repeat(n))))
                .unwrap()
        };

        let small = app.clone().oneshot(post_json(1000)).await.unwrap();
        assert_eq!(small.status(), StatusCode::OK);
        let large = app
            .oneshot(post_json(super::WIDGET_BODY_LIMIT_BYTES + 10))
            .await
            .unwrap();
        assert_eq!(large.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn credential_body_limit_admits_the_largest_valid_body_and_refuses_more() {
        use super::routes::credentials::CredBody;
        use axum::{extract::DefaultBodyLimit, http::StatusCode, routing::post, Json, Router};
        use tower::ServiceExt as _;

        let app = Router::new()
            .route("/", post(|Json(_): Json<CredBody>| async {}))
            .layer(DefaultBodyLimit::max(super::CREDENTIAL_BODY_LIMIT_BYTES))
            .layer(DefaultBodyLimit::max(5 * 1024 * 1024));
        let post_body = |body: String| {
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap()
        };

        let largest = serde_json::json!({
            "id": "a".repeat(128),
            "name": "n".repeat(256),
            "value": "\n".repeat(super::MAX_VALUE_BYTES),
            "cred_type": "api_key",
            "update": true,
        })
        .to_string();
        assert!(largest.len() < super::CREDENTIAL_BODY_LIMIT_BYTES);
        let ok = app.clone().oneshot(post_body(largest)).await.unwrap();
        assert_eq!(ok.status(), StatusCode::OK);

        let oversized = serde_json::json!({
            "id": "a",
            "name": "n",
            "value": "v".repeat(super::CREDENTIAL_BODY_LIMIT_BYTES),
        })
        .to_string();
        let refused = app.oneshot(post_body(oversized)).await.unwrap();
        assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn proxy_count_notice_only_for_non_loopback_bind_with_no_proxy_count() {
        assert!(needs_proxy_count_notice("0.0.0.0", 0));
        assert!(needs_proxy_count_notice("192.168.1.5", 0));
        assert!(!needs_proxy_count_notice("0.0.0.0", 1));
        assert!(!needs_proxy_count_notice("127.0.0.1", 0));
        assert!(!needs_proxy_count_notice("::1", 0));
        assert!(!needs_proxy_count_notice("localhost", 0));
    }

    #[test]
    fn backup_key_input_is_bounded_plain_text_and_not_blank() {
        let key = "A".repeat(44);
        assert_eq!(&*read_backup_key(format!("{key}\n").as_bytes()).unwrap(), &format!("{key}\n"));
        assert!(read_backup_key(&b" \r\n"[..]).is_err());
        assert!(read_backup_key(&b""[..]).is_err());
        assert!(read_backup_key(&[0xff, 0xfe, b'a'][..]).is_err());
        assert!(read_backup_key("A".repeat(1025).as_bytes()).is_err());
        assert!(read_backup_key("A".repeat(1024).as_bytes()).is_ok());
    }
}
