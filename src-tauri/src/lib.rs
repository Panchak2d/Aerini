mod commands;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use aerini_engine::{
    db::WorkflowDb,
    node::{NodeRegistry, Reloadable},
    nodes::register_builtins,
    nodes::database::start_pool_eviction_task,
    plugin_loader::{load_plugins, PluginLoadReport},
    scheduler::SchedulerDaemon,
    store::{CredentialStore, KeySource, StoreCredentialResolver},
    EventSink,
};
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;
use tokio_util::sync::CancellationToken;

/// Bookkeeping for in-flight manual `run_workflow` invocations. Two
/// independent concerns, deliberately keyed differently:
///
/// - **Cancellation** — one [`CancellationToken`] per `run_id` (a fresh id
///   the frontend generates for *every* `run_workflow` call, including
///   single-node test runs — see `run-manager/state-machine.ts::start`).
///   Each `run_workflow` invocation registers its own token under its own
///   `run_id` and removes only that entry on completion (success, error, or
///   cancellation) — see `commands/workflow.rs::run_workflow`. `cancel_run`
///   takes a `run_id` and cancels only that entry, so two concurrent
///   invocations (e.g. a whole-workflow run racing a single-node test run)
///   each hold an independent cancellation handle instead of sharing one.
///
/// - **Duplicate-execution guard** — one `tokio::sync::Mutex<()>` exec-lock
///   per `workflow_id` (the workflow's own stable id parsed from the saved
///   JSON — *not* `run_id`, which is fresh every call and would never
///   collide with anything). A second `run_workflow` call for the *same*
///   `workflow_id` while the first is still in flight is rejected with
///   `SchedulerError::AlreadyRunning` rather than executing concurrently.
///   All three of this codebase's execution surfaces serialize same-workflow
///   runs this way: `SchedulerDaemon::exec_locks` (scheduler, skip-if-busy),
///   `ApiState::exec_locks` (`aerini-server`, reject-if-busy after a 5s
///   timeout), and this exec-lock for desktop. A manual Run is a direct
///   user action, so it gets the server's reject-if-busy shape — an
///   explicit "already running" response, not a silently dropped attempt.
///   Per-`run_id` cancellation tracking alone would not stop two runs of
///   the identical saved workflow from executing fully concurrently and
///   doubling every side effect (HTTP calls, file writes, DB rows,
///   emails); closing that gap is this exec-lock's entire job.
///
/// Single-node test runs build their subgraph under a distinct
/// `${workflowId}_sub` id (`run-manager/stream-handler.ts::handleRunSingleNode`),
/// so they use their own exec-lock bucket and do not serialize against (or
/// get rejected by) a concurrent full run of the same open canvas — a
/// deliberate, disclosed scope boundary. Same-canvas overlap between a full
/// run and a single-node test run is instead prevented client-side, by both
/// entry points sharing one `isRunning`/`activeRunId` pair on
/// `RunStateMachine` (see `run-manager/state-machine.ts`); this exec-lock's
/// job is specifically to stop two invocations of the *identical* saved
/// workflow from double-executing.
///
/// `aerini_engine::executor::WorkflowExecutor::run()` creates fully
/// independent internal state per call (`new_shared_state`), so concurrent
/// runs are already independent at the engine level — this struct's locks
/// exist purely for command-layer bookkeeping, with no engine-level
/// counterpart needed.
pub struct ActiveRunToken {
    cancel_tokens: Mutex<HashMap<String, CancellationToken>>,
    exec_locks:    Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl ActiveRunToken {
    pub fn new() -> Self {
        Self {
            cancel_tokens: Mutex::new(HashMap::new()),
            exec_locks:    Mutex::new(HashMap::new()),
        }
    }

    /// Register a fresh cancellation token under `run_id`. `run_id` is a
    /// fresh id generated per invocation by the frontend, so this always
    /// inserts a new entry rather than meaningfully overwriting one.
    pub fn register(&self, run_id: &str, token: CancellationToken) {
        self.cancel_tokens
            .lock().expect("ActiveRunToken cancel_tokens lock poisoned")
            .insert(run_id.to_string(), token);
    }

    /// Remove `run_id`'s cancellation token. Call once that run finishes —
    /// success, error, or cancellation — so a stale id can never later be
    /// mistaken for a still-active run.
    pub fn unregister(&self, run_id: &str) {
        self.cancel_tokens
            .lock().expect("ActiveRunToken cancel_tokens lock poisoned")
            .remove(run_id);
    }

    /// Cancel the run registered under `run_id`. No-op if that id isn't
    /// currently active (already finished, or never existed) — matches the
    /// pre-existing "no-op when idle" contract `cancel_run` has always had.
    pub fn cancel(&self, run_id: &str) {
        if let Some(token) = self.cancel_tokens
            .lock().expect("ActiveRunToken cancel_tokens lock poisoned")
            .get(run_id)
        {
            token.cancel();
        }
    }

    /// Acquire the exec-lock for `workflow_id`, waiting up to `timeout`.
    /// `Ok` holds the lock until the returned guard drops (i.e. for the
    /// caller's entire run). `Err(SchedulerError::AlreadyRunning)` if
    /// another run of the same `workflow_id` is still holding it once
    /// `timeout` elapses.
    ///
    /// Locks are created lazily, one per distinct `workflow_id` ever run
    /// this process lifetime, and never evicted. Unlike the server's
    /// equivalent (`ApiState::exec_locks`, which sweeps once it holds over
    /// 1000 entries — sized for arbitrary external API callers), desktop's
    /// workflow-id set is one local user's own saved workflows; the
    /// unbounded-growth risk the server guards against does not apply at
    /// this scale. Revisit if that assumption stops holding.
    pub async fn acquire_exec_lock(
        &self,
        workflow_id: &str,
        timeout: std::time::Duration,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, aerini_engine::scheduler::SchedulerError> {
        let lock = {
            let mut locks = self.exec_locks
                .lock().expect("ActiveRunToken exec_locks lock poisoned");
            Arc::clone(
                locks.entry(workflow_id.to_string())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            )
        };
        match tokio::time::timeout(timeout, lock.lock_owned()).await {
            Ok(guard)     => Ok(guard),
            Err(_elapsed) => Err(aerini_engine::scheduler::SchedulerError::AlreadyRunning),
        }
    }
}

impl Default for ActiveRunToken {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod active_run_token_tests {
    use super::*;

    #[tokio::test]
    async fn cancel_targets_only_the_named_run_id() {
        let bookkeeping = ActiveRunToken::new();
        let token_a = CancellationToken::new();
        let token_b = CancellationToken::new();
        bookkeeping.register("run_a", token_a.clone());
        bookkeeping.register("run_b", token_b.clone());

        bookkeeping.cancel("run_a");

        assert!(token_a.is_cancelled(), "cancel(\"run_a\") must cancel run_a's token");
        assert!(!token_b.is_cancelled(), "cancel(\"run_a\") must not touch run_b's token");
    }

    #[tokio::test]
    async fn cancel_of_unknown_run_id_is_a_silent_no_op() {
        let bookkeeping = ActiveRunToken::new();
        // Must not panic — matches cancel_run's pre-existing "no-op when idle" contract.
        bookkeeping.cancel("never_registered");
    }

    #[tokio::test]
    async fn unregister_then_cancel_is_a_no_op_not_a_double_cancel_of_a_reused_id() {
        let bookkeeping = ActiveRunToken::new();
        let first_token = CancellationToken::new();
        bookkeeping.register("run_1", first_token.clone());
        bookkeeping.unregister("run_1");

        // A hypothetical second run reusing the same id string (ids are
        // fresh-per-call in practice, but nothing stops the frontend from
        // reusing a string) must only ever affect the second registration.
        let second_token = CancellationToken::new();
        bookkeeping.register("run_1", second_token.clone());
        bookkeeping.cancel("run_1");

        assert!(!first_token.is_cancelled(), "the unregistered, finished run must be untouched");
        assert!(second_token.is_cancelled(), "cancel must reach the currently-registered run_1");
    }

    #[tokio::test]
    async fn second_acquire_for_the_same_workflow_id_is_rejected_while_the_first_holds_the_lock() {
        let bookkeeping = ActiveRunToken::new();
        let short_timeout = std::time::Duration::from_millis(50);

        let _first_guard = bookkeeping
            .acquire_exec_lock("wf_1", short_timeout)
            .await
            .expect("first acquire must succeed immediately — lock is uncontended");

        let second = bookkeeping.acquire_exec_lock("wf_1", short_timeout).await;
        match second {
            Err(aerini_engine::scheduler::SchedulerError::AlreadyRunning) => {}
            other => panic!("expected AlreadyRunning while the first guard is held, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn acquire_succeeds_again_once_the_first_guard_is_dropped() {
        let bookkeeping = ActiveRunToken::new();
        let timeout = std::time::Duration::from_millis(200);

        let first_guard = bookkeeping
            .acquire_exec_lock("wf_2", timeout)
            .await
            .expect("first acquire must succeed");
        drop(first_guard);

        bookkeeping
            .acquire_exec_lock("wf_2", timeout)
            .await
            .expect("acquire must succeed again once the prior run's guard has dropped");
    }

    #[tokio::test]
    async fn different_workflow_ids_never_contend_with_each_other() {
        let bookkeeping = ActiveRunToken::new();
        let timeout = std::time::Duration::from_millis(50);

        // Both held at once — must not block or reject each other; this is
        // the case that keeps unrelated workflows running fully concurrently,
        // matching WorkflowExecutor::run()'s own per-call independence.
        let _guard_a = bookkeeping.acquire_exec_lock("wf_a", timeout).await
            .expect("wf_a must acquire uncontended");
        let _guard_b = bookkeeping.acquire_exec_lock("wf_b", timeout).await
            .expect("wf_b must acquire uncontended even while wf_a's guard is held");
    }

    #[tokio::test]
    async fn a_queued_second_acquire_proceeds_if_the_first_releases_before_timeout() {
        let bookkeeping = std::sync::Arc::new(ActiveRunToken::new());
        let generous_timeout = std::time::Duration::from_millis(500);

        let first_guard = bookkeeping
            .acquire_exec_lock("wf_3", generous_timeout)
            .await
            .expect("first acquire must succeed");

        let waiter = {
            let bookkeeping = std::sync::Arc::clone(&bookkeeping);
            tokio::spawn(async move {
                bookkeeping.acquire_exec_lock("wf_3", generous_timeout).await
            })
        };

        // Give the waiter a moment to actually start waiting on the lock,
        // then release it well before generous_timeout elapses.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        drop(first_guard);

        let result = waiter.await.expect("waiter task must not panic");
        assert!(result.is_ok(), "queued acquire must succeed once the lock is released, not time out");
    }
}

struct TauriEventSink {
    handle: tauri::AppHandle,
}

impl EventSink for TauriEventSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = tauri::Emitter::emit(&self.handle, event, payload);
    }
}

/// Read a .aerini or .json workflow file from an absolute path.
/// Restricted to those two extensions.
#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    let canonical = std::fs::canonicalize(&path)
        .map_err(|e| format!("Could not resolve path '{}': {}", path, e))?;
    let canonical_str = canonical.to_string_lossy();
    if !canonical_str.ends_with(".aerini") && !canonical_str.ends_with(".json") {
        return Err("Only .aerini or .json files can be opened this way.".to_string());
    }
    std::fs::read_to_string(&canonical)
        .map_err(|e| format!("Could not read file '{}': {}", canonical_str, e))
}

/// Called by the frontend once it has painted its first real frame.
/// Window is created hidden (see tauri.conf.json) to avoid a flash of the
/// desktop showing through before content is ready.
#[tauri::command]
fn show_main_window(window: tauri::Window) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_file_dialog(
    app:      tauri::AppHandle,
    content:  String,
    filename: String,
) -> Result<String, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog()
        .file()
        .set_file_name(&filename)
        .add_filter("Aerini Workflow", &["aerini"])
        .save_file(move |path| { let _ = tx.send(path); });

    match rx.await {
        Ok(Some(path)) => {
            let path_buf = path.as_path()
                .ok_or_else(|| "Invalid path".to_string())?
                .to_path_buf();
            std::fs::write(&path_buf, content.as_bytes())
                .map_err(|e| format!("Write failed: {}", e))?;
            Ok(path_buf.display().to_string())
        }
        Ok(None) => Err("cancelled".to_string()),
        Err(_)   => Err("Dialog error".to_string()),
    }
}

/// Save a server export zip — opens a save dialog filtered for .zip,
/// copies the temp zip to the user-chosen path, then deletes the temp file.
#[tauri::command]
async fn save_export_zip(
    app:      tauri::AppHandle,
    zip_path: String,
    filename: String,
) -> Result<String, String> {
    let src = std::path::PathBuf::from(&zip_path);
    let temp = std::env::temp_dir();
    // Canonicalize to resolve '..' and symlinks before containment check.
    // Also confirms the file exists — canonicalize fails if the path doesn't.
    let canonical_src = src.canonicalize()
        .map_err(|_| format!("Export zip not found at: {}", zip_path))?;
    if !canonical_src.starts_with(&temp) {
        return Err("zip_path must be within the system temp directory".to_string());
    }

    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog()
        .file()
        .set_file_name(&filename)
        .add_filter("Zip Archive", &["zip"])
        .save_file(move |path| { let _ = tx.send(path); });

    match rx.await {
        Ok(Some(path)) => {
            let dest = path.as_path()
                .ok_or_else(|| "Invalid path".to_string())?
                .to_path_buf();
            std::fs::copy(&canonical_src, &dest)
                .map_err(|e| format!("Copy failed: {}", e))?;
            let _ = std::fs::remove_file(&canonical_src);
            Ok(dest.display().to_string())
        }
        Ok(None) => {
            let _ = std::fs::remove_file(&canonical_src);
            Err("cancelled".to_string())
        }
        Err(_) => Err("Dialog error".to_string()),
    }
}

#[tauri::command]
fn close_window(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

#[tauri::command]
fn force_quit(
    app:    tauri::AppHandle,
    daemon: tauri::State<'_, Arc<SchedulerDaemon>>,
) {
    daemon.stop_all();
    app.exit(0);
}

#[tauri::command]
async fn check_bundled_node() -> Result<String, String> {
    aerini_engine::nodes::code_node::bundled_node_health_check()
}

/// Cancel a specific in-flight manual run by its `run_id`. Runs are tracked
/// independently by ID, so this is a no-op if that particular `run_id` isn't
/// currently active, and it never affects any other concurrently active run.
#[tauri::command]
fn cancel_run(run_id: String, active_run: tauri::State<'_, Arc<ActiveRunToken>>) {
    active_run.cancel(&run_id);
}

#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let al = app.autolaunch();
    if enabled { al.enable().map_err(|e| e.to_string()) }
    else       { al.disable().map_err(|e| e.to_string()) }
}

/// Open a native folder picker dialog and return the chosen path.
/// Returns null (None → JS null) when the user cancels.
#[tauri::command]
async fn pick_folder_dialog(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog()
        .file()
        .pick_folder(move |path| { let _ = tx.send(path); });
    match rx.await {
        Ok(Some(path)) => path.as_path().map(|p| p.display().to_string()),
        _ => None,
    }
}

/// Open a native file picker dialog filtered to `.wasm` and `.aerinipkg`
/// files and return the chosen path. Returns null (None → JS null) when the
/// user cancels.
#[tauri::command]
async fn pick_plugin_file_dialog(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog()
        .file()
        .add_filter("Aerini Plugin", &["wasm", "aerinipkg"])
        .pick_file(move |path| { let _ = tx.send(path); });
    match rx.await {
        Ok(Some(path)) => path.as_path().map(|p| p.display().to_string()),
        _ => None,
    }
}

/// Write base64-encoded media bytes to a temp file and return the absolute path.
///
/// Strips `/`, `\`, and `..` from the filename before constructing the path.
/// The file is written to `{temp_dir}/aerini_media/{safe_filename}`.
#[tauri::command]
async fn write_temp_file(filename: String, data: String) -> Result<String, String> {
    // Path traversal protection — strip separators and collapse ".."
    let safe_name: String = filename
        .chars()
        .filter(|c| *c != '/' && *c != '\\')
        .collect::<String>()
        .split("..")
        .collect::<Vec<_>>()
        .join("");
    let safe_name = safe_name.trim().to_string();
    if safe_name.is_empty() {
        return Err("Invalid filename: empty after sanitisation".to_string());
    }

    let dir = std::env::temp_dir().join("aerini_media");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("Failed to create temp dir: {}", e))?;

    const MAX_TEMP_FILE_BYTES: usize = 50 * 1024 * 1024; // 50 MB
    let bytes = BASE64.decode(&data)
        .map_err(|e| format!("Base64 decode error: {}", e))?;
    if bytes.len() > MAX_TEMP_FILE_BYTES {
        return Err(format!(
            "File too large: {} bytes (limit {} bytes)",
            bytes.len(),
            MAX_TEMP_FILE_BYTES,
        ));
    }

    let path = dir.join(&safe_name);
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| format!("Write failed: {}", e))?;

    Ok(path.display().to_string())
}

/// Delete files in `{temp_dir}/aerini_media/` that are older than 24 hours.
/// Must be called via spawn_blocking — uses std::fs which is synchronous.
fn cleanup_old_temp_files(dir: &std::path::Path) {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(86_400))
        .unwrap_or(std::time::UNIX_EPOCH);

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if modified < cutoff {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Installs the memory-tracking allocator's tracker before anything else
    // — must happen before the first workflow could possibly run, and this
    // is the earliest point in the app's lifecycle. The #[global_allocator]
    // itself (aerini-engine/src/lib.rs) is already active from the process's
    // first allocation regardless; this call only wires up where tracked
    // allocations get reported to.
    aerini_engine::mem_tracking::install();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(|app| {
            let data_dir: PathBuf = app.path().app_data_dir()
                .expect("Failed to resolve app data directory");
            std::fs::create_dir_all(&data_dir)?;

            let db = Arc::new(
                WorkflowDb::open(&data_dir.join("workflows.db"), 8)
                    .expect("Failed to open workflow database")
            );
            let cred_db_path = data_dir.join("credentials.db");
            let cred_key_fallback = data_dir.join(".cred.key");
            // OsKeychain's Linux backend blocks the calling thread to reach the
            // secret service, and this hook runs on Tauri's own Tokio runtime —
            // do the open on a dedicated blocking thread, not here.
            let cred_store = Arc::new(
                tauri::async_runtime::block_on(async move {
                    tokio::task::spawn_blocking(move || {
                        CredentialStore::open(
                            &cred_db_path,
                            KeySource::OsKeychain { fallback: cred_key_fallback },
                        )
                    })
                    .await
                    .expect("Credential store init task panicked")
                })
                .expect("Failed to open credential store")
            );
            let mut registry = NodeRegistry::new();
            register_builtins(&mut registry, &data_dir, Some(Arc::clone(&db)));
            start_pool_eviction_task(tauri::async_runtime::handle().inner());
            let mut plugin_load_report = PluginLoadReport::default();
            if let Some(dir_str) = db.get_setting("plugin_dir").unwrap_or(None) {
                let dir = std::path::PathBuf::from(&dir_str);
                if !dir.exists() {
                    tracing::warn!("plugin_dir {:?} does not exist — no plugins loaded", dir);
                } else {
                    plugin_load_report = load_plugins(&mut registry, &dir);
                }
            }
            let registry = Arc::new(Reloadable::new(registry));

            let resolver = Arc::new(StoreCredentialResolver { store: Arc::clone(&cred_store) });

            let event_sink: Arc<dyn EventSink> = Arc::new(TauriEventSink {
                handle: app.handle().clone(),
            });

            let parallel_execution = db.get_setting("parallel_execution")
                .unwrap_or(None)
                .map(|v| v == "true")
                .unwrap_or(false);

            let daemon = Arc::new(SchedulerDaemon::new(
                Arc::clone(&db) as Arc<dyn aerini_engine::scheduler::SchedulerDb>,
                Arc::clone(&registry),
                Arc::clone(&resolver) as Arc<dyn aerini_engine::executor::CredentialResolver>,
                Arc::clone(&event_sink),
            )
            .with_parallel_execution(parallel_execution)
            // Desktop is single-tenant by definition — the
            // person who configured this schedule is the same person whose
            // machine runs it, same trust level as their own manual runs
            // (see commands/workflow.rs::run_workflow's identical grant).
            // Never set on aerini-server (main.rs / api_server/mod.rs),
            // where a scheduled/webhook run has no per-caller token scope to
            // derive trust from.
            .with_caller_is_admin(true));
            daemon.start(tauri::async_runtime::handle().inner());

            app.manage(Arc::clone(&db));
            app.manage(Arc::clone(&cred_store));
            app.manage(Arc::clone(&registry));
            app.manage(
                Arc::clone(&resolver)
                    as Arc<dyn aerini_engine::executor::CredentialResolver>
            );
            app.manage(Arc::clone(&daemon));
            app.manage(Arc::clone(&event_sink));
            app.manage(Arc::new(ActiveRunToken::new()));
            app.manage(Arc::new(Reloadable::new(plugin_load_report)));
            app.manage(Arc::new(tokio::sync::Mutex::new(())));

            // Async startup cleanup of temp media files older than 24h —
            // spawn_blocking so the std::fs directory scan doesn't occupy a tokio thread.
            let temp_media_dir = std::env::temp_dir().join("aerini_media");
            tauri::async_runtime::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    cleanup_old_temp_files(&temp_media_dir);
                }).await;
            });

            // Periodic live memory-breakdown event, consumed by the
            // frontend's memory indicator. 2s interval — frequent enough
            // to feel live, cheap enough (a DashMap iteration over however
            // many runs/nodes are currently in flight, realistically single
            // digits) that this is not worth making configurable. Emits
            // only when `snapshot()` is non-empty, i.e. only while at least
            // one workflow is actually running — most of this product's
            // target deployments sit idle between scheduled runs, and
            // there is nothing useful to tell a listener during that time.
            let mem_event_sink = Arc::clone(&event_sink);
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
                loop {
                    interval.tick().await;
                    let breakdown = aerini_engine::mem_tracking::snapshot();
                    if !breakdown.is_empty() {
                        if let Ok(payload) = serde_json::to_value(&breakdown) {
                            mem_event_sink.emit("memory-breakdown", payload);
                        }
                    }
                }
            });

            // Live-performance push, alongside — not replacing — the 2s
            // memory-breakdown event above (the MEM chip stays on the slow
            // poll; this panel is for depth). 300ms: perf_monitor's own
            // sampler ticks every SAMPLE_INTERVAL_MS=150ms — pushing at
            // exactly that rate would often re-emit a payload with no new
            // sampler data since the prior tick; doubling the sampler's own
            // interval still lands ~6.7x faster than the 2s baseline for a
            // live-updating view without emitting more often than the
            // underlying data can actually change. Broadcasts every
            // currently-running workflow at once
            // (`perf_monitor::all_live_snapshots`) — the same "all-live,
            // frontend filters/aggregates per workflow" shape
            // `memory-breakdown` above uses (`statusbar-fields.ts::initMemChip`
            // filters `RunBreakdown[]` to `wfManager.currentId` and
            // aggregates the rest into `bg-mem-badge`) — deliberately not
            // scoped to a single "currently open" workflow_id, which would
            // silently lose that same background-runs case. Emits only
            // when non-empty, same idle-cost reasoning as memory-breakdown.
            let perf_event_sink = Arc::clone(&event_sink);
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(300));
                loop {
                    interval.tick().await;
                    let live = aerini_engine::perf_monitor::all_live_snapshots();
                    if !live.is_empty() {
                        if let Ok(payload) = serde_json::to_value(&live) {
                            perf_event_sink.emit("performance-live", payload);
                        }
                    }
                }
            });

            // OS close button hides to tray rather than quitting
            let main_window = app.get_webview_window("main")
                .expect("main webview window must exist at setup time");
            let main_window_clone = main_window.clone();
            // Safety net: if the frontend never calls show_main_window (e.g. a JS
            // error before it reaches that point), don't leave the app permanently
            // invisible to the user.
            let fallback_window = main_window.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                if !fallback_window.is_visible().unwrap_or(true) {
                    let _ = fallback_window.show();
                }
            });
            main_window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = tauri::Emitter::emit(
                        &main_window_clone,
                        "tauri://close-requested",
                        (),
                    );
                }
            });

            let tray_menu = tauri::menu::MenuBuilder::new(app)
                .item(&tauri::menu::MenuItemBuilder::with_id("show", "Open Aerini").build(app)?)
                .separator()
                .item(&tauri::menu::MenuItemBuilder::with_id("quit", "Quit Aerini").build(app)?)
                .build()?;

            let _tray = tauri::tray::TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("app must have a window icon configured").clone())
                .menu(&tray_menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        if let Some(daemon) = app.try_state::<Arc<SchedulerDaemon>>() {
                            daemon.stop_all();
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left, ..
                    } = event {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            show_main_window,
            read_text_file,
            save_file_dialog,
            save_export_zip,
            close_window,
            force_quit,
            check_bundled_node,
            cancel_run,
            get_autostart,
            set_autostart,
            commands::workflow::save_workflow,
            commands::workflow::load_workflow,
            commands::workflow::list_workflows,
            commands::workflow::delete_workflow,
            commands::workflow::run_workflow,
            commands::workflow::get_node_types,
            commands::workflow::get_setting,
            commands::workflow::set_setting,
            commands::workflow::clear_chat_session,
            commands::workflow::save_run_record,
            commands::workflow::save_performance_report,
            commands::workflow::save_run_started,
            commands::workflow::list_run_records,
            commands::workflow::delete_run_record,
            commands::workflow::clear_run_records,
            commands::workflow::save_version,
            commands::workflow::list_versions,
            commands::workflow::get_version,
            commands::workflow::delete_version,
            commands::chat::list_chat_sessions,
            commands::chat::save_chat_session,
            commands::chat::delete_chat_session,
            commands::credentials::list_credentials,
            commands::credentials::list_credential_usage,
            commands::credentials::get_credential_metadata,
            commands::credentials::get_credential_secret,
            commands::credentials::save_credential,
            commands::credentials::delete_credential,
            commands::credentials::export_encryption_key,
            commands::oauth::get_oauth_redirect_port,
            // Keep original command names for IPC compatibility with frontend
            commands::scheduler::start_scheduled_workflow,
            commands::scheduler::stop_scheduled_workflow,
            commands::scheduler::get_scheduled_jobs,
            commands::scheduler::get_scheduled_job,
            commands::scheduler::set_always_on,
            commands::scheduler::stop_all_jobs,
            commands::scheduler::request_scheduler_state,
            commands::export::validate_workflow_for_export,
            commands::export::generate_server_package,
            commands::export::generate_docker_package,
            commands::export::export_all_workflows,
            commands::plugins::list_installed_plugins,
            commands::plugins::install_plugin_from_path,
            commands::plugins::remove_plugin,
            commands::plugins::install_plugin_pack_from_path,
            commands::plugins::remove_plugin_pack,
            commands::plugins::reload_plugins,
            commands::memory::get_memory_breakdown,
            commands::memory::get_process_memory,
            commands::performance::get_live_performance,
            commands::performance::get_recent_performance,
            commands::performance::get_performance_report,
            commands::performance::list_performance_reports,
            commands::performance::delete_performance_report,
            commands::performance::clear_performance_reports,
            commands::update::check_for_update,
            pick_folder_dialog,
            pick_plugin_file_dialog,
            write_temp_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running aerini")
}
