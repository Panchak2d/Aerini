use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use serde_json::Value;
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

use crate::executor::{CredentialResolver, WorkflowExecutor};
use crate::model::Workflow;
use crate::node::NodeRegistry;
use crate::cron::next_cron_delay_secs;
use crate::EventSink;

use super::{SchedulerDb, SchedulerStatusEvent, TriggerKind};

/// Decrements `active_runs` on drop, covering success, early return, and panic.
struct ActiveRunGuard(Arc<AtomicUsize>);
impl Drop for ActiveRunGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Tracks spawned per-connection webhook tasks and aborts every still-tracked
/// one when dropped. Used so a hard abort of the owning job's own top-level
/// task (stop_job/stop_all, or drain_all's timeout fallback to stop_all())
/// cascades to any in-flight connection handler — a bare `tokio::spawn`
/// would leave those as independent sibling tasks that survive the abort.
///
/// On a *graceful* exit (the webhook loop's `shutting_down` check), call
/// `detach_all` first: drain_all's own contract is to let in-flight runs
/// finish, not cut them off, so that path must not trigger this Drop.
struct AbortOnDropTasks(Vec<tokio::task::JoinHandle<()>>);

impl AbortOnDropTasks {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn push(&mut self, handle: tokio::task::JoinHandle<()>) {
        self.0.push(handle);
    }

    /// Drops handles for tasks that have already finished, so this doesn't
    /// grow for the life of a long-running webhook job.
    fn reap(&mut self) {
        self.0.retain(|h| !h.is_finished());
    }

    /// Clears every tracked handle without aborting the tasks they refer to
    /// — dropping a `JoinHandle` (unlike aborting it) lets the task keep
    /// running independently. Call before a graceful exit so in-flight
    /// connections are allowed to finish rather than being cancelled.
    fn detach_all(&mut self) {
        self.0.clear();
    }
}

impl Drop for AbortOnDropTasks {
    fn drop(&mut self) {
        for handle in &self.0 {
            handle.abort();
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_job_loop(
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
    database_exec_disabled: bool,
    code_sandbox_enabled: bool,
    code_max_memory_mb:   Option<u64>,
    parallel_execution:   bool,
    max_concurrent_nodes: usize,
    server_max_duration_secs: Option<u64>,
    file_sandbox_dir: Option<Arc<std::path::PathBuf>>,
    run_semaphore:    Arc<Semaphore>,
    shutting_down:    Arc<AtomicBool>,
    active_runs:      Arc<AtomicUsize>,
) {
    match trigger {
        TriggerKind::Interval { secs } => {
            if fire_immediately {
                if let Ok(_guard) = exec_lock.try_lock() {
                    let _permit = run_semaphore.acquire().await;
                    fire_once(&workflow_id, &db, &registry, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, database_exec_disabled, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir, &active_runs).await;
                } else {
                    log_skip(&event_sink, &workflow_id, "previous run still in progress");
                }
            }
            loop {
                if shutting_down.load(Ordering::SeqCst) { break; }
                let next = Utc::now()
                    .checked_add_signed(chrono::Duration::seconds(secs as i64))
                    .unwrap_or_else(Utc::now);
                update_next_run(&db, &workflow_id, &next);
                emit_waiting(&event_sink, &db, &workflow_id, Some(next));

                tokio::time::sleep(std::time::Duration::from_secs(secs)).await;

                if shutting_down.load(Ordering::SeqCst) { break; }
                if let Ok(_guard) = exec_lock.try_lock() {
                    let _permit = run_semaphore.acquire().await;
                    fire_once(&workflow_id, &db, &registry, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, database_exec_disabled, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir, &active_runs).await;
                } else {
                    log_skip(&event_sink, &workflow_id, "previous run still in progress");
                }
            }
        }

        TriggerKind::Cron { ref expr } => {
            if fire_immediately {
                if let Ok(_guard) = exec_lock.try_lock() {
                    let _permit = run_semaphore.acquire().await;
                    fire_once(&workflow_id, &db, &registry, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, database_exec_disabled, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir, &active_runs).await;
                } else {
                    log_skip(&event_sink, &workflow_id, "previous run still in progress");
                }
            }
            loop {
                if shutting_down.load(Ordering::SeqCst) { break; }
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

                if shutting_down.load(Ordering::SeqCst) { break; }
                if let Ok(_guard) = exec_lock.try_lock() {
                    let _permit = run_semaphore.acquire().await;
                    fire_once(&workflow_id, &db, &registry, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, database_exec_disabled, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir, &active_runs).await;
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
            {
                let _permit = run_semaphore.acquire().await;
                fire_once(&workflow_id, &db, &registry, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, database_exec_disabled, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir, &active_runs).await;
            }
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

            // Tracks every per-connection task spawned below, aborting any
            // still-tracked task when dropped. Held locally, not in
            // SchedulerDaemon, so that a *hard* abort of this job's own
            // top-level task (stop_job/stop_all's handle.abort(), or
            // drain_all's timeout fallback to stop_all()) forcibly drops
            // this wrapper mid-poll and cascades the cancellation to every
            // in-flight connection handler — a bare tokio::spawn would
            // instead leave those as siblings, not children, of the job's
            // own handle, surviving the abort. On the *graceful*
            // shutting_down exit below, this is explicitly detached first:
            // drain_all's own contract is to let in-flight runs finish, not
            // cut them off, so that path must not trigger the same Drop.
            let mut conn_tasks = AbortOnDropTasks::new();

            loop {
                // Reap already-finished connections so conn_tasks doesn't
                // grow for the life of the job.
                conn_tasks.reap();

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

                // Draining: reject new connections immediately so no new runs
                // start, then detach in-flight connections before returning —
                // they keep running independently, matching drain_all's own
                // "waits for in-flight runs to finish" contract, and only a
                // subsequent hard abort (stop_all, e.g. on drain timeout) can
                // still cancel them via their own JoinHandle::abort() path
                // (unaffected by this wrapper once detached).
                if shutting_down.load(Ordering::SeqCst) {
                    use tokio::io::AsyncWriteExt;
                    let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n").await;
                    conn_tasks.detach_all();
                    break;
                }

                // Spawned per connection: a slow or stalled client (e.g. one that
                // never sends a trailing newline) must not block this loop from
                // accepting the next connection. Every captured value is cloned
                // here (cheap — Arc clones and short strings) so the accept loop
                // keeps its own copies for the next iteration.
                let workflow_id      = workflow_id.clone();
                let path             = path.clone();
                let method           = method.clone();
                let secret           = secret.clone();
                let db               = Arc::clone(&db);
                let registry         = Arc::clone(&registry);
                let cred_store       = Arc::clone(&cred_store);
                let event_sink       = Arc::clone(&event_sink);
                let exec_lock        = Arc::clone(&exec_lock);
                let env_allowlist    = env_allowlist.clone();
                let file_sandbox_dir = file_sandbox_dir.clone();
                let run_semaphore    = Arc::clone(&run_semaphore);
                let active_runs      = Arc::clone(&active_runs);

                let handle = tokio::spawn(async move {
                    let payload = match tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        parse_http_request(&mut stream, &path, &method, &secret),
                    ).await {
                        Ok(Ok(p))  => p,
                        Ok(Err(e)) => {
                            use tokio::io::AsyncWriteExt;
                            let _ = stream.write_all(e.as_bytes()).await;
                            return;
                        }
                        Err(_) => {
                            use tokio::io::AsyncWriteExt;
                            let _ = stream.write_all(
                                b"HTTP/1.1 408 Request Timeout\r\n\r\n"
                            ).await;
                            return;
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
                            return;
                        }
                        Ok(g) => g,
                    };
                    let _permit = run_semaphore.acquire().await;
                    fire_once_with_vars(
                        &workflow_id, &db, &registry, &cred_store, &event_sink, payload, &env_allowlist,
                        shell_exec_disabled, code_exec_disabled, database_exec_disabled, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes,
                        server_max_duration_secs, &file_sandbox_dir, &active_runs,
                    ).await;

                    emit_waiting(&event_sink, &db, &workflow_id, None);
                });
                conn_tasks.push(handle);
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

#[allow(clippy::too_many_arguments)]
async fn fire_once(
    workflow_id:          &str,
    db:                   &Arc<dyn SchedulerDb>,
    registry:             &Arc<NodeRegistry>,
    cred_store:           &Arc<dyn CredentialResolver>,
    event_sink:           &Arc<dyn EventSink>,
    env_allowlist:        &Option<Arc<std::collections::HashSet<String>>>,
    shell_exec_disabled:  bool,
    code_exec_disabled:   bool,
    database_exec_disabled: bool,
    code_sandbox_enabled: bool,
    code_max_memory_mb:   Option<u64>,
    parallel_execution:   bool,
    max_concurrent_nodes: usize,
    server_max_duration_secs: Option<u64>,
    file_sandbox_dir:     &Option<Arc<std::path::PathBuf>>,
    active_runs:          &Arc<AtomicUsize>,
) {
    fire_once_with_vars(workflow_id, db, registry, cred_store, event_sink,
        std::collections::HashMap::new(), env_allowlist,
        shell_exec_disabled, code_exec_disabled, database_exec_disabled, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes,
        server_max_duration_secs, file_sandbox_dir, active_runs,
    ).await;
}

#[allow(clippy::too_many_arguments)]
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
    database_exec_disabled: bool,
    code_sandbox_enabled: bool,
    code_max_memory_mb:   Option<u64>,
    parallel_execution:   bool,
    max_concurrent_nodes: usize,
    server_max_duration_secs: Option<u64>,
    file_sandbox_dir:     &Option<Arc<std::path::PathBuf>>,
    active_runs:          &Arc<AtomicUsize>,
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

    // This function is only ever called from the TriggerKind::Webhook arm above,
    // so the entry node (no incoming edge) is the Webhook node that's already
    // holding the listener for this run. Hand it the parsed request via this
    // reserved key instead of letting WebhookNode::execute() bind the port a
    // second time — that always fails (PORT_IN_USE / address-in-use).
    //
    // Matched by node_id (not port/path): port/path captured at job-start can
    // go stale against a live-edited workflow, and a user-supplied
    // `port_override` (start_job) means the bound port can legitimately differ
    // from the node's own stored config. node_id is read fresh from this exact
    // `workflow` — the same one about to be executed — so there's no staleness
    // window. A second, unrelated Webhook node elsewhere in the graph has a
    // different node_id and is unaffected; it still binds and waits normally.
    let mut vars = vars;
    {
        let has_incoming: std::collections::HashSet<&str> =
            workflow.edges.iter().map(|e| e.to_node.as_str()).collect();
        if let Some(entry) = workflow.nodes.iter().find(|n| !has_incoming.contains(n.id.as_str())) {
            let reserved_value = serde_json::json!({
                "node_id": entry.id,
                "payload": {
                    "body":    vars.get("body").cloned().unwrap_or(Value::Null),
                    "headers": vars.get("headers").cloned().unwrap_or(Value::Null),
                    "method":  vars.get("method").cloned().unwrap_or(Value::Null),
                    "path":    vars.get("path").cloned().unwrap_or(Value::Null),
                }
            });
            vars.insert(
                crate::nodes::webhook::WEBHOOK_TRIGGER_PAYLOAD_KEY.to_string(),
                reserved_value,
            );
        }
        // No entry node found: leave `vars` untouched. WebhookNode::execute()
        // will then find no matching reserved key and fall through to its
        // normal bind-and-wait path, which fails clearly (PORT_IN_USE) rather
        // than silently — same as today's behavior, not a regression.
    }

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
    if database_exec_disabled {
        executor = executor.with_database_disabled(true);
    }
    if code_sandbox_enabled {
        executor = executor.with_code_sandbox(true);
    }
    if code_max_memory_mb.is_some() {
        executor = executor.with_code_max_memory_mb(code_max_memory_mb);
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
    active_runs.fetch_add(1, Ordering::SeqCst);
    let _run_guard = ActiveRunGuard(Arc::clone(active_runs));
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

pub(super) fn emit_waiting(
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

pub(super) fn emit_error(
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

pub(super) fn update_next_run(db: &Arc<dyn SchedulerDb>, workflow_id: &str, next: &DateTime<Utc>) {
    db.scheduler_update_next_run_at(workflow_id, &next.to_rfc3339()).ok();
}

fn log_skip(event_sink: &Arc<dyn EventSink>, workflow_id: &str, reason: &str) {
    event_sink.emit("scheduler-skip", serde_json::json!({
        "workflow_id": workflow_id,
        "reason": reason
    }));
}

/// Reads a single line (through the trailing `\n`, if present) from `reader`,
/// enforcing `max_len` as a hard cap on bytes consumed *while reading* rather
/// than only checking the result afterward — unlike `AsyncBufReadExt::
/// read_line`, whose destination buffer can grow without bound before any
/// length is ever checked. Returns `Ok(None)` if `max_len` bytes were read
/// without finding a newline; the caller decides how to treat an oversized
/// line. `Ok(Some(line))` covers both a normal `\n`-terminated line and one
/// ended by EOF.
async fn read_line_capped(
    reader:  &mut (impl tokio::io::AsyncRead + Unpin),
    max_len: usize,
) -> std::io::Result<Option<String>> {
    use tokio::io::AsyncReadExt;

    let mut buf  = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if buf.len() >= max_len {
            return Ok(None);
        }
        if reader.read(&mut byte).await? == 0 {
            break; // EOF
        }
        buf.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

async fn parse_http_request(
    stream:          &mut tokio::net::TcpStream,
    expected_path:   &str,
    expected_method: &str,
    secret:          &str,
) -> Result<std::collections::HashMap<String, Value>, String> {
    use tokio::io::{AsyncReadExt, BufReader};

    // Hard cap on a single request/header line, enforced *during* the read
    // (see `read_line_capped`) rather than only checked after the fact.
    const MAX_LINE_BYTES: usize = 8192;
    // Hard cap on the request body. A declared Content-Length beyond this is
    // rejected outright (413) rather than truncated-and-read, so the socket
    // is never left with an unread remainder and the workflow is never fed
    // a partial body while still reporting success.
    const MAX_BODY_BYTES: usize = 1_000_000;

    let (r, _w) = stream.split();
    let mut reader = BufReader::new(r);

    let req_line = match read_line_capped(&mut reader, MAX_LINE_BYTES).await {
        Ok(Some(line)) => line,
        _ => return Err("HTTP/1.1 400 Bad Request\r\n\r\n".to_string()),
    };

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
        let line = match read_line_capped(&mut reader, MAX_LINE_BYTES).await {
            Ok(Some(line)) => line,
            // Oversized line, read error, or invalid UTF-8 — stop parsing
            // headers, same as reaching the blank line that ends them.
            _ => break,
        };
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

    if content_length > MAX_BODY_BYTES {
        return Err("HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\n\r\n".to_string());
    }

    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body_bytes).await.is_err() {
        return Err("HTTP/1.1 400 Bad Request\r\n\r\n".to_string());
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

/// Live reproduction test for the webhook trigger re-execution bug (Patch
/// R1) — `AERINI_RECOVERY_PLAN.md` Session 1 Step 2. This was previously
/// only design-confirmed by source read, never actually run. It drives
/// `SchedulerDaemon::start_job` for real — the same call path used by both
/// `src-tauri/src/commands/scheduler.rs::start_scheduled_workflow` and
/// `aerini-server` — binds a real `TcpListener`, fires a real HTTP POST at
/// it, and asserts the workflow actually completes instead of failing on
/// the pre-patch PORT_IN_USE bug.
#[cfg(test)]
mod integration_tests {
    use crate::db::WorkflowDb;
    use crate::executor::CredentialResolver;
    use crate::model::{NodeType, Workflow, WorkflowEdge, WorkflowNode};
    use crate::node::NodeRegistry;
    use crate::nodes::register_builtins;
    use crate::scheduler::{SchedulerDaemon, SchedulerDb};
    use crate::EventSink;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    struct NoopCredentials;
    #[async_trait::async_trait]
    impl CredentialResolver for NoopCredentials {
        async fn resolve(&self, _id: &str) -> Option<String> { None }
    }

    /// Captures every emitted `scheduler-status` payload so the test can
    /// find the one carrying the post-run `WorkflowResult` (`last_result`).
    #[derive(Clone, Default)]
    struct CapturingSink(Arc<Mutex<Vec<(String, Value)>>>);
    impl EventSink for CapturingSink {
        fn emit(&self, event: &str, payload: Value) {
            self.0.lock().expect("CapturingSink mutex poisoned").push((event.to_string(), payload));
        }
    }

    fn temp_data_dir(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aerini_recovery_repro_{}", tag));
        std::fs::create_dir_all(&p).expect("create temp data dir failed");
        p
    }

    fn cleanup_db(path: &std::path::PathBuf) {
        let _ = std::fs::remove_file(path);
        let mut wal = path.clone();
        wal.set_extension("db-wal");
        let _ = std::fs::remove_file(&wal);
        let mut shm = path.clone();
        shm.set_extension("db-shm");
        let _ = std::fs::remove_file(&shm);
    }

    /// Webhook (real bind, fixed test port) -> JSON extract (passthrough) ->
    /// Output. Mirrors the recovery plan's prescribed reproduction graph.
    fn webhook_repro_workflow(port: u16) -> Workflow {
        let mut wf = Workflow::new("wf-recovery-repro", "Recovery Repro");

        wf.nodes.push(WorkflowNode {
            id: "trigger".to_string(),
            node_type_id: "webhook".to_string(),
            node_type: NodeType::Action,
            name: "Webhook".to_string(),
            config: json!({ "port": port, "path": "/hook", "method": "POST" }),
            credentials: Default::default(),
            input_schema: json!({}),
            output_schema: json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        });
        wf.nodes.push(WorkflowNode {
            id: "extract".to_string(),
            node_type_id: "json".to_string(),
            node_type: NodeType::Utility,
            name: "Extract Body".to_string(),
            config: json!({ "operation": "extract", "pointer": "/trigger/body" }),
            credentials: Default::default(),
            input_schema: json!({}),
            output_schema: json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        });
        wf.nodes.push(WorkflowNode {
            id: "out".to_string(),
            node_type_id: "output".to_string(),
            node_type: NodeType::Utility,
            name: "Output".to_string(),
            // Pinned to "extract" explicitly so assertions don't depend on
            // OutputNode's HashMap-iteration-order fallback when no
            // source_node is set — a separate, out-of-scope concern
            // (Rule 6) this reproduction test must not incidentally rely on.
            config: json!({ "source_node": "extract" }),
            credentials: Default::default(),
            input_schema: json!({}),
            output_schema: json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        });

        wf.edges.push(WorkflowEdge {
            id: "e1".to_string(),
            from_node: "trigger".to_string(),
            from_port: "output".to_string(),
            to_node: "extract".to_string(),
            to_port: "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: None,
        });
        wf.edges.push(WorkflowEdge {
            id: "e2".to_string(),
            from_node: "extract".to_string(),
            from_port: "output".to_string(),
            to_node: "out".to_string(),
            to_port: "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: None,
        });

        wf
    }

    /// THE bug reproduction: before Patch R1, `WebhookNode::execute()`
    /// always tried to bind its own port even when invoked from the
    /// scheduler's `TriggerKind::Webhook` loop — which is already holding
    /// that exact port for the life of the job. The second bind always
    /// failed (address-in-use), so this workflow could never succeed when
    /// started via `SchedulerDaemon::start_job`. This test fires a real HTTP
    /// request at a really-bound listener and asserts the run completes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scheduler_webhook_run_completes_against_real_listener() {
        const PORT: u16 = 38456;
        let data_dir = temp_data_dir("repro");
        let db_path = data_dir.join("scheduler.db");
        cleanup_db(&db_path);

        let wf_db = WorkflowDb::open(&db_path, 4).expect("WorkflowDb::open failed");
        let workflow = webhook_repro_workflow(PORT);
        wf_db.save(&workflow).expect("save workflow failed");
        let db: Arc<dyn SchedulerDb> = Arc::new(wf_db);

        let mut registry = NodeRegistry::new();
        register_builtins(&mut registry, &data_dir, None);

        let sink = CapturingSink::default();
        let daemon = SchedulerDaemon::new(
            db,
            Arc::new(registry),
            Arc::new(NoopCredentials),
            Arc::new(sink.clone()),
        );

        daemon.start_job(&workflow.id, Some(PORT), Some(false))
            .expect("start_job failed to arm the webhook listener");

        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/hook", PORT);
        let body = json!({ "hello": "world" });

        // Listener binding happens in a just-spawned background task — retry
        // briefly instead of guessing a fixed sleep duration.
        let mut response = None;
        for _ in 0..40 {
            match client.post(&url).json(&body).send().await {
                Ok(r) => { response = Some(r); break; }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }
        let response = response.expect("webhook listener never accepted a connection (bind likely failed)");
        assert!(response.status().is_success(), "webhook HTTP response was not success: {}", response.status());

        // Wait for the run to finish and emit its WorkflowResult.
        let mut last_result: Option<Value> = None;
        for _ in 0..60 {
            {
                let events = sink.0.lock().expect("CapturingSink mutex poisoned");
                if let Some((_, payload)) = events.iter().rev()
                    .find(|(name, p)| name.as_str() == "scheduler-status" && !p["last_result"].is_null())
                {
                    last_result = Some(payload.clone());
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let event = last_result.expect("workflow run never produced a scheduler-status event with last_result");

        // THE assertion that fails pre-Patch-R1: the run must succeed, not
        // abort on the Webhook node trying (and failing) to bind a second time.
        assert_eq!(event["status"], "waiting", "expected post-run status 'waiting' (success); got: {:?}", event);
        let result = &event["last_result"];
        assert_eq!(result["success"], true, "workflow run did not succeed: {:?}", result);

        let trigger_out = &result["node_outputs"]["trigger"];
        assert_eq!(trigger_out["method"], "POST");
        assert_eq!(trigger_out["path"], "/hook");
        assert_eq!(trigger_out["body"], json!({ "hello": "world" }));
        assert!(trigger_out["headers"].is_object());

        let extract_out = &result["node_outputs"]["extract"];
        assert_eq!(extract_out["result"], json!({ "hello": "world" }));

        let out_out = &result["node_outputs"]["out"];
        assert_eq!(out_out["value"], json!({ "result": { "hello": "world" } }));

        daemon.stop_job(&workflow.id).ok();
        cleanup_db(&db_path);
        let _ = std::fs::remove_dir_all(&data_dir);
    }
}
