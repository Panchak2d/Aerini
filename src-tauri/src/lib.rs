mod commands;
mod temp_media;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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
    error::EngineError,
    EventSink,
};
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;
use tokio_util::sync::CancellationToken;

const MAX_RUN_ID_BYTES: usize = 128;

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

    fn tokens(&self) -> std::sync::MutexGuard<'_, HashMap<String, CancellationToken>> {
        self.cancel_tokens.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Registers `token` under `run_id`. Returns `false`, leaving the existing
    /// entry in place, when a run with that id is already registered.
    pub fn register(&self, run_id: &str, token: CancellationToken) -> bool {
        match self.tokens().entry(run_id.to_string()) {
            std::collections::hash_map::Entry::Occupied(_) => false,
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(token);
                true
            }
        }
    }

    /// Registers `token` under `run_id` for as long as the returned guard
    /// lives, so the entry is removed on every exit including a dropped
    /// command future.
    pub fn track(self: &Arc<Self>, run_id: &str, token: CancellationToken) -> Result<RunRegistration, String> {
        if run_id.is_empty() || run_id.len() > MAX_RUN_ID_BYTES {
            return Err(format!("run_id must be 1-{MAX_RUN_ID_BYTES} bytes"));
        }
        if !self.register(run_id, token) {
            return Err("A run with this id is already active".to_string());
        }
        Ok(RunRegistration { owner: Arc::clone(self), run_id: run_id.to_string() })
    }

    /// Removes `run_id`'s cancellation token.
    pub fn unregister(&self, run_id: &str) {
        self.tokens().remove(run_id);
    }

    /// Cancels the run registered under `run_id`. No-op if that id isn't
    /// currently active.
    pub fn cancel(&self, run_id: &str) {
        if let Some(token) = self.tokens().get(run_id) {
            token.cancel();
        }
    }

    /// Number of runs currently registered, i.e. started and not yet finished.
    pub fn active_count(&self) -> usize {
        self.tokens().len()
    }

    /// Cancels every registered run. Each run unregisters itself as it unwinds.
    pub fn cancel_all(&self) {
        for token in self.tokens().values() {
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
                .lock().unwrap_or_else(|e| e.into_inner());
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

/// Holds a run's cancellation entry; dropping it removes the entry.
pub struct RunRegistration {
    owner: Arc<ActiveRunToken>,
    run_id: String,
}

impl Drop for RunRegistration {
    fn drop(&mut self) {
        self.owner.unregister(&self.run_id);
    }
}

impl Default for ActiveRunToken {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod start_visibility_tests {
    use super::should_start_hidden;

    #[test]
    fn minimized_launch_without_an_update_result_stays_hidden() {
        assert!(should_start_hidden(["aerini", "--minimized"], false));
    }

    #[test]
    fn minimized_launch_with_an_update_result_is_shown() {
        assert!(!should_start_hidden(["aerini", "--minimized"], true));
    }

    #[test]
    fn launch_without_the_exact_flag_is_shown() {
        assert!(!should_start_hidden(["aerini"], false));
        assert!(!should_start_hidden(["aerini", "--minimized-later", "minimized"], false));
        assert!(!should_start_hidden(Vec::<String>::new(), false));
    }
}

#[cfg(test)]
mod close_action_tests {
    use super::{close_action, CloseAction};

    #[test]
    fn a_usable_tray_always_hides_and_without_one_only_running_work_keeps_the_app_open() {
        assert_eq!(close_action(true, false), CloseAction::Hide);
        assert_eq!(close_action(true, true), CloseAction::Hide);
        assert_eq!(close_action(false, true), CloseAction::Minimize);
        assert_eq!(close_action(false, false), CloseAction::Quit);
    }
}

#[cfg(test)]
mod tray_wait_tests {
    use super::wait_until;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn polling_stops_as_soon_as_the_probe_succeeds() {
        let calls = AtomicUsize::new(0);
        let found = wait_until(
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                async move { n == 3 }
            },
            Duration::from_secs(30),
            Duration::from_millis(5),
        )
        .await;
        assert!(found);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_zero_wait_probes_exactly_once() {
        let calls = AtomicUsize::new(0);
        let found = wait_until(
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { false }
            },
            Duration::ZERO,
            Duration::from_millis(10),
        )
        .await;
        assert!(!found);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
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
    async fn cancel_all_reaches_every_registered_run_and_count_tracks_registration() {
        let bookkeeping = ActiveRunToken::new();
        let token_a = CancellationToken::new();
        let token_b = CancellationToken::new();
        assert_eq!(bookkeeping.active_count(), 0);
        bookkeeping.register("run_a", token_a.clone());
        bookkeeping.register("run_b", token_b.clone());
        assert_eq!(bookkeeping.active_count(), 2);

        bookkeeping.cancel_all();
        assert!(token_a.is_cancelled() && token_b.is_cancelled());

        bookkeeping.unregister("run_a");
        bookkeeping.unregister("run_b");
        assert_eq!(bookkeeping.active_count(), 0);
    }

    #[tokio::test]
    async fn a_run_id_already_in_flight_is_refused_and_keeps_its_own_token() {
        let bookkeeping = Arc::new(ActiveRunToken::new());
        let first_token = CancellationToken::new();
        let _first = bookkeeping
            .track("run_1", first_token.clone())
            .expect("a new run id registers");

        let second_token = CancellationToken::new();
        assert!(bookkeeping.track("run_1", second_token.clone()).is_err());
        assert!(bookkeeping.track("", CancellationToken::new()).is_err());
        assert!(bookkeeping.track(&"x".repeat(MAX_RUN_ID_BYTES + 1), CancellationToken::new()).is_err());

        bookkeeping.cancel("run_1");
        assert!(first_token.is_cancelled());
        assert!(!second_token.is_cancelled());
        assert_eq!(bookkeeping.active_count(), 1);
    }

    #[tokio::test]
    async fn dropping_a_registration_frees_the_run_id() {
        let bookkeeping = Arc::new(ActiveRunToken::new());
        let registration = bookkeeping
            .track("run_1", CancellationToken::new())
            .expect("a new run id registers");
        assert_eq!(bookkeeping.active_count(), 1);
        drop(registration);
        assert_eq!(bookkeeping.active_count(), 0);
        assert!(bookkeeping.track("run_1", CancellationToken::new()).is_ok());
    }

    #[tokio::test]
    async fn cancel_of_unknown_run_id_is_a_silent_no_op() {
        let bookkeeping = ActiveRunToken::new();
        // Must not panic: cancelling an idle id is a no-op.
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

/// Argument the autostart entry passes so a login launch opens in the tray.
const MINIMIZED_FLAG: &str = "--minimized";

/// Whether the main window stays hidden after launch. A launch with the
/// minimized flag stays hidden unless there is an update result the person
/// needs to see.
fn should_start_hidden<I, S>(args: I, has_update_notice: bool) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    !has_update_notice && args.into_iter().any(|a| a.as_ref() == MINIMIZED_FLAG)
}

/// `show_main_window` does nothing while this is `true`. Set at startup, and
/// cleared if the launch turns out to have no tray to reopen the window from.
struct StartHidden(AtomicBool);

/// Whether the tray icon was created.
struct TrayInstalled(bool);

/// How long a launch that stays hidden waits for a desktop to offer a tray
/// before it shows the window instead. A login launch can start before the
/// panel has registered its tray.
#[cfg(target_os = "linux")]
const TRAY_HOST_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

#[cfg(target_os = "linux")]
const TRAY_HOST_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// Whether the desktop can show a tray icon. Only Linux can lack one: a
/// desktop there needs a StatusNotifier host, and GNOME has none without an
/// extension.
async fn desktop_has_tray_host() -> bool {
    #[cfg(target_os = "linux")]
    {
        aerini_engine::session_bus::status_notifier_host_registered().await
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// Polls `probe` every `every` until it returns `true` or `within` has passed;
/// always probes at least once.
#[cfg(any(target_os = "linux", test))]
async fn wait_until<F, Fut>(mut probe: F, within: std::time::Duration, every: std::time::Duration) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if probe().await {
            return true;
        }
        if tokio::time::Instant::now() + every > deadline {
            return false;
        }
        tokio::time::sleep(every).await;
    }
}

fn reveal_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Called by the frontend once it has painted its first real frame.
/// Window is created hidden (see tauri.conf.json) to avoid a flash of the
/// desktop showing through before content is ready. Does nothing for a launch
/// that is meant to stay in the tray; the tray's "Open Aerini" shows it then.
#[tauri::command]
fn show_main_window(window: tauri::Window) -> Result<(), String> {
    if window.try_state::<StartHidden>().is_some_and(|hold| hold.0.load(Ordering::Acquire)) {
        return Ok(());
    }
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

/// Save an export zip — opens a save dialog filtered for .zip, copies the
/// export the app generated to the user-chosen path, then deletes the
/// temporary file whatever the outcome. Only files created by the export
/// commands are accepted.
#[tauri::command]
async fn save_export_zip(
    app:      tauri::AppHandle,
    zip_path: String,
    filename: String,
) -> Result<String, String> {
    let canonical_src = commands::export::issued_export(&zip_path)?;
    let outcome = copy_export_to_chosen_path(&app, &canonical_src, &filename).await;
    commands::export::discard_export(&canonical_src);
    outcome
}

async fn copy_export_to_chosen_path(
    app:      &tauri::AppHandle,
    src:      &std::path::Path,
    filename: &str,
) -> Result<String, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog()
        .file()
        .set_file_name(filename)
        .add_filter("Zip Archive", &["zip"])
        .save_file(move |path| { let _ = tx.send(path); });

    match rx.await {
        Ok(Some(path)) => {
            let dest = path.as_path()
                .ok_or_else(|| "Invalid path".to_string())?
                .to_path_buf();
            std::fs::copy(src, &dest).map_err(|e| format!("Copy failed: {}", e))?;
            Ok(dest.display().to_string())
        }
        Ok(None) => Err("cancelled".to_string()),
        Err(_) => Err("Dialog error".to_string()),
    }
}

/// Shows a startup error and exits once it is dismissed. Uses the non-blocking
/// `show` because `setup` runs on the main thread, where `blocking_show` would
/// freeze the app.
fn exit_with_startup_error(app: &tauri::AppHandle, title: &str, message: String) {
    eprintln!("[startup] {title}: {message}");
    app.dialog()
        .message(message)
        .title(title)
        .kind(tauri_plugin_dialog::MessageDialogKind::Error)
        .show(|_| std::process::exit(1));
}

fn report_credential_store_failure(app: &tauri::AppHandle, err: &EngineError) {
    let message = match err {
        EngineError::KeyUnavailable(detail) => format!(
            "Aerini could not read the encryption key for your saved credentials, so it did not start. \
             Your credentials have not been changed.\n\n\
             On macOS: quit Aerini, open it again, and choose \"Always Allow\" when macOS asks for Keychain access.\n\n\
             Otherwise: unlock or start your system keychain and reopen Aerini, or restore the key from \
             your Backup Encryption Key copy (see the Credentials guide).\n\n\
             Details: {detail}"
        ),
        EngineError::KeyMismatch(detail) => format!(
            "The encryption key Aerini found does not decrypt your saved credentials, so it did not start. \
             Your credentials have not been changed.\n\n\
             Restore the key from your Backup Encryption Key copy (see the Credentials guide).\n\n\
             Details: {detail}"
        ),
        other => format!("Aerini could not open its credential store: {other}"),
    };
    exit_with_startup_error(app, "Aerini cannot unlock your credentials", message);
}

/// What the window's close button does. With a tray the window hides and the
/// tray brings it back. Without one nothing can bring a hidden window back and
/// there is no other way to quit, so the app quits, unless a run is in flight
/// or a background job is armed, which the window then leaves running by
/// minimizing.
#[derive(Debug, PartialEq, Eq)]
enum CloseAction {
    Hide,
    Minimize,
    Quit,
}

fn close_action(tray_usable: bool, work_in_progress: bool) -> CloseAction {
    match (tray_usable, work_in_progress) {
        (true, _) => CloseAction::Hide,
        (false, true) => CloseAction::Minimize,
        (false, false) => CloseAction::Quit,
    }
}

/// `true` when a manual run is in flight or a job is armed in the scheduler,
/// or when that can't be told.
async fn work_in_progress(app: &tauri::AppHandle) -> bool {
    if app.try_state::<Arc<ActiveRunToken>>().is_some_and(|runs| runs.active_count() > 0) {
        return true;
    }
    let Some(daemon) = app.try_state::<Arc<SchedulerDaemon>>().map(|d| d.inner().clone()) else {
        return true;
    };
    tauri::async_runtime::spawn_blocking(move || {
        daemon
            .list_jobs()
            .map(|rows| rows.iter().any(|row| row.status == "active"))
            .unwrap_or(true)
    })
    .await
    .unwrap_or(true)
}

fn quit_app(app: &tauri::AppHandle) {
    if let Some(daemon) = app.try_state::<Arc<SchedulerDaemon>>() {
        daemon.stop_all();
    }
    app.exit(0);
}

#[tauri::command]
async fn close_window(app: tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    let tray_usable = app.try_state::<TrayInstalled>().is_some_and(|t| t.0) && desktop_has_tray_host().await;
    let busy = !tray_usable && work_in_progress(&app).await;
    match close_action(tray_usable, busy) {
        CloseAction::Hide => { let _ = window.hide(); }
        CloseAction::Minimize => { let _ = window.minimize(); }
        CloseAction::Quit => quit_app(&app),
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

/// Open a native file picker dialog with no extension filter and return the
/// chosen path. Returns null (None → JS null) when the user cancels. Unlike
/// `pick_plugin_file_dialog` above, this is for node config fields (e.g. a
/// plugin's own `x-aerini-path-picker` schema flag -- see
/// popover/field-renderer.ts's renderPathPickerField) that can point at any
/// file, not a fixed type this binary already knows about.
#[tauri::command]
async fn pick_file_dialog(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog()
        .file()
        .pick_file(move |path| { let _ = tx.send(path); });
    match rx.await {
        Ok(Some(path)) => path.as_path().map(|p| p.display().to_string()),
        _ => None,
    }
}

/// Write base64-encoded media bytes to a temp file and return the absolute path.
///
/// The file goes into a private `aerini_media` directory under the system temp
/// directory, named from a hash of its bytes plus the sanitized last segment of
/// `filename` (see `temp_media`), so the caller never picks the location and
/// the same media is stored once.
#[tauri::command]
async fn write_temp_file(filename: String, data: String) -> Result<String, String> {
    const MAX_TEMP_FILE_BYTES: usize = 50 * 1024 * 1024; // 50 MB
    // Base64 of MAX_TEMP_FILE_BYTES; checked first so an oversized string is
    // refused before it is decoded.
    const MAX_ENCODED_LEN: usize = MAX_TEMP_FILE_BYTES.div_ceil(3) * 4;
    let too_large = || format!("File too large (limit {} bytes)", MAX_TEMP_FILE_BYTES);
    if data.len() > MAX_ENCODED_LEN {
        return Err(too_large());
    }

    tokio::task::spawn_blocking(move || {
        let bytes = BASE64.decode(&data)
            .map_err(|e| format!("Base64 decode error: {}", e))?;
        if bytes.len() > MAX_TEMP_FILE_BYTES {
            return Err(too_large());
        }
        temp_media::store(&filename, &bytes).map(|path| path.display().to_string())
    })
    .await
    .map_err(|e| format!("Write task failed: {}", e))?
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

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ));

    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());

    builder
        .setup(|app| {
            let data_dir: PathBuf = app.path().app_data_dir()
                .expect("Failed to resolve app data directory");
            std::fs::create_dir_all(&data_dir)?;

            let db = match WorkflowDb::open(&data_dir.join("workflows.db"), 8) {
                Ok(db) => Arc::new(db),
                Err(e) => {
                    exit_with_startup_error(
                        app.handle(),
                        "Aerini cannot open its database",
                        format!("Aerini could not open its workflow database: {e}"),
                    );
                    return Ok(());
                }
            };
            let cred_db_path = data_dir.join("credentials.db");
            let cred_key_fallback = data_dir.join(".cred.key");
            // OsKeychain's Linux backend blocks the calling thread to reach the
            // secret service, and this hook runs on Tauri's own Tokio runtime —
            // do the open on a dedicated blocking thread, not here.
            let cred_store = match tauri::async_runtime::block_on(async move {
                tokio::task::spawn_blocking(move || {
                    CredentialStore::open(
                        &cred_db_path,
                        KeySource::OsKeychain { fallback: cred_key_fallback },
                    )
                })
                .await
                .expect("Credential store init task panicked")
            }) {
                Ok(store) => Arc::new(store),
                Err(e) => {
                    report_credential_store_failure(app.handle(), &e);
                    return Ok(());
                }
            };
            let mut registry = NodeRegistry::new();
            register_builtins(&mut registry, &data_dir, Some(Arc::clone(&db)));
            start_pool_eviction_task(tauri::async_runtime::handle().inner());
            let mut plugin_load_report = PluginLoadReport::default();
            if let Some(dir_str) = db.get_setting(commands::plugins::PLUGIN_DIR_SETTING).unwrap_or(None) {
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
            daemon.set_plugin_dir(commands::scheduler::configured_plugin_dir(&db));
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
            let update_notice = commands::update::consume_startup_marker(
                &data_dir,
                &app.package_info().version.to_string(),
            );
            let start_hidden = should_start_hidden(std::env::args(), update_notice.is_some());
            app.manage(StartHidden(AtomicBool::new(start_hidden)));
            app.manage(commands::update::UpdateState::new(
                commands::update::marker_path(&data_dir),
                update_notice,
            ));
            app.manage(Arc::new(Reloadable::new(plugin_load_report)));
            app.manage(Arc::new(tokio::sync::Mutex::new(())));
            app.manage(commands::providers::PullState::default());

            // Async startup cleanup of temp media files older than 24h —
            // spawn_blocking so the std::fs directory scan doesn't occupy a tokio thread.
            let temp_media_dir = temp_media::dir_path();
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

            let tray_installed = match tauri::tray::TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("app must have a window icon configured").clone())
                .menu(&tray_menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => reveal_main_window(app),
                    "quit" => quit_app(app),
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
                .build(app)
            {
                Ok(_) => true,
                Err(e) => {
                    tracing::warn!("system tray unavailable: {e}");
                    false
                }
            };
            app.manage(TrayInstalled(tray_installed));

            let hold_hidden = start_hidden && tray_installed;
            app.state::<StartHidden>().0.store(hold_hidden, Ordering::Release);

            if hold_hidden {
                #[cfg(target_os = "linux")]
                {
                    let handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        if !wait_until(desktop_has_tray_host, TRAY_HOST_WAIT, TRAY_HOST_POLL).await {
                            handle.state::<StartHidden>().0.store(false, Ordering::Release);
                            reveal_main_window(&handle);
                        }
                    });
                }
            } else {
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
            }

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
            commands::chat::list_chat_session_meta,
            commands::chat::load_chat_messages,
            commands::chat::append_chat_message,
            commands::chat::save_chat_session_meta,
            commands::chat::save_chat_session,
            commands::chat::delete_chat_session,
            commands::credentials::list_credentials,
            commands::credentials::list_credential_usage,
            commands::credentials::get_credential_metadata,
            commands::credentials::list_credential_models,
            commands::credentials::save_credential,
            commands::credentials::delete_credential,
            commands::credentials::export_encryption_key,
            commands::oauth::get_oauth_redirect_port,
            // Keep original command names for IPC compatibility with frontend
            commands::scheduler::start_scheduled_workflow,
            commands::scheduler::stop_scheduled_workflow,
            commands::scheduler::cancel_scheduled_run,
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
            commands::plugins::choose_plugin_dir,
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
            commands::update::install_update,
            commands::update::cancel_update_download,
            commands::update::take_update_notice,
            commands::update::startup_update_check,
            commands::update::get_update_check_on_startup,
            commands::update::set_update_check_on_startup,
            commands::providers::list_provider_models,
            commands::providers::ollama_probe,
            commands::providers::ollama_pull_model,
            commands::providers::ollama_cancel_pull,
            commands::providers::ollama_delete_model,
            pick_folder_dialog,
            pick_plugin_file_dialog,
            pick_file_dialog,
            write_temp_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running aerini")
}
