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
//! `WorkflowDb` (SQLite). In `flowo-server` single-workflow mode it is `MemSchedulerDb`
//! (in-memory, no persistence). Both implement the same trait — the daemon is unaware
//! of which backend it has.

pub mod job;

pub use job::{ScheduledJobRow, SchedulerError, SchedulerStatusEvent, TriggerKind};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde_json::Value;
use subtle::ConstantTimeEq;

use crate::executor::{CredentialResolver, WorkflowExecutor};
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
    code_sandbox_enabled: bool,
    parallel_execution:   bool,
    max_concurrent_nodes: usize,
    server_max_duration_secs: Option<u64>,
    file_sandbox_dir: Option<Arc<std::path::PathBuf>>,
}

impl SchedulerDaemon {
    pub fn new(
        db:         Arc<dyn SchedulerDb>,
        registry:   Arc<NodeRegistry>,
        cred_store: Arc<dyn CredentialResolver>,
        event_sink: Arc<dyn EventSink>,
    ) -> Self {
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
            code_sandbox_enabled: false,
            parallel_execution:   false,
            max_concurrent_nodes: 8,
            server_max_duration_secs: None,
            file_sandbox_dir: None,
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
                    update_next_run(&self.db, &row.workflow_id, &next);
                }
                if let Err(e) = self.arm_job_internal(&row.workflow_id, None, false, rt) {
                    emit_error(&self.event_sink, &self.db, &row.workflow_id,
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
            TriggerKind::Webhook { port, path, method, secret } => {
                let effective_port = port_override.unwrap_or(port);
                let ports = self.webhook_ports.lock().expect("scheduler webhook_ports mutex poisoned");
                if let Some(holder_id) = ports.get(&effective_port) {
                    if holder_id != workflow_id {
                        let holder_name = self.db.scheduler_get(holder_id)
                            .ok().flatten()
                            .map(|r| r.workflow_name)
                            .unwrap_or_else(|| holder_id.clone());
                        return Err(SchedulerError::PortConflict(PortConflict {
                            port: effective_port,
                            held_by_workflow_id:   holder_id.clone(),
                            held_by_workflow_name: holder_name,
                        }));
                    }
                }
                drop(ports);
                TriggerKind::Webhook { port: effective_port, path, method, secret }
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

        self.arm_job_internal(workflow_id, Some(trigger), true, &tokio::runtime::Handle::current())
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
            emit_waiting(&self.event_sink, &self.db, &row.workflow_id, next_run);
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
        let code_sandbox_enabled = self.code_sandbox_enabled;
        let parallel_execution   = self.parallel_execution;
        let max_concurrent_nodes = self.max_concurrent_nodes;
        let server_max_duration_secs = self.server_max_duration_secs;
        let file_sandbox_dir = self.file_sandbox_dir.clone();

        let trigger = if let Some(t) = trigger_override {
            t
        } else {
            let row = db.scheduler_get(&wf_id)
                .map_err(|e| SchedulerError::Other { message: e })?
                .ok_or(SchedulerError::WorkflowNotFound)?;
            serde_json::from_str::<TriggerKind>(&row.trigger_kind)
                .map_err(|e| SchedulerError::Other { message: e.to_string() })?
        };

        if let TriggerKind::Webhook { port, .. } = &trigger {
            ports_map.lock().expect("scheduler webhook_ports mutex poisoned").insert(*port, wf_id.clone());
        }

        let exec_lock = {
            let mut locks = self.exec_locks.lock().expect("scheduler exec_locks mutex poisoned");
            Arc::clone(locks.entry(wf_id.clone()).or_insert_with(||
                Arc::new(tokio::sync::Mutex::new(()))
            ))
        };

        let join_handle = Arc::new(rt.spawn(async move {
            run_job_loop(
                wf_id.clone(), trigger, db, registry, cred_store,
                event_sink, exec_lock, fire_immediately, env_allowlist,
                shell_exec_disabled, code_exec_disabled, code_sandbox_enabled,
                parallel_execution, max_concurrent_nodes,
                server_max_duration_secs, file_sandbox_dir,
            ).await;
            jobs_map.lock().expect("scheduler jobs mutex poisoned").remove(&wf_id);
        }));

        self.jobs.lock().expect("scheduler jobs mutex poisoned").insert(workflow_id.to_string(), join_handle);
        Ok(())
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

#[allow(clippy::too_many_arguments)]
async fn run_job_loop(
    workflow_id:      String,
    trigger:          TriggerKind,
    db:               Arc<dyn SchedulerDb>,
    registry:         Arc<NodeRegistry>,
    cred_store:       Arc<dyn CredentialResolver>,
    event_sink:       Arc<dyn EventSink>,
    exec_lock:        Arc<tokio::sync::Mutex<()>>,
    fire_immediately: bool,
    env_allowlist:    Option<Arc<std::collections::HashSet<String>>>,
    shell_exec_disabled:  bool,
    code_exec_disabled:   bool,
    code_sandbox_enabled: bool,
    parallel_execution:   bool,
    max_concurrent_nodes: usize,
    server_max_duration_secs: Option<u64>,
    file_sandbox_dir: Option<Arc<std::path::PathBuf>>,
) {
    match trigger {
        TriggerKind::Interval { secs } => {
            if fire_immediately {
                if let Ok(_guard) = exec_lock.try_lock() {
                    fire_once(&workflow_id, &db, &registry, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, code_sandbox_enabled, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir).await;
                } else {
                    log_skip(&event_sink, &workflow_id, "previous run still in progress");
                }
            }
            loop {
                let next = Utc::now()
                    .checked_add_signed(chrono::Duration::seconds(secs as i64))
                    .unwrap_or_else(Utc::now);
                update_next_run(&db, &workflow_id, &next);
                emit_waiting(&event_sink, &db, &workflow_id, Some(next));

                tokio::time::sleep(std::time::Duration::from_secs(secs)).await;

                if let Ok(_guard) = exec_lock.try_lock() {
                    fire_once(&workflow_id, &db, &registry, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, code_sandbox_enabled, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir).await;
                } else {
                    log_skip(&event_sink, &workflow_id, "previous run still in progress");
                }
            }
        }

        TriggerKind::Cron { ref expr } => {
            if fire_immediately {
                if let Ok(_guard) = exec_lock.try_lock() {
                    fire_once(&workflow_id, &db, &registry, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, code_sandbox_enabled, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir).await;
                } else {
                    log_skip(&event_sink, &workflow_id, "previous run still in progress");
                }
            }
            loop {
                let now = Utc::now();
                let delay_secs = match next_cron_delay_secs(expr, &now) {
                    Ok(d)  => d,
                    Err(e) => {
                        emit_error(&event_sink, &db, &workflow_id, &e);
                        db.scheduler_set_status(&workflow_id, "error").ok();
                        break;
                    }
                };
                let next = now
                    .checked_add_signed(chrono::Duration::seconds(delay_secs as i64))
                    .unwrap_or(now);
                update_next_run(&db, &workflow_id, &next);
                emit_waiting(&event_sink, &db, &workflow_id, Some(next));

                tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;

                if let Ok(_guard) = exec_lock.try_lock() {
                    fire_once(&workflow_id, &db, &registry, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, code_sandbox_enabled, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir).await;
                } else {
                    log_skip(&event_sink, &workflow_id, "previous run still in progress");
                }
            }
        }

        TriggerKind::Once { run_at } => {
            let now   = Utc::now();
            let delay = run_at.signed_duration_since(now);
            if delay.num_milliseconds() > 0 {
                update_next_run(&db, &workflow_id, &run_at);
                emit_waiting(&event_sink, &db, &workflow_id, Some(run_at));
                tokio::time::sleep(
                    std::time::Duration::from_millis(delay.num_milliseconds() as u64)
                ).await;
            } else {
                event_sink.emit("scheduler-warning", serde_json::json!({
                    "workflow_id": workflow_id,
                    "message": format!(
                        "Scheduled time ({}) has already passed — running immediately.",
                        run_at.format("%Y-%m-%d %H:%M UTC")
                    )
                }));
            }
            fire_once(&workflow_id, &db, &registry, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, code_sandbox_enabled, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir).await;
            db.scheduler_set_status(&workflow_id, "done").ok();
            emit_done(&event_sink, &db, &workflow_id);
        }

        TriggerKind::Webhook { port, path, method, secret } => {
            let listener = match tokio::net::TcpListener::bind(
                format!("127.0.0.1:{}", port)
            ).await {
                Ok(l)  => l,
                Err(e) => {
                    let msg = format!("Failed to bind port {}: {}", port, e);
                    emit_error(&event_sink, &db, &workflow_id, &msg);
                    db.scheduler_set_status(&workflow_id, "error").ok();
                    return;
                }
            };

            emit_waiting(&event_sink, &db, &workflow_id, None);

            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(s)  => s,
                    Err(e) => {
                        event_sink.emit("scheduler-error", serde_json::json!({
                            "workflow_id": workflow_id,
                            "message": format!("Webhook accept error: {}", e)
                        }));
                        continue;
                    }
                };

                let payload = match tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    parse_http_request(&mut stream, &path, &method, &secret),
                ).await {
                    Ok(Ok(p))  => p,
                    Ok(Err(e)) => {
                        use tokio::io::AsyncWriteExt;
                        let _ = stream.write_all(e.as_bytes()).await;
                        continue;
                    }
                    Err(_) => {
                        use tokio::io::AsyncWriteExt;
                        let _ = stream.write_all(
                            b"HTTP/1.1 408 Request Timeout\r\n\r\n"
                        ).await;
                        continue;
                    }
                };

                {
                    use tokio::io::AsyncWriteExt;
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK"
                    ).await;
                }

                let _guard = match exec_lock.try_lock() {
                    Err(_) => {
                        log_skip(&event_sink, &workflow_id, "previous run still in progress");
                        continue;
                    }
                    Ok(g) => g,
                };
                fire_once_with_vars(
                    &workflow_id, &db, &registry, &cred_store, &event_sink, payload, &env_allowlist,
                    shell_exec_disabled, code_exec_disabled, code_sandbox_enabled, parallel_execution, max_concurrent_nodes,
                    server_max_duration_secs, &file_sandbox_dir,
                ).await;

                emit_waiting(&event_sink, &db, &workflow_id, None);
            }
        }

        TriggerKind::Manual => {
            event_sink.emit("scheduler-error", serde_json::json!({
                "workflow_id": workflow_id,
                "message": "Manual trigger reached job loop — this is a bug"
            }));
        }
    }
}

async fn fire_once(
    workflow_id:          &str,
    db:                   &Arc<dyn SchedulerDb>,
    registry:             &Arc<NodeRegistry>,
    cred_store:           &Arc<dyn CredentialResolver>,
    event_sink:           &Arc<dyn EventSink>,
    env_allowlist:        &Option<Arc<std::collections::HashSet<String>>>,
    shell_exec_disabled:  bool,
    code_exec_disabled:   bool,
    code_sandbox_enabled: bool,
    parallel_execution:   bool,
    max_concurrent_nodes: usize,
    server_max_duration_secs: Option<u64>,
    file_sandbox_dir:     &Option<Arc<std::path::PathBuf>>,
) {
    fire_once_with_vars(workflow_id, db, registry, cred_store, event_sink,
        std::collections::HashMap::new(), env_allowlist,
        shell_exec_disabled, code_exec_disabled, code_sandbox_enabled, parallel_execution, max_concurrent_nodes,
        server_max_duration_secs, file_sandbox_dir,
    ).await;
}

async fn fire_once_with_vars(
    workflow_id:          &str,
    db:                   &Arc<dyn SchedulerDb>,
    registry:             &Arc<NodeRegistry>,
    cred_store:           &Arc<dyn CredentialResolver>,
    event_sink:           &Arc<dyn EventSink>,
    vars:                 std::collections::HashMap<String, Value>,
    env_allowlist:        &Option<Arc<std::collections::HashSet<String>>>,
    shell_exec_disabled:  bool,
    code_exec_disabled:   bool,
    code_sandbox_enabled: bool,
    parallel_execution:   bool,
    max_concurrent_nodes: usize,
    server_max_duration_secs: Option<u64>,
    file_sandbox_dir:     &Option<Arc<std::path::PathBuf>>,
) {
    {
        let row   = db.scheduler_get(workflow_id).ok().flatten();
        let count = row.as_ref().map(|r| r.run_count).unwrap_or(0);
        let name  = row.map(|r| r.workflow_name).unwrap_or_else(|| workflow_id.to_string());
        event_sink.emit("scheduler-status", serde_json::to_value(SchedulerStatusEvent {
            workflow_id:   workflow_id.to_string(),
            workflow_name: name,
            status:        "running".to_string(),
            run_count:     count,
            last_run_at:   None,
            next_run_at:   None,
            last_error:    None,
            last_result:   None,
        }).unwrap_or_default());
    }

    let json = match db.load_workflow_json(workflow_id) {
        Ok(Some(j)) => j,
        Ok(None) => {
            emit_error(event_sink, db, workflow_id, "Workflow deleted from database");
            return;
        }
        Err(e) => {
            emit_error(event_sink, db, workflow_id, &e);
            return;
        }
    };

    let workflow = match Workflow::from_json(&json) {
        Ok(w)  => w,
        Err(e) => {
            emit_error(event_sink, db, workflow_id, &e.to_string());
            return;
        }
    };

    let mut executor = WorkflowExecutor::new(Arc::clone(registry), Arc::clone(cred_store))
        .with_event_sink(Arc::clone(event_sink));
    if let Some(allowlist) = env_allowlist {
        executor = executor.with_env_allowlist(allowlist.iter().cloned().collect());
    }
    if shell_exec_disabled {
        executor = executor.with_shell_disabled(true);
    }
    if code_exec_disabled {
        executor = executor.with_code_disabled(true);
    }
    if code_sandbox_enabled {
        executor = executor.with_code_sandbox(true);
    }
    if parallel_execution {
        executor = executor
            .with_parallel_execution(true)
            .with_max_concurrent_nodes(max_concurrent_nodes);
    }
    if server_max_duration_secs.is_some() {
        executor = executor.with_server_max_duration_secs(server_max_duration_secs);
    }
    if let Some(ref sandbox) = file_sandbox_dir {
        executor = executor.with_file_sandbox_dir(sandbox.as_ref().clone());
    }

    let now_str = Utc::now().to_rfc3339();
    let result  = executor.run(Arc::new(workflow), vars).await;

    match result {
        Ok(r) => {
            let success = r.success;
            let err_msg = r.error.clone();
            db.scheduler_update_run(workflow_id, success, err_msg.as_deref(), None).ok();

            let row   = db.scheduler_get(workflow_id).ok().flatten();
            let count = row.as_ref().map(|r| r.run_count).unwrap_or(0);
            let name  = row.map(|r| r.workflow_name).unwrap_or_else(|| workflow_id.to_string());
            let result_value = serde_json::to_value(&r).ok();

            event_sink.emit("scheduler-status", serde_json::to_value(SchedulerStatusEvent {
                workflow_id:   workflow_id.to_string(),
                workflow_name: name,
                status:        if success { "waiting" } else { "error" }.to_string(),
                run_count:     count,
                last_run_at:   Some(now_str),
                next_run_at:   None,
                last_error:    if success { None } else { err_msg },
                last_result:   result_value,
            }).unwrap_or_default());
        }
        Err(e) => {
            db.scheduler_update_run(workflow_id, false, Some(&e.to_string()), None).ok();
            emit_error(event_sink, db, workflow_id, &e.to_string());
        }
    }
}

fn emit_waiting(
    event_sink:  &Arc<dyn EventSink>,
    db:          &Arc<dyn SchedulerDb>,
    workflow_id: &str,
    next_run_at: Option<DateTime<Utc>>,
) {
    let row   = db.scheduler_get(workflow_id).ok().flatten();
    let count = row.as_ref().map(|r| r.run_count).unwrap_or(0);
    let name  = row.as_ref().map(|r| r.workflow_name.clone())
                   .unwrap_or_else(|| workflow_id.to_string());
    let last  = row.and_then(|r| r.last_run_at);

    event_sink.emit("scheduler-status", serde_json::to_value(SchedulerStatusEvent {
        workflow_id:   workflow_id.to_string(),
        workflow_name: name,
        status:        "waiting".to_string(),
        run_count:     count,
        last_run_at:   last,
        next_run_at:   next_run_at.map(|t| t.to_rfc3339()),
        last_error:    None,
        last_result:   None,
    }).unwrap_or_default());
}

fn emit_error(
    event_sink:  &Arc<dyn EventSink>,
    db:          &Arc<dyn SchedulerDb>,
    workflow_id: &str,
    message:     &str,
) {
    let row   = db.scheduler_get(workflow_id).ok().flatten();
    let count = row.as_ref().map(|r| r.run_count).unwrap_or(0);
    let name  = row.map(|r| r.workflow_name).unwrap_or_else(|| workflow_id.to_string());

    event_sink.emit("scheduler-status", serde_json::to_value(SchedulerStatusEvent {
        workflow_id:   workflow_id.to_string(),
        workflow_name: name,
        status:        "error".to_string(),
        run_count:     count,
        last_run_at:   None,
        next_run_at:   None,
        last_error:    Some(message.to_string()),
        last_result:   None,
    }).unwrap_or_default());
}

fn emit_done(
    event_sink:  &Arc<dyn EventSink>,
    db:          &Arc<dyn SchedulerDb>,
    workflow_id: &str,
) {
    let row   = db.scheduler_get(workflow_id).ok().flatten();
    let count = row.as_ref().map(|r| r.run_count).unwrap_or(0);
    let name  = row.as_ref().map(|r| r.workflow_name.clone())
                   .unwrap_or_else(|| workflow_id.to_string());
    let last  = row.and_then(|r| r.last_run_at);

    event_sink.emit("scheduler-status", serde_json::to_value(SchedulerStatusEvent {
        workflow_id:   workflow_id.to_string(),
        workflow_name: name,
        status:        "done".to_string(),
        run_count:     count,
        last_run_at:   last,
        next_run_at:   None,
        last_error:    None,
        last_result:   None,
    }).unwrap_or_default());
}

fn update_next_run(db: &Arc<dyn SchedulerDb>, workflow_id: &str, next: &DateTime<Utc>) {
    db.scheduler_update_next_run_at(workflow_id, &next.to_rfc3339()).ok();
}

fn log_skip(event_sink: &Arc<dyn EventSink>, workflow_id: &str, reason: &str) {
    event_sink.emit("scheduler-skip", serde_json::json!({
        "workflow_id": workflow_id,
        "reason": reason
    }));
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
            Ok(TriggerKind::Webhook { port, path, method, secret })
        }
        "manual_trigger" => Ok(TriggerKind::Manual),
        other => Err(format!(
            "Node type '{}' is not a recognised trigger. \
             Add a Schedule, Webhook, or Manual Trigger node as the first node.",
            other
        )),
    }
}

async fn parse_http_request(
    stream:          &mut tokio::net::TcpStream,
    expected_path:   &str,
    expected_method: &str,
    secret:          &str,
) -> Result<std::collections::HashMap<String, Value>, String> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

    let (r, _w) = stream.split();
    let mut reader = BufReader::new(r);

    let mut req_line = String::new();
    reader.read_line(&mut req_line).await
        .map_err(|_| "HTTP/1.1 400 Bad Request\r\n\r\n".to_string())?;

    let parts: Vec<&str> = req_line.split_whitespace().collect();
    let req_method = parts.first().copied().unwrap_or("GET");
    let req_path   = parts.get(1).copied().unwrap_or("/");

    if expected_method != "ANY" && req_method != expected_method {
        return Err("HTTP/1.1 405 Method Not Allowed\r\n\r\n".to_string());
    }
    if req_path != expected_path {
        return Err("HTTP/1.1 404 Not Found\r\n\r\n".to_string());
    }

    let mut headers = serde_json::Map::new();
    let mut content_length: Option<usize> = None;
    let mut header_count = 0usize;
    loop {
        if header_count >= 100 { break; }
        let mut line = String::new();
        let _ = reader.read_line(&mut line).await;
        if line.len() > 8192 { break; }
        if line.trim().is_empty() { break; }
        if let Some((k, v)) = line.trim().split_once(": ") {
            let k_lower = k.to_lowercase();
            if k_lower == "content-length" {
                content_length = v.trim().parse().ok();
            }
            headers.insert(k_lower, Value::String(v.trim().to_string()));
        }
        header_count += 1;
    }

    if !secret.is_empty() {
        let provided = headers.get("x-webhook-secret")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let secrets_match = provided.as_bytes().ct_eq(secret.as_bytes()).unwrap_u8() == 1;
        if !secrets_match {
            return Err("HTTP/1.1 401 Unauthorized\r\n\r\n".to_string());
        }
    }

    // Reject requests with a body but no Content-Length rather than silently
    // discarding the body. Chunked transfer encoding omits Content-Length, so
    // callers that don't set it get a clear 411 instead of body: null.
    let body_methods = matches!(req_method, "POST" | "PUT" | "PATCH");
    let content_length = match content_length {
        Some(n) => n,
        None if body_methods => {
            return Err("HTTP/1.1 411 Length Required\r\nContent-Length: 0\r\n\r\n".to_string());
        }
        None => 0,
    };

    let mut body_bytes = vec![0u8; content_length.min(1_000_000)];
    if content_length > 0 {
        let _ = reader.read_exact(&mut body_bytes).await;
    }
    let body_str   = String::from_utf8_lossy(&body_bytes).to_string();
    let body_value: Value = serde_json::from_str(&body_str)
        .unwrap_or(Value::String(body_str));

    let mut vars = std::collections::HashMap::new();
    vars.insert("body".to_string(),    body_value);
    vars.insert("headers".to_string(), Value::Object(headers));
    vars.insert("method".to_string(),  Value::String(req_method.to_string()));
    vars.insert("path".to_string(),    Value::String(req_path.to_string()));
    Ok(vars)
}
