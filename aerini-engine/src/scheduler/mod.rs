//! Background scheduler daemon.
//!
//! [`SchedulerDaemon`] manages a map of active jobs. Each job runs in its own `tokio::spawn`
//! task. The daemon is started once at app launch and persists for the application lifetime.
//!
//! # Trigger kinds
//!
//! | Kind | Behaviour |
//! |------|-----------|
//! | `Interval` | Sleep N seconds, run, repeat |
//! | `Cron` | Parse expression, sleep until next boundary, run, repeat |
//! | `Once` | Sleep until target UTC time, run once, mark done |
//! | `Webhook` | Bind TCP port, wait for HTTP request, validate HMAC secret, run, re-arm |
//! | `Manual` | Not schedulable; `start_scheduled_workflow` returns `NotSchedulable` |
//!
//! # Webhook security
//!
//! Secret comparison uses `subtle::ConstantTimeEq`. Do not replace with `==` — timing
//! attacks are a real concern for network-accessible secrets.
//!
//! # `SchedulerDb` trait
//!
//! The daemon persists job state through [`SchedulerDb`]. In the desktop app this is
//! `WorkflowDb` (SQLite). In `aerini-server` single-workflow mode it is `MemSchedulerDb`
//! (in-memory, no persistence). Both implement the same trait — the daemon is unaware
//! of which backend it has.

pub mod job;
mod runner;

pub use job::{ScheduledJobRow, SchedulerError, SchedulerStatusEvent, TriggerKind};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use tokio::sync::Semaphore;

use crate::executor::CredentialResolver;
use crate::model::Workflow;
use crate::node::NodeRegistry;
use crate::cron::next_cron_delay_secs;
use crate::EventSink;

use job::PortConflict;

pub trait SchedulerDb: Send + Sync + 'static {
    fn scheduler_list_active(&self) -> Result<Vec<ScheduledJobRow>, String>;
    fn scheduler_list_all(&self) -> Result<Vec<ScheduledJobRow>, String>;
    /// Paginated list. Returns (items, total_count).
    /// Override for SQL-backed implementations; default falls back to list_all + Rust-side slice.
    fn scheduler_list_paginated(&self, offset: usize, limit: usize) -> Result<(Vec<ScheduledJobRow>, usize), String> {
        let all = self.scheduler_list_all()?;
        let total = all.len();
        let items = all.into_iter().skip(offset).take(limit).collect();
        Ok((items, total))
    }
    fn scheduler_upsert(&self, row: &ScheduledJobRow) -> Result<(), String>;
    fn scheduler_update_run(
        &self,
        workflow_id: &str,
        success:     bool,
        error_msg:   Option<&str>,
        next_run_at: Option<&str>,
    ) -> Result<(), String>;
    fn scheduler_set_status(&self, workflow_id: &str, status: &str) -> Result<(), String>;
    fn scheduler_clear_next_run(&self, workflow_id: &str) -> Result<(), String>;
    fn scheduler_update_next_run_at(&self, workflow_id: &str, next_run_at: &str) -> Result<(), String>;
    fn scheduler_get(&self, workflow_id: &str) -> Result<Option<ScheduledJobRow>, String>;
    fn load_workflow_json(&self, workflow_id: &str) -> Result<Option<String>, String>;
}

pub struct SchedulerDaemon {
    db:            Arc<dyn SchedulerDb>,
    registry:      Arc<NodeRegistry>,
    cred_store:    Arc<dyn CredentialResolver>,
    event_sink:    Arc<dyn EventSink>,
    jobs:          Arc<Mutex<HashMap<String, Arc<tokio::task::JoinHandle<()>>>>>,
    webhook_ports: Arc<Mutex<HashMap<u16, String>>>,
    exec_locks:    Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    env_allowlist:      Option<Arc<std::collections::HashSet<String>>>,
    shell_exec_disabled: bool,
    code_exec_disabled:  bool,
    database_exec_disabled: bool,
    /// true = this daemon's runs are fully-trusted-operator
    /// runs, unlocking node-level admin gates (e.g. `allow_raw_sql`). Set by
    /// desktop (single-tenant by definition — see `src-tauri/src/lib.rs`'s
    /// `SchedulerDaemon::new` call site); left `false` (default) on
    /// `aerini-server`, where a scheduled/webhook run has no per-caller
    /// token scope to derive trust from and must stay conservative. Mirrors
    /// `WorkflowExecutor::with_caller_is_admin` — see that method's doc
    /// comment for the full admin-gate rationale.
    caller_is_admin: bool,
    code_sandbox_enabled: bool,
    code_max_memory_mb:   Option<u64>,
    parallel_execution:   bool,
    max_concurrent_nodes: usize,
    max_concurrent_runs:  usize,
    run_semaphore:        Arc<Semaphore>,
    server_max_duration_secs: Option<u64>,
    file_sandbox_dir: Option<Arc<std::path::PathBuf>>,
    /// Set to `true` by `drain_all` to prevent new iterations from starting.
    shutting_down: Arc<AtomicBool>,
    /// In-flight `executor.run()` count. Decremented on drop via `ActiveRunGuard`.
    active_runs: Arc<AtomicUsize>,
}

impl SchedulerDaemon {
    pub fn new(
        db:         Arc<dyn SchedulerDb>,
        registry:   Arc<NodeRegistry>,
        cred_store: Arc<dyn CredentialResolver>,
        event_sink: Arc<dyn EventSink>,
    ) -> Self {
        const DEFAULT_MAX_CONCURRENT_RUNS: usize = 16;
        Self {
            db,
            registry,
            cred_store,
            event_sink,
            jobs:          Arc::new(Mutex::new(HashMap::new())),
            webhook_ports: Arc::new(Mutex::new(HashMap::new())),
            exec_locks:    Arc::new(Mutex::new(HashMap::new())),
            env_allowlist: None,
            shell_exec_disabled: false,
            code_exec_disabled:  false,
            database_exec_disabled: false,
            caller_is_admin:     false,
            code_sandbox_enabled: false,
            code_max_memory_mb:   None,
            parallel_execution:   false,
            max_concurrent_nodes: 8,
            max_concurrent_runs:  DEFAULT_MAX_CONCURRENT_RUNS,
            run_semaphore:        Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENT_RUNS)),
            server_max_duration_secs: None,
            file_sandbox_dir: None,
            shutting_down: Arc::new(AtomicBool::new(false)),
            active_runs:   Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Restrict `{{$env.VAR}}` expressions to the listed variable names.
    pub fn with_env_allowlist(mut self, vars: Vec<String>) -> Self {
        self.env_allowlist = Some(Arc::new(vars.into_iter().collect()));
        self
    }

    /// Disable Shell Command nodes for all workflows run by this scheduler.
    pub fn with_shell_disabled(mut self, disabled: bool) -> Self {
        self.shell_exec_disabled = disabled;
        self
    }

    /// Disable Code (JS) nodes for all workflows run by this scheduler.
    pub fn with_code_disabled(mut self, disabled: bool) -> Self {
        self.code_exec_disabled = disabled;
        self
    }

    /// Disable Database nodes for all workflows run by this scheduler.
    /// Recommended for multi-tenant API deployments (SSRF surface + RUSTSEC-2023-0071).
    pub fn with_database_disabled(mut self, disabled: bool) -> Self {
        self.database_exec_disabled = disabled;
        self
    }

    /// Grant admin-level node permissions (e.g. `allow_raw_sql`) to every
    /// workflow run by this scheduler. Mirrors
    /// `WorkflowExecutor::with_caller_is_admin` — only set `true` for a
    /// single-tenant caller (desktop). Default: `false`.
    pub fn with_caller_is_admin(mut self, is_admin: bool) -> Self {
        self.caller_is_admin = is_admin;
        self
    }

    /// Set the memory cap (MB) for Code (JS) node subprocesses.
    /// Only effective on Linux with sandbox enabled; no-op on macOS/Windows.
    pub fn with_code_max_memory_mb(mut self, mb: Option<u64>) -> Self {
        self.code_max_memory_mb = mb;
        self
    }

    /// Enable Code (JS) node sandboxing for all workflows run by this scheduler.
    pub fn with_code_sandbox(mut self, enabled: bool) -> Self {
        self.code_sandbox_enabled = enabled;
        self
    }

    /// Enable parallel execution of independent branches for all workflows run by
    /// this scheduler. Mirrors `WorkflowExecutor::with_parallel_execution`. Default: false.
    pub fn with_parallel_execution(mut self, enabled: bool) -> Self {
        self.parallel_execution = enabled;
        self
    }

    /// Maximum simultaneous node tasks in parallel mode. Default: 8.
    pub fn with_max_concurrent_nodes(mut self, limit: usize) -> Self {
        self.max_concurrent_nodes = limit.max(1);
        self
    }

    /// Maximum simultaneous workflow runs across all jobs. Default: 16.
    /// When the ceiling is reached, new run attempts queue (await) rather than fail.
    pub fn with_max_concurrent_runs(mut self, limit: usize) -> Self {
        let limit = limit.max(1);
        self.max_concurrent_runs = limit;
        self.run_semaphore = Arc::new(Semaphore::new(limit));
        self
    }

    /// Set a server-level ceiling on workflow execution time for all jobs run by this scheduler.
    /// Passed through to `WorkflowExecutor::with_server_max_duration_secs`.
    pub fn with_server_max_duration_secs(mut self, secs: Option<u64>) -> Self {
        self.server_max_duration_secs = secs;
        self
    }

    /// Restrict File nodes to paths within `dir` for all workflows run by this scheduler.
    pub fn with_file_sandbox_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.file_sandbox_dir = Some(Arc::new(dir));
        self
    }

    /// Called once at startup. Re-arms all always_on jobs from the DB.
    /// `rt` must be a handle to an active Tokio runtime; it is used to spawn
    /// background tasks so that this method can be called from a sync context
    /// (e.g. Tauri's setup closure) without requiring the calling thread to
    /// have entered the runtime.
    pub fn start(&self, rt: &tokio::runtime::Handle) {
        // Load all rows — not just active — so always_on jobs that were
        // explicitly stopped before the app closed are also re-armed.
        let rows = match self.db.scheduler_list_all() {
            Ok(r)  => r,
            Err(e) => { eprintln!("[scheduler] startup: failed to load jobs — {}", e); return; }
        };

        for row in rows {
            if row.always_on {
                // always_on = start on every app open, regardless of prior status.
                if row.status != "active" {
                    if let Err(e) = self.db.scheduler_set_status(&row.workflow_id, "active") {
                        self.event_sink.emit("scheduler-error", serde_json::json!({
                            "workflow_id": row.workflow_id,
                            "message": format!("Startup: failed to reset always-on job to active: {}", e)
                        }));
                        continue;
                    }
                }
                let fresh_next = self.compute_fresh_next_run(&row);
                if let Some(next) = fresh_next {
                    runner::update_next_run(&self.db, &row.workflow_id, &next);
                }
                if let Err(e) = self.arm_job_internal(&row.workflow_id, None, false, rt) {
                    runner::emit_error(&self.event_sink, &self.db, &row.workflow_id,
                               &format!("Failed to re-arm on startup: {:?}", e));
                }
            } else if row.status == "active" {
                // Non-always-on job was active when app last shut down.
                // No loop will re-arm it — mark stopped so the frontend is consistent.
                if let Err(e) = self.db.scheduler_set_status(&row.workflow_id, "stopped") {
                    self.event_sink.emit("scheduler-error", serde_json::json!({
                        "workflow_id": row.workflow_id,
                        "message": format!("Failed to mark stale job stopped: {}", e)
                    }));
                } else {
                    self.db.scheduler_clear_next_run(&row.workflow_id).ok();
                    self.emit_status(
                        &row.workflow_id,
                        "stopped",
                        row.run_count,
                        row.last_run_at.clone(),
                        None,
                        None,
                    );
                }
            }
        }
    }

    pub fn start_job(
        &self,
        workflow_id:   &str,
        port_override: Option<u16>,
        always_on:     Option<bool>,
    ) -> Result<(), SchedulerError> {
        if self.jobs.lock().expect("scheduler jobs mutex poisoned").contains_key(workflow_id) {
            return Err(SchedulerError::AlreadyRunning);
        }

        let json = self.db.load_workflow_json(workflow_id)
            .map_err(|e| SchedulerError::Other { message: e })?
            .ok_or(SchedulerError::WorkflowNotFound)?;

        let workflow = Workflow::from_json(&json)
            .map_err(|e| SchedulerError::Other { message: e.to_string() })?;

        let trigger = extract_trigger(&workflow)
            .map_err(|e| SchedulerError::Other { message: e })?;

        if !trigger.is_schedulable() {
            return Err(SchedulerError::NotSchedulable);
        }

        let trigger = match trigger {
            TriggerKind::Webhook { port, path, method, secret, dedup_window_secs } => {
                let effective_port = port_override.unwrap_or(port);
                check_webhook_port(&*self.db, &self.webhook_ports, workflow_id, effective_port)?;
                probe_port_is_free(effective_port)?;
                TriggerKind::Webhook { port: effective_port, path, method, secret, dedup_window_secs }
            }
            other => other,
        };

        let existing = self.db.scheduler_get(workflow_id).ok().flatten();
        let existing_count = existing.as_ref().map(|r| r.run_count).unwrap_or(0);
        // None = preserve existing DB value; only false when no prior row exists.
        let effective_always_on = always_on.unwrap_or_else(|| {
            existing.as_ref().map(|r| r.always_on).unwrap_or(false)
        });

        let row = ScheduledJobRow {
            workflow_id:   workflow_id.to_string(),
            workflow_name: workflow.name.clone(),
            trigger_kind:  serde_json::to_string(&trigger).unwrap_or_default(),
            status:        "active".to_string(),
            always_on:     effective_always_on,
            run_count:     existing_count,
            last_run_at:   None,
            next_run_at:   None,
            last_error:    None,
            created_at:    Utc::now().to_rfc3339(),
        };
        self.db.scheduler_upsert(&row)
            .map_err(|e| SchedulerError::Other { message: e })?;

        let armed = self.arm_job_internal(workflow_id, Some(trigger), true, &tokio::runtime::Handle::current());
        if armed.is_err() {
            // arm_job_internal re-checks the same port (covers its other caller,
            // start()'s launch-time re-arm) and can still fail here even though
            // the checks above just passed — don't leave the row claiming
            // "active" for a job that never actually armed.
            let _ = self.db.scheduler_set_status(workflow_id, "stopped");
        }
        armed
    }

    pub fn stop_job(&self, workflow_id: &str) -> Result<(), String> {
        if let Some(handle) = self.jobs.lock().expect("scheduler jobs mutex poisoned").remove(workflow_id) {
            handle.abort();
        }
        {
            let mut ports = self.webhook_ports.lock().expect("scheduler webhook_ports mutex poisoned");
            ports.retain(|_, v| v != workflow_id);
        }
        self.db.scheduler_set_status(workflow_id, "stopped")?;
        self.db.scheduler_clear_next_run(workflow_id).ok();
        let run_count = self.db.scheduler_get(workflow_id)
            .ok().flatten().map(|r| r.run_count).unwrap_or(0);
        self.emit_status(workflow_id, "stopped", run_count, None, None, None);
        Ok(())
    }

    pub fn list_jobs(&self) -> Result<Vec<ScheduledJobRow>, String> {
        self.db.scheduler_list_all()
    }

    pub fn get_job(&self, workflow_id: &str) -> Result<Option<ScheduledJobRow>, String> {
        self.db.scheduler_get(workflow_id)
    }

    pub fn list_jobs_paginated(&self, offset: usize, limit: usize) -> Result<(Vec<ScheduledJobRow>, usize), String> {
        self.db.scheduler_list_paginated(offset, limit)
    }

    pub fn stop_all(&self) {
        let workflow_ids: Vec<String> = {
            let mut jobs = self.jobs.lock().expect("scheduler jobs mutex poisoned");
            let ids: Vec<String> = jobs.keys().cloned().collect();
            for (_, handle) in jobs.drain() {
                handle.abort();
            }
            ids
        };
        {
            let mut ports = self.webhook_ports.lock().expect("scheduler webhook_ports mutex poisoned");
            ports.clear();
        }
        for wf_id in &workflow_ids {
            let _ = self.db.scheduler_set_status(wf_id, "stopped");
            let _ = self.db.scheduler_clear_next_run(wf_id);
        }
    }

    pub fn set_always_on(&self, workflow_id: &str, always_on: bool) -> Result<(), String> {
        let mut row = self.db.scheduler_get(workflow_id)?
            .ok_or_else(|| format!("Workflow '{}' not in scheduler", workflow_id))?;
        row.always_on = always_on;
        self.db.scheduler_upsert(&row)?;
        Ok(())
    }

    /// Re-emits current `scheduler-status` for every active job, then emits
    /// `scheduler-ready`. Called by the `request_scheduler_state` Tauri command
    /// after the frontend has registered its `scheduler-status` listener.
    /// This replaces the old 800 ms startup delay.
    pub fn replay_state(&self) {
        let rows = match self.db.scheduler_list_active() {
            Ok(r)  => r,
            Err(e) => { eprintln!("[scheduler] replay_state: {}", e); return; }
        };
        let job_count = rows.len();
        for row in &rows {
            let next_run: Option<DateTime<Utc>> = row.next_run_at.as_ref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            runner::emit_waiting(&self.event_sink, &self.db, &row.workflow_id, next_run);
        }
        self.event_sink.emit("scheduler-ready", serde_json::json!({
            "job_count": job_count,
            "timestamp": Utc::now().to_rfc3339(),
        }));
    }

    fn compute_fresh_next_run(&self, row: &ScheduledJobRow) -> Option<DateTime<Utc>> {
        let trigger: TriggerKind = serde_json::from_str(&row.trigger_kind).ok()?;
        match trigger {
            TriggerKind::Interval { secs } => {
                Utc::now().checked_add_signed(chrono::Duration::seconds(secs as i64))
            }
            TriggerKind::Cron { ref expr } => {
                let delay = next_cron_delay_secs(expr, &Utc::now()).ok()?;
                Utc::now().checked_add_signed(chrono::Duration::seconds(delay as i64))
            }
            TriggerKind::Once { run_at } => Some(run_at),
            TriggerKind::Webhook { .. } | TriggerKind::Manual => None,
        }
    }

    fn arm_job_internal(
        &self,
        workflow_id:      &str,
        trigger_override: Option<TriggerKind>,
        fire_immediately: bool,
        rt:               &tokio::runtime::Handle,
    ) -> Result<(), SchedulerError> {
        let db         = Arc::clone(&self.db);
        let registry   = Arc::clone(&self.registry);
        let cred_store = Arc::clone(&self.cred_store);
        let event_sink = Arc::clone(&self.event_sink);
        let jobs_map   = Arc::clone(&self.jobs);
        let ports_map  = Arc::clone(&self.webhook_ports);
        let wf_id      = workflow_id.to_string();
        let env_allowlist = self.env_allowlist.clone();
        let shell_exec_disabled = self.shell_exec_disabled;
        let code_exec_disabled  = self.code_exec_disabled;
        let database_exec_disabled = self.database_exec_disabled;
        let caller_is_admin = self.caller_is_admin;
        let code_sandbox_enabled = self.code_sandbox_enabled;
        let code_max_memory_mb   = self.code_max_memory_mb;
        let parallel_execution   = self.parallel_execution;
        let max_concurrent_nodes = self.max_concurrent_nodes;
        let server_max_duration_secs = self.server_max_duration_secs;
        let file_sandbox_dir = self.file_sandbox_dir.clone();
        let run_semaphore = Arc::clone(&self.run_semaphore);
        let shutting_down = Arc::clone(&self.shutting_down);
        let active_runs   = Arc::clone(&self.active_runs);

        let trigger = if let Some(t) = trigger_override {
            t
        } else {
            let row = db.scheduler_get(&wf_id)
                .map_err(|e| SchedulerError::Other { message: e })?
                .ok_or(SchedulerError::WorkflowNotFound)?;
            serde_json::from_str::<TriggerKind>(&row.trigger_kind)
                .map_err(|e| SchedulerError::Other { message: e.to_string() })?
        };

        // Logs the same dangerous-node signal at every execution entry point,
        // including the scheduled/always-on path, mirroring the identical
        // check in `run_workflow` for a manual run — so a dangerous node
        // type never executes unattended without this warning. Reached by
        // both `start_job` (user clicks Start) and `start()` (always_on jobs
        // re-armed at app launch), so one check here covers both without
        // threading a new parameter through either call site. Warn-only —
        // does not block; desktop's full-host-access default is unchanged.
        // One extra `load_workflow_json` + parse per arm (not per execution
        // tick) — acceptable at this frequency.
        match db.load_workflow_json(&wf_id) {
            Ok(Some(json)) => match Workflow::from_json(&json) {
                Ok(wf) => {
                    let dangerous = crate::nodes::dangerous_node_types_present(&wf.nodes);
                    if !dangerous.is_empty() {
                        tracing::warn!(
                            workflow_id = %wf_id,
                            node_types  = ?dangerous,
                            "scheduler: workflow contains dangerous node type(s) {:?} — will \
                             execute unattended on every scheduled/always-on run.",
                            dangerous
                        );
                    }
                }
                Err(e) => {
                    // A corrupted/unparseable stored workflow JSON must leave a
                    // trace rather than silently skipping the dangerous-node
                    // check with no diagnostic. Diagnostic-only — arming still
                    // proceeds unchanged below.
                    tracing::warn!(
                        workflow_id = %wf_id,
                        error = %e,
                        "scheduler: could not parse workflow JSON while checking for dangerous \
                         node types — dangerous-node warning skipped for this arm (diagnostic \
                         only, does not block scheduling)."
                    );
                }
            },
            Ok(None) => {
                tracing::warn!(
                    workflow_id = %wf_id,
                    "scheduler: no workflow JSON found while checking for dangerous node types \
                     — dangerous-node warning skipped for this arm (diagnostic only, does not \
                     block scheduling)."
                );
            }
            Err(e) => {
                tracing::warn!(
                    workflow_id = %wf_id,
                    error = %e,
                    "scheduler: failed to load workflow JSON while checking for dangerous node \
                     types — dangerous-node warning skipped for this arm (diagnostic only, does \
                     not block scheduling)."
                );
            }
        }

        if let TriggerKind::Webhook { port, .. } = &trigger {
            claim_webhook_port(&*db, &ports_map, &wf_id, *port)?;
        }

        let exec_lock = {
            let mut locks = self.exec_locks.lock().expect("scheduler exec_locks mutex poisoned");
            Arc::clone(locks.entry(wf_id.clone()).or_insert_with(||
                Arc::new(tokio::sync::Mutex::new(()))
            ))
        };

        let join_handle = Arc::new(rt.spawn(async move {
            runner::run_job_loop(
                wf_id.clone(), trigger, db, registry, cred_store,
                event_sink, exec_lock, fire_immediately, env_allowlist,
                shell_exec_disabled, code_exec_disabled, database_exec_disabled, caller_is_admin, code_sandbox_enabled,
                code_max_memory_mb, parallel_execution, max_concurrent_nodes,
                server_max_duration_secs, file_sandbox_dir, run_semaphore,
                shutting_down, active_runs,
            ).await;
            jobs_map.lock().expect("scheduler jobs mutex poisoned").remove(&wf_id);
        }));

        self.jobs.lock().expect("scheduler jobs mutex poisoned").insert(workflow_id.to_string(), join_handle);
        Ok(())
    }

    /// Returns the current count of in-flight `executor.run()` calls.
    pub fn active_runs(&self) -> usize {
        self.active_runs.load(Ordering::SeqCst)
    }

    /// Graceful drain: stops new iterations and waits for in-flight runs to finish.
    ///
    /// Sets `shutting_down = true` so each trigger loop exits at its next check point.
    /// Polls `active_runs` every 100 ms. Timeout is taken from `server_max_duration_secs`
    /// (the `--max-workflow-duration-secs` flag); defaults to 30 s when not set.
    /// On timeout, falls back to `stop_all()` so the process never hangs on SIGTERM.
    ///
    /// **Does not affect `stop_job` or `stop_all` semantics** — those remain immediate
    /// hard-abort and are unchanged for the desktop Stop button and the API stop endpoint.
    pub async fn drain_all(&self) {
        use std::time::{Duration, Instant};
        self.shutting_down.store(true, Ordering::SeqCst);
        let timeout = self.server_max_duration_secs
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(30));
        let deadline = Instant::now() + timeout;
        loop {
            if self.active_runs.load(Ordering::SeqCst) == 0 {
                break;
            }
            if Instant::now() >= deadline {
                tracing::warn!(
                    "[scheduler] drain_all: timeout ({:?}) reached — aborting remaining runs",
                    timeout
                );
                self.stop_all();
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn emit_status(
        &self,
        workflow_id: &str,
        status:      &str,
        run_count:   i64,
        last_run_at: Option<String>,
        next_run_at: Option<String>,
        last_error:  Option<String>,
    ) {
        let workflow_name = self.db.scheduler_get(workflow_id)
            .ok().flatten().map(|r| r.workflow_name)
            .unwrap_or_else(|| workflow_id.to_string());

        let payload = SchedulerStatusEvent {
            workflow_id:   workflow_id.to_string(),
            workflow_name,
            status:        status.to_string(),
            run_count,
            last_run_at,
            next_run_at,
            last_error,
            last_result:   None,
        };
        self.event_sink.emit("scheduler-status",
            serde_json::to_value(payload).unwrap_or_default());
    }
}

/// Checks the webhook port registry for a conflict with a workflow other
/// than `workflow_id`. A port already held by `workflow_id` itself is not a
/// conflict — re-claiming its own port (e.g. on re-arm) is expected. Used by
/// both `start_job` (interactive start) and `arm_job_internal` (covers both
/// that same call and the launch-time re-arm path in `start()`), so a port
/// conflict is caught the same way regardless of caller.
fn check_webhook_port(
    db:          &dyn SchedulerDb,
    ports:       &Mutex<HashMap<u16, String>>,
    workflow_id: &str,
    port:        u16,
) -> Result<(), SchedulerError> {
    let ports = ports.lock().expect("scheduler webhook_ports mutex poisoned");
    if let Some(holder_id) = ports.get(&port) {
        if holder_id != workflow_id {
            let holder_name = db.scheduler_get(holder_id)
                .ok().flatten()
                .map(|r| r.workflow_name)
                .unwrap_or_else(|| holder_id.clone());
            return Err(SchedulerError::PortConflict(PortConflict {
                port,
                held_by_workflow_id:   holder_id.clone(),
                held_by_workflow_name: holder_name,
            }));
        }
    }
    Ok(())
}

/// Real OS-level probe for whether `port` can actually be bound right now —
/// not just absent from our own in-memory `webhook_ports` registry (see
/// `check_webhook_port` above, which only catches a conflict between two of
/// *our own* scheduled jobs). Two things `check_webhook_port` alone cannot
/// see: an ad-hoc `run_workflow` execution of the Webhook node itself
/// (aerini-engine/src/nodes/webhook.rs, tracked in its own process-global
/// `ACTIVE_PORTS` set, entirely separate from this module), and any
/// unrelated process on the machine.
///
/// Binding here and dropping immediately doesn't fully close the race —
/// another process could still grab the port in the moment between this
/// probe and `run_job_loop`'s real bind — but it converts the common case
/// (the port is already held when Start is clicked) into an immediate,
/// synchronous error the caller can act on right away, the same way a
/// `check_webhook_port` conflict already is. `claim_webhook_port` below
/// calls this while still holding the registry lock, closing that same gap
/// against a second *scheduler-side* claim the way its own insert already
/// does — only an external, non-scheduler process can still win the
/// remaining, narrower race.
fn probe_port_is_free(port: u16) -> Result<(), SchedulerError> {
    std::net::TcpListener::bind(("127.0.0.1", port))
        .map(|_| ())
        .map_err(|e| SchedulerError::PortConflict(PortConflict {
            port,
            held_by_workflow_id:   String::new(),
            held_by_workflow_name: format!("another process on this machine ({e})"),
        }))
}

/// Checks the in-memory registry for a conflict with another workflow,
/// probes that the port is actually free at the OS level, and claims it for
/// `workflow_id` — all under one lock acquisition, so two concurrent arms
/// targeting the same free port can't both pass the checks before either
/// inserts. Used only by `arm_job_internal`, which always needs the insert
/// to follow a clean check; `start_job`'s own earlier pre-check has no
/// insert and keeps using `check_webhook_port` (plus its own, separately
/// non-atomic `probe_port_is_free` call — a fail-fast UX hint doesn't need
/// this function's stronger guarantee).
fn claim_webhook_port(
    db:          &dyn SchedulerDb,
    ports:       &Mutex<HashMap<u16, String>>,
    workflow_id: &str,
    port:        u16,
) -> Result<(), SchedulerError> {
    let mut ports = ports.lock().expect("scheduler webhook_ports mutex poisoned");
    if let Some(holder_id) = ports.get(&port) {
        if holder_id != workflow_id {
            let holder_name = db.scheduler_get(holder_id)
                .ok().flatten()
                .map(|r| r.workflow_name)
                .unwrap_or_else(|| holder_id.clone());
            return Err(SchedulerError::PortConflict(PortConflict {
                port,
                held_by_workflow_id:   holder_id.clone(),
                held_by_workflow_name: holder_name,
            }));
        }
    }
    probe_port_is_free(port)?;
    ports.insert(port, workflow_id.to_string());
    Ok(())
}

pub fn extract_trigger(workflow: &Workflow) -> Result<TriggerKind, String> {
    let has_incoming: std::collections::HashSet<&str> = workflow.edges
        .iter()
        .map(|e| e.to_node.as_str())
        .collect();

    let entry_nodes: Vec<_> = workflow.nodes.iter()
        .filter(|n| !has_incoming.contains(n.id.as_str()))
        .collect();

    if entry_nodes.is_empty() {
        return Err("Workflow has no entry node".to_string());
    }

    let node = entry_nodes[0];

    match node.node_type_id.as_str() {
        "schedule" => {
            let mode = node.config["mode"].as_str().unwrap_or("interval");
            match mode {
                "interval" => {
                    let secs = node.config["interval_secs"].as_u64().unwrap_or(60);
                    Ok(TriggerKind::Interval { secs: secs.max(10) })
                }
                "cron" => {
                    let expr = node.config["cron_expr"].as_str()
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "cron_expr is required for cron mode".to_string())?;
                    Ok(TriggerKind::Cron { expr: expr.to_string() })
                }
                "once" => {
                    let run_at_str = node.config["run_at"].as_str()
                        .ok_or_else(|| "run_at is required for once mode".to_string())?;
                    let run_at = chrono::DateTime::parse_from_rfc3339(run_at_str)
                        .map_err(|e| format!("Invalid run_at timestamp: {}", e))?
                        .with_timezone(&Utc);
                    Ok(TriggerKind::Once { run_at })
                }
                other => Err(format!("Unknown schedule mode: {}", other)),
            }
        }
        "webhook" => {
            let port   = node.config["port"].as_u64().unwrap_or(3456) as u16;
            let path   = node.config["path"].as_str().unwrap_or("/webhook").to_string();
            let method = node.config["method"].as_str().unwrap_or("ANY").to_string();
            let secret = node.config["secret"].as_str().unwrap_or("").to_string();
            let dedup_window_secs = node.config["dedup_window_secs"].as_u64().unwrap_or(0);
            Ok(TriggerKind::Webhook { port, path, method, secret, dedup_window_secs })
        }
        "manual_trigger" => Ok(TriggerKind::Manual),
        other => Err(format!(
            "Node type '{}' is not a recognised trigger. \
             Add a Schedule, Webhook, or Manual Trigger node as the first node.",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockDb {
        rows: HashMap<String, ScheduledJobRow>,
    }

    impl MockDb {
        fn with_row(workflow_id: &str, workflow_name: &str) -> Self {
            let mut rows = HashMap::new();
            rows.insert(workflow_id.to_string(), ScheduledJobRow {
                workflow_id:   workflow_id.to_string(),
                workflow_name: workflow_name.to_string(),
                trigger_kind:  "{}".to_string(),
                status:        "active".to_string(),
                always_on:     false,
                run_count:     0,
                last_run_at:   None,
                next_run_at:   None,
                last_error:    None,
                created_at:    "2026-01-01T00:00:00Z".to_string(),
            });
            MockDb { rows }
        }
    }

    impl SchedulerDb for MockDb {
        fn scheduler_list_active(&self) -> Result<Vec<ScheduledJobRow>, String> { Ok(vec![]) }
        fn scheduler_list_all(&self) -> Result<Vec<ScheduledJobRow>, String> { Ok(vec![]) }
        fn scheduler_upsert(&self, _row: &ScheduledJobRow) -> Result<(), String> { Ok(()) }
        fn scheduler_update_run(
            &self,
            _workflow_id: &str,
            _success:     bool,
            _error_msg:   Option<&str>,
            _next_run_at: Option<&str>,
        ) -> Result<(), String> { Ok(()) }
        fn scheduler_set_status(&self, _workflow_id: &str, _status: &str) -> Result<(), String> { Ok(()) }
        fn scheduler_clear_next_run(&self, _workflow_id: &str) -> Result<(), String> { Ok(()) }
        fn scheduler_update_next_run_at(&self, _workflow_id: &str, _next_run_at: &str) -> Result<(), String> { Ok(()) }
        fn scheduler_get(&self, workflow_id: &str) -> Result<Option<ScheduledJobRow>, String> {
            Ok(self.rows.get(workflow_id).cloned())
        }
        fn load_workflow_json(&self, _workflow_id: &str) -> Result<Option<String>, String> { Ok(None) }
    }

    #[test]
    fn check_webhook_port_allows_free_port() {
        let db = MockDb::with_row("wf-a", "Workflow A");
        let ports: Mutex<HashMap<u16, String>> = Mutex::new(HashMap::new());
        assert!(check_webhook_port(&db, &ports, "wf-a", 4000).is_ok());
    }

    #[test]
    fn check_webhook_port_allows_reclaiming_own_port() {
        let db = MockDb::with_row("wf-a", "Workflow A");
        let mut map = HashMap::new();
        map.insert(4000u16, "wf-a".to_string());
        let ports = Mutex::new(map);
        assert!(check_webhook_port(&db, &ports, "wf-a", 4000).is_ok());
    }

    #[test]
    fn check_webhook_port_rejects_conflict_with_other_workflow() {
        let db = MockDb::with_row("wf-b", "Workflow B");
        let mut map = HashMap::new();
        map.insert(4000u16, "wf-b".to_string());
        let ports = Mutex::new(map);
        match check_webhook_port(&db, &ports, "wf-a", 4000) {
            Err(SchedulerError::PortConflict(conflict)) => {
                assert_eq!(conflict.port, 4000);
                assert_eq!(conflict.held_by_workflow_id, "wf-b");
                assert_eq!(conflict.held_by_workflow_name, "Workflow B");
            }
            other => panic!("expected PortConflict, got {:?}", other),
        }
    }

    #[test]
    fn claim_webhook_port_claims_free_port_and_updates_registry() {
        // Real bind now happens inside claim_webhook_port (the OS-level
        // probe) — get a port the OS confirms is free right now rather than
        // a hardcoded number, so this doesn't flake if something else on the
        // machine happens to hold it.
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("failed to bind an ephemeral port for the test")
            .local_addr().expect("failed to read local_addr").port();
        let db = MockDb::with_row("wf-a", "Workflow A");
        let ports: Mutex<HashMap<u16, String>> = Mutex::new(HashMap::new());
        assert!(claim_webhook_port(&db, &ports, "wf-a", port).is_ok());
        assert_eq!(
            ports.lock().unwrap().get(&port),
            Some(&"wf-a".to_string()),
            "a successful claim must actually insert — this only holds if the check \
             and the insert happen under the same lock acquisition"
        );
    }

    #[test]
    fn claim_webhook_port_conflict_does_not_insert() {
        let db = MockDb::with_row("wf-b", "Workflow B");
        let mut map = HashMap::new();
        map.insert(4000u16, "wf-b".to_string());
        let ports = Mutex::new(map);
        match claim_webhook_port(&db, &ports, "wf-a", 4000) {
            Err(SchedulerError::PortConflict(conflict)) => {
                assert_eq!(conflict.held_by_workflow_id, "wf-b");
            }
            other => panic!("expected PortConflict, got {:?}", other),
        }
        assert_eq!(
            ports.lock().unwrap().get(&4000),
            Some(&"wf-b".to_string()),
            "a rejected claim must not overwrite the existing holder"
        );
    }

    /// Normal case: a port nothing is listening on passes the probe. Uses
    /// port 0 (OS picks a free one) rather than a hardcoded port number —
    /// this test suite may run in parallel with others on the same machine.
    #[test]
    fn probe_port_is_free_accepts_a_genuinely_free_port() {
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("failed to bind an ephemeral port for the test")
            .local_addr().expect("failed to read local_addr").port();
        assert!(probe_port_is_free(port).is_ok());
    }

    /// Edge case check_webhook_port alone can never catch: a port genuinely
    /// held by a real listener (standing in for the ad-hoc Run path's own
    /// listener, or any other process) — not just one recorded in our own
    /// in-memory `webhook_ports` map.
    #[test]
    fn probe_port_is_free_rejects_a_port_held_by_a_real_listener() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("failed to bind an ephemeral port for the test");
        let port = listener.local_addr().expect("failed to read local_addr").port();

        match probe_port_is_free(port) {
            Err(SchedulerError::PortConflict(conflict)) => assert_eq!(conflict.port, port),
            other => panic!("expected PortConflict for a port a live listener already holds, got {:?}", other),
        }
    }

    /// Proves the two merged mechanisms are actually wired together: a port
    /// absent from the in-memory registry (so `check_webhook_port` alone
    /// would pass it) but genuinely held by a real listener must still be
    /// rejected by `claim_webhook_port`, and rejection must not insert.
    #[test]
    fn claim_webhook_port_rejects_when_os_level_probe_fails_and_does_not_insert() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("failed to bind an ephemeral port for the test");
        let port = listener.local_addr().expect("failed to read local_addr").port();

        let db = MockDb::with_row("wf-a", "Workflow A");
        let ports: Mutex<HashMap<u16, String>> = Mutex::new(HashMap::new());
        match claim_webhook_port(&db, &ports, "wf-a", port) {
            Err(SchedulerError::PortConflict(conflict)) => assert_eq!(conflict.port, port),
            other => panic!("expected PortConflict for a port a live listener already holds, got {:?}", other),
        }
        assert_eq!(
            ports.lock().unwrap().get(&port),
            None,
            "a claim rejected by the OS-level probe must not insert into the registry"
        );
    }
}
