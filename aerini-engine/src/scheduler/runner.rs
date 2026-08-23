use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde_json::Value;
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

use crate::executor::{CredentialResolver, WorkflowExecutor};
use crate::model::Workflow;
use crate::node::{NodeRegistry, Reloadable};
use crate::cron::next_cron_delay_secs;
use crate::plugin_loader::PluginLoader;
use crate::EventSink;

use super::{ScheduledJobRow, SchedulerDb, SchedulerStatusEvent, TriggerKind};

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

/// Aborts a single wrapped task if dropped before it finishes — the
/// single-handle counterpart to [`AbortOnDropTasks`] above, used by
/// [`fire_once_with_vars`]'s panic-isolation wrapper (see its own doc
/// comment) to preserve `stop_job`/`stop_all`'s documented "immediate hard
/// abort, even mid-run" contract (see `SchedulerDaemon::drain_all`'s doc
/// comment) across the extra task boundary that wrapper introduces.
///
/// Without this guard, aborting the *outer* job task while a run is
/// in-flight would only drop the `JoinHandle` this struct wraps — which,
/// per tokio's own docs, detaches the task rather than stopping it,
/// leaving the in-flight run to keep executing, untracked, in the
/// background. Aborting an already-finished task's handle is a documented
/// no-op, so this guard's normal (non-cancelled) drop at the end of
/// `fire_once_with_vars` is harmless.
struct AbortOnDropSingle(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDropSingle {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Extracts a human-readable message from a [`tokio::task::JoinError`]'s
/// panic payload. Panic payloads are almost always `&'static str`
/// (`panic!("literal")`) or `String` (`panic!("{}", x)` / `.expect("...")`
/// / `.unwrap()`-style messages) — this covers both; any other payload
/// type falls back to a generic message rather than failing.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_job_loop(
    workflow_id:      String,
    trigger:          TriggerKind,
    db:               Arc<dyn SchedulerDb>,
    registry:         Arc<Reloadable<NodeRegistry>>,
    cred_store:       Arc<dyn CredentialResolver>,
    event_sink:       Arc<dyn EventSink>,
    exec_lock:        Arc<tokio::sync::Mutex<()>>,
    fire_immediately: bool,
    env_allowlist:    Option<Arc<std::collections::HashSet<String>>>,
    shell_exec_disabled:  bool,
    code_exec_disabled:   bool,
    database_exec_disabled: bool,
    caller_is_admin:      bool,
    code_sandbox_enabled: bool,
    code_max_memory_mb:   Option<u64>,
    parallel_execution:   bool,
    max_concurrent_nodes: usize,
    server_max_duration_secs: Option<u64>,
    file_sandbox_dir: Option<Arc<std::path::PathBuf>>,
    plugin_dir:       Option<Arc<std::path::PathBuf>>,
    run_semaphore:    Arc<Semaphore>,
    shutting_down:    Arc<AtomicBool>,
    active_runs:      Arc<AtomicUsize>,
) {
    match trigger {
        TriggerKind::Interval { secs } => {
            if fire_immediately {
                if let Ok(_guard) = exec_lock.try_lock() {
                    let _permit = run_semaphore.acquire().await;
                    let snapshot = registry.current();
                    fire_once(&workflow_id, &db, &snapshot, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, database_exec_disabled, caller_is_admin, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir, &active_runs).await;
                } else {
                    log_skip(&event_sink, &workflow_id, "previous run still in progress");
                }
            }
            loop {
                if shutting_down.load(Ordering::SeqCst) { break; }
                let next = Utc::now()
                    .checked_add_signed(chrono::Duration::seconds(secs as i64))
                    .unwrap_or_else(Utc::now);
                update_next_run_async(&db, &workflow_id, &next).await;
                emit_waiting_async(&event_sink, &db, &workflow_id, Some(next)).await;

                tokio::time::sleep(std::time::Duration::from_secs(secs)).await;

                if shutting_down.load(Ordering::SeqCst) { break; }
                if let Ok(_guard) = exec_lock.try_lock() {
                    let _permit = run_semaphore.acquire().await;
                    let snapshot = registry.current();
                    fire_once(&workflow_id, &db, &snapshot, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, database_exec_disabled, caller_is_admin, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir, &active_runs).await;
                } else {
                    log_skip(&event_sink, &workflow_id, "previous run still in progress");
                }
            }
        }

        TriggerKind::Cron { ref expr } => {
            if fire_immediately {
                if let Ok(_guard) = exec_lock.try_lock() {
                    let _permit = run_semaphore.acquire().await;
                    let snapshot = registry.current();
                    fire_once(&workflow_id, &db, &snapshot, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, database_exec_disabled, caller_is_admin, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir, &active_runs).await;
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
                        emit_error_async(&event_sink, &db, &workflow_id, &e).await;
                        scheduler_set_status_async(&db, &workflow_id, "error").await;
                        break;
                    }
                };
                let next = now
                    .checked_add_signed(chrono::Duration::seconds(delay_secs as i64))
                    .unwrap_or(now);
                update_next_run_async(&db, &workflow_id, &next).await;
                emit_waiting_async(&event_sink, &db, &workflow_id, Some(next)).await;

                tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;

                if shutting_down.load(Ordering::SeqCst) { break; }
                if let Ok(_guard) = exec_lock.try_lock() {
                    let _permit = run_semaphore.acquire().await;
                    let snapshot = registry.current();
                    fire_once(&workflow_id, &db, &snapshot, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, database_exec_disabled, caller_is_admin, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir, &active_runs).await;
                } else {
                    log_skip(&event_sink, &workflow_id, "previous run still in progress");
                }
            }
        }

        TriggerKind::Once { run_at } => {
            let now   = Utc::now();
            let delay = run_at.signed_duration_since(now);
            if delay.num_milliseconds() > 0 {
                update_next_run_async(&db, &workflow_id, &run_at).await;
                emit_waiting_async(&event_sink, &db, &workflow_id, Some(run_at)).await;
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
                let snapshot = registry.current();
                fire_once(&workflow_id, &db, &snapshot, &cred_store, &event_sink, &env_allowlist, shell_exec_disabled, code_exec_disabled, database_exec_disabled, caller_is_admin, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes, server_max_duration_secs, &file_sandbox_dir, &active_runs).await;
            }
            scheduler_set_status_async(&db, &workflow_id, "done").await;
            emit_done_async(&event_sink, &db, &workflow_id).await;
        }

        TriggerKind::Webhook { port, path, method, secret, dedup_window_secs } => {
            let listener = match tokio::net::TcpListener::bind(
                format!("127.0.0.1:{}", port)
            ).await {
                Ok(l)  => l,
                Err(e) => {
                    let msg = format!("Failed to bind port {}: {}", port, e);
                    emit_error_async(&event_sink, &db, &workflow_id, &msg).await;
                    scheduler_set_status_async(&db, &workflow_id, "error").await;
                    return;
                }
            };

            emit_waiting_async(&event_sink, &db, &workflow_id, None).await;

            // Per-job dedup store: request body hash -> when it was last seen.
            // Scoped to this loop (not a global static) so one workflow's
            // dedup state can never collide with, or leak into, another's,
            // and it's naturally cleared when the job restarts.
            let dedup_seen: Arc<DashMap<[u8; 32], std::time::Instant>> = Arc::new(DashMap::new());

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
                    let _ = stream.write_all(
                        format!("HTTP/1.1 503 Service Unavailable\r\n{CORS_HEADER_LINES}\r\n").as_bytes()
                    ).await;
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
                // Resolved fresh per connection (not once per job-arm) so a
                // reload is visible to the very next inbound webhook call.
                let registry         = registry.current();
                let cred_store       = Arc::clone(&cred_store);
                let event_sink       = Arc::clone(&event_sink);
                let exec_lock        = Arc::clone(&exec_lock);
                let env_allowlist    = env_allowlist.clone();
                let file_sandbox_dir = file_sandbox_dir.clone();
                let run_semaphore    = Arc::clone(&run_semaphore);
                let active_runs      = Arc::clone(&active_runs);
                let dedup_seen       = Arc::clone(&dedup_seen);

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
                                format!("HTTP/1.1 408 Request Timeout\r\n{CORS_HEADER_LINES}\r\n").as_bytes()
                            ).await;
                            return;
                        }
                    };

                    {
                        use tokio::io::AsyncWriteExt;
                        let _ = stream.write_all(
                            format!("HTTP/1.1 200 OK\r\n{CORS_HEADER_LINES}Content-Length: 2\r\n\r\nOK").as_bytes()
                        ).await;
                    }

                    // Duplicate-delivery guard: providers that retry (Stripe, GitHub, ...)
                    // resend the same event body byte-for-byte, so a repeat within the
                    // window is treated as a re-delivery, not a new trigger, and the
                    // workflow is not re-run. The caller already got its 2xx above —
                    // per each provider's own retry contract, that's what stops the
                    // retries; this only prevents this delivery from running the
                    // workflow a second time. A body-less request (GET, or POST with
                    // no body) is exempt: every such request would hash identically,
                    // which would wrongly collapse distinct intentional triggers
                    // (e.g. a repeated manual ping) into one.
                    if dedup_window_secs > 0 {
                        let body_is_empty = matches!(
                            payload.get("body"),
                            Some(Value::String(s)) if s.is_empty()
                        );
                        if !body_is_empty {
                            let body_str = payload.get("body")
                                .map(|b| b.to_string())
                                .unwrap_or_default();
                            let key = *blake3::hash(body_str.as_bytes()).as_bytes();
                            let now = std::time::Instant::now();
                            let window = std::time::Duration::from_secs(dedup_window_secs);

                            // entry() holds the shard lock across the check and the
                            // write, so two near-simultaneous identical deliveries
                            // can't both slip past a separate get-then-insert race.
                            let is_duplicate = match dedup_seen.entry(key) {
                                dashmap::mapref::entry::Entry::Occupied(mut e) => {
                                    let seen_at = *e.get();
                                    let dup = now.duration_since(seen_at) < window;
                                    e.insert(now);
                                    dup
                                }
                                dashmap::mapref::entry::Entry::Vacant(e) => {
                                    e.insert(now);
                                    false
                                }
                            };

                            if is_duplicate {
                                log_skip(&event_sink, &workflow_id, "duplicate webhook delivery skipped (dedup window)");
                                return;
                            }
                            // Opportunistic prune on every new key so this map doesn't
                            // grow unbounded over a long-lived job.
                            dedup_seen.retain(|_, seen_at| now.duration_since(*seen_at) < window);
                        }
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
                        shell_exec_disabled, code_exec_disabled, database_exec_disabled, caller_is_admin, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes,
                        server_max_duration_secs, &file_sandbox_dir, &active_runs,
                    ).await;

                    emit_waiting_async(&event_sink, &db, &workflow_id, None).await;
                });
                conn_tasks.push(handle);
            }
        }

        TriggerKind::Plugin { type_id, config } => {
            let Some(plugin_dir) = plugin_dir.clone() else {
                let msg = format!(
                    "Plugin trigger \"{}\": no plugin directory is configured for this scheduler.",
                    type_id
                );
                emit_error_async(&event_sink, &db, &workflow_id, &msg).await;
                scheduler_set_status_async(&db, &workflow_id, "error").await;
                return;
            };

            emit_waiting_async(&event_sink, &db, &workflow_id, None).await;

            // Outer loop: (re)resolves and (re)starts a fresh plugin
            // instance every time the inner drain loop below falls through
            // -- whether that's the stream closing normally, a read error,
            // or the instance never producing a usable stream in the first
            // place. A short backoff between attempts avoids a busy-loop if
            // the plugin is persistently failing to resolve or start.
            loop {
                if shutting_down.load(Ordering::SeqCst) { break; }

                let loader = match PluginLoader::shared() {
                    Ok(l)  => l,
                    Err(e) => {
                        emit_error_async(&event_sink, &db, &workflow_id, &e).await;
                        scheduler_set_status_async(&db, &workflow_id, "error").await;
                        break;
                    }
                };

                let (handle, mut rx) = match loader.start_trigger(&plugin_dir, &type_id, &config).await {
                    Ok(v)  => v,
                    Err(e) => {
                        let msg = format!("Plugin trigger \"{}\": {}", type_id, e);
                        emit_error_async(&event_sink, &db, &workflow_id, &msg).await;
                        // Left running (not "error" status): a plugin can be
                        // (re)installed after this job is already armed, so
                        // a resolution failure retries rather than
                        // permanently parking the job — matching Cron's own
                        // "log and keep trying" handling of a bad
                        // expression, not Webhook's "port bind failed, stop"
                        // one.
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        continue;
                    }
                };
                // Ties the event-pump task's lifetime to this iteration: a
                // hard abort of this job's own task (stop_job/stop_all) or
                // simply falling out of the inner loop below both drop this
                // guard, which stops the pump task. Nothing here needs the
                // graceful `detach_all`-style exception `conn_tasks` has in
                // the Webhook arm above -- unlike a webhook connection, a
                // pump task is never itself an in-flight workflow run
                // (`fire_once_with_vars`, below, still gets that handling,
                // internally, regardless of trigger kind).
                let _task_guard = AbortOnDropSingle(handle);

                loop {
                    if shutting_down.load(Ordering::SeqCst) { break; }

                    match rx.recv().await {
                        Some(Ok(data)) => {
                            if shutting_down.load(Ordering::SeqCst) { break; }
                            if let Ok(_guard) = exec_lock.try_lock() {
                                let payload = plugin_event_to_vars(&data);
                                let _permit = run_semaphore.acquire().await;
                                // Resolved fresh per event (not once per job-arm) so a
                                // reload is visible to the very next plugin trigger event,
                                // matching the Webhook arm's own per-connection resolve.
                                let registry_snapshot = registry.current();
                                fire_once_with_vars(
                                    &workflow_id, &db, &registry_snapshot, &cred_store, &event_sink, payload, &env_allowlist,
                                    shell_exec_disabled, code_exec_disabled, database_exec_disabled, caller_is_admin, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes,
                                    server_max_duration_secs, &file_sandbox_dir, &active_runs,
                                ).await;
                            } else {
                                log_skip(&event_sink, &workflow_id, "previous run still in progress");
                            }
                        }
                        Some(Err(e)) => {
                            let msg = format!("Plugin trigger \"{}\": {}", type_id, e);
                            emit_error_async(&event_sink, &db, &workflow_id, &msg).await;
                            // The pump task always ends its own run right
                            // after sending an Err (see
                            // `PluginLoader::start_trigger`), so the next
                            // `recv()` would just be `None` regardless —
                            // break now instead of spending an extra
                            // iteration finding that out.
                            break;
                        }
                        // Guest closed the stream, or the pump task ended
                        // for any other reason (e.g. it traps) without
                        // sending an explicit Err.
                        None => break,
                    }
                }

                drop(_task_guard);
                if shutting_down.load(Ordering::SeqCst) { break; }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
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
    caller_is_admin:      bool,
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
        shell_exec_disabled, code_exec_disabled, database_exec_disabled, caller_is_admin, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes,
        server_max_duration_secs, file_sandbox_dir, active_runs,
    ).await;
}

/// Runs a single scheduled/webhook-triggered execution **isolated in its
/// own tokio task**, so a panic anywhere inside it — a bad node input, an
/// unexpected output shape, any of the engine's numerous `.unwrap()` call
/// sites reachable from a node's `execute()` (the `Node` trait's own
/// contract says "never panic", but nothing enforces that against
/// unexpected input) — cannot unwind past this function and kill whichever
/// task called it.
///
/// Every call site in this file (`Interval`/`Cron`/`Once`'s trigger loops
/// in `run_job_loop`, and `Webhook`'s per-connection task) calls this
/// function inline, `.await`-ed directly. If [`fire_once_with_vars_inner`]'s
/// body ran inline here instead of inside a spawned task, a panic inside it
/// would propagate straight through that `.await`: for `Interval`/`Cron`,
/// that would unwind the job's single long-lived scheduling task, silently
/// and permanently ending the schedule, and leave `jobs_map`'s cleanup
/// (`scheduler/mod.rs`'s post-`run_job_loop` `.remove(&wf_id)`) unreached —
/// so a subsequent `start_job` attempt would return `AlreadyRunning`
/// forever.
/// 
/// a *spawned* task's panic is isolated to that task alone — "the panic is
/// forwarded to the task's `JoinHandle` and all spawned tasks continue
/// running normally." Spawning the real work here and `.await`-ing its
/// `JoinHandle` (through [`AbortOnDropSingle`], which preserves
/// `stop_job`/`stop_all`'s hard-abort contract — see that struct's doc
/// comment) is what claims that isolation for this function's own caller.
///
/// On a panic, records the run as a failure through the exact same
/// `scheduler_update_run_async`/`emit_error_async` path a normal `Err`
/// from `executor.run()` already takes below — a panicked run is visible
/// to the operator exactly like any other failed run, not silently
/// dropped.
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
    caller_is_admin:      bool,
    code_sandbox_enabled: bool,
    code_max_memory_mb:   Option<u64>,
    parallel_execution:   bool,
    max_concurrent_nodes: usize,
    server_max_duration_secs: Option<u64>,
    file_sandbox_dir:     &Option<Arc<std::path::PathBuf>>,
    active_runs:          &Arc<AtomicUsize>,
) {
    // Owned clones for the spawned task ('static bound) — every value here
    // is a cheap Arc/Option<Arc>/Copy clone, matching the pattern the
    // Webhook per-connection spawn above already uses for the same reason.
    let workflow_id_owned     = workflow_id.to_string();
    let db_owned               = Arc::clone(db);
    let registry_owned         = Arc::clone(registry);
    let cred_store_owned       = Arc::clone(cred_store);
    let event_sink_owned       = Arc::clone(event_sink);
    let env_allowlist_owned    = env_allowlist.clone();
    let file_sandbox_dir_owned = file_sandbox_dir.clone();
    let active_runs_owned      = Arc::clone(active_runs);
    // Separate clones for the panic-reporting path below — the ones above
    // are moved into the spawned task and unavailable after that point.
    let db_for_report          = Arc::clone(db);
    let event_sink_for_report  = Arc::clone(event_sink);
    let workflow_id_for_report = workflow_id.to_string();

    let mut guard = AbortOnDropSingle(tokio::spawn(async move {
        fire_once_with_vars_inner(
            &workflow_id_owned, &db_owned, &registry_owned, &cred_store_owned, &event_sink_owned,
            vars, &env_allowlist_owned,
            shell_exec_disabled, code_exec_disabled, database_exec_disabled, caller_is_admin,
            code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes,
            server_max_duration_secs, &file_sandbox_dir_owned, &active_runs_owned,
        ).await;
    }));

    match (&mut guard.0).await {
        Ok(()) => {}
        Err(join_err) if join_err.is_panic() => {
            let msg = format!("Run panicked: {}", panic_message(join_err.into_panic()));
            tracing::error!(workflow_id = %workflow_id_for_report, "scheduler: {}", msg);
            scheduler_update_run_async(&db_for_report, &workflow_id_for_report, false, Some(msg.clone())).await;
            emit_error_async(&event_sink_for_report, &db_for_report, &workflow_id_for_report, &msg).await;
        }
        Err(join_err) => {
            // Not a panic — nothing in this codebase holds an AbortHandle to
            // this specific inner task (only the *outer* job task is ever
            // aborted, which cancels this whole function via `guard` above
            // rather than reaching this arm), so this should be unreachable
            // in practice. Logged defensively rather than silently dropped,
            // matching the panic arm's visibility.
            let msg = format!("Run did not complete: {}", join_err);
            tracing::error!(workflow_id = %workflow_id_for_report, "scheduler: {}", msg);
            scheduler_update_run_async(&db_for_report, &workflow_id_for_report, false, Some(msg.clone())).await;
            emit_error_async(&event_sink_for_report, &db_for_report, &workflow_id_for_report, &msg).await;
        }
    }
}

/// The actual run logic. Only ever called from inside
/// [`fire_once_with_vars`]'s spawned task, which is what isolates a panic
/// in here from the caller of `fire_once_with_vars`. Do
/// not call this directly — go through `fire_once`/`fire_once_with_vars`
/// so panics stay isolated.
#[allow(clippy::too_many_arguments)]
async fn fire_once_with_vars_inner(
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
    caller_is_admin:      bool,
    code_sandbox_enabled: bool,
    code_max_memory_mb:   Option<u64>,
    parallel_execution:   bool,
    max_concurrent_nodes: usize,
    server_max_duration_secs: Option<u64>,
    file_sandbox_dir:     &Option<Arc<std::path::PathBuf>>,
    active_runs:          &Arc<AtomicUsize>,
) {
    {
        let row   = scheduler_get_async(db, workflow_id).await;
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

    let json = match load_workflow_json_async(db, workflow_id).await {
        Ok(Some(j)) => j,
        Ok(None) => {
            emit_error_async(event_sink, db, workflow_id, "Workflow deleted from database").await;
            return;
        }
        Err(e) => {
            emit_error_async(event_sink, db, workflow_id, &e).await;
            return;
        }
    };

    let workflow = match Workflow::from_json(&json) {
        Ok(w)  => w,
        Err(e) => {
            emit_error_async(event_sink, db, workflow_id, &e.to_string()).await;
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
        // than silently.
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
    if caller_is_admin {
        executor = executor.with_caller_is_admin(true);
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
            scheduler_update_run_async(db, workflow_id, success, err_msg.clone()).await;

            let row   = scheduler_get_async(db, workflow_id).await;
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
            scheduler_update_run_async(db, workflow_id, false, Some(e.to_string())).await;
            emit_error_async(event_sink, db, workflow_id, &e.to_string()).await;
        }
    }
}

// ── Blocking-DB-call helpers ─────────────────────────────────────────
//
// `SchedulerDb` is a synchronous trait — `WorkflowDb` backs it with a
// blocking `r2d2`/`rusqlite` connection pool, with no async-aware
// equivalent. Calling any of its methods directly inside an `async fn` runs
// the pool checkout + query inline on whichever tokio worker thread is
// executing that task. `emit_waiting`/`emit_error`/`emit_done` and
// `update_next_run` below are shared between genuinely-synchronous callers
// (`scheduler/mod.rs`'s `start`/`replay_state`/`arm_job_internal` — see
// `SchedulerDaemon::start`'s own doc comment for why those stay sync) and
// this file's `run_job_loop`/`fire_once_with_vars`, which are `async fn`s
// invoked on every scheduler tick and every webhook fire — a much higher
// frequency than the sync, once-per-arm call sites.
//
// Rather than make the shared functions themselves `async` (which would
// force every sync caller in `scheduler/mod.rs` to become `async` too — a
// public-API change), each keeps its existing
// synchronous form, backed by a shared pure `*_event` builder, and gets a
// thin `*_async` sibling that offloads only the blocking DB read/write to
// `tokio::task::spawn_blocking` before building the same event payload.
// `run_job_loop`/`fire_once_with_vars` use the `*_async` siblings
// exclusively; `scheduler/mod.rs` is unaffected and untouched.

/// Fetches a `ScheduledJobRow` on the blocking thread pool instead of
/// inline on the async reactor. A `spawn_blocking` panic (or, in principle,
/// cancellation) degrades to `None`, matching every existing call site's
/// own `.ok().flatten()` — a DB error here was always treated as "no row."
async fn scheduler_get_async(
    db:          &Arc<dyn SchedulerDb>,
    workflow_id: &str,
) -> Option<ScheduledJobRow> {
    let db          = Arc::clone(db);
    let workflow_id = workflow_id.to_string();
    tokio::task::spawn_blocking(move || db.scheduler_get(&workflow_id).ok().flatten())
        .await
        .unwrap_or(None)
}

/// `db.scheduler_set_status`, offloaded. Errors are discarded, matching
/// every pre-existing call site's own `.ok()`.
async fn scheduler_set_status_async(db: &Arc<dyn SchedulerDb>, workflow_id: &str, status: &str) {
    let db          = Arc::clone(db);
    let workflow_id = workflow_id.to_string();
    let status      = status.to_string();
    let _ = tokio::task::spawn_blocking(move || db.scheduler_set_status(&workflow_id, &status)).await;
}

/// `db.scheduler_update_run`, offloaded. `next_run_at` is always `None` at
/// both call sites in this file, matching pre-existing behavior exactly.
async fn scheduler_update_run_async(
    db:          &Arc<dyn SchedulerDb>,
    workflow_id: &str,
    success:     bool,
    error_msg:   Option<String>,
) {
    let db          = Arc::clone(db);
    let workflow_id = workflow_id.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        db.scheduler_update_run(&workflow_id, success, error_msg.as_deref(), None)
    }).await;
}

/// `db.load_workflow_json`, offloaded. A `spawn_blocking` panic surfaces as
/// an `Err`, matching this fn's `Result`-returning contract (unlike the
/// other three helpers above, callers here already branch on `Err`).
async fn load_workflow_json_async(
    db:          &Arc<dyn SchedulerDb>,
    workflow_id: &str,
) -> Result<Option<String>, String> {
    let db          = Arc::clone(db);
    let workflow_id = workflow_id.to_string();
    tokio::task::spawn_blocking(move || db.load_workflow_json(&workflow_id))
        .await
        .unwrap_or_else(|e| Err(format!("scheduler DB task panicked: {e}")))
}

fn waiting_event(
    workflow_id: &str,
    row:         Option<ScheduledJobRow>,
    next_run_at: Option<DateTime<Utc>>,
) -> SchedulerStatusEvent {
    let count = row.as_ref().map(|r| r.run_count).unwrap_or(0);
    let name  = row.as_ref().map(|r| r.workflow_name.clone())
                   .unwrap_or_else(|| workflow_id.to_string());
    let last  = row.and_then(|r| r.last_run_at);
    SchedulerStatusEvent {
        workflow_id:   workflow_id.to_string(),
        workflow_name: name,
        status:        "waiting".to_string(),
        run_count:     count,
        last_run_at:   last,
        next_run_at:   next_run_at.map(|t| t.to_rfc3339()),
        last_error:    None,
        last_result:   None,
    }
}

pub(super) fn emit_waiting(
    event_sink:  &Arc<dyn EventSink>,
    db:          &Arc<dyn SchedulerDb>,
    workflow_id: &str,
    next_run_at: Option<DateTime<Utc>>,
) {
    let row = db.scheduler_get(workflow_id).ok().flatten();
    event_sink.emit("scheduler-status",
        serde_json::to_value(waiting_event(workflow_id, row, next_run_at)).unwrap_or_default());
}

/// Async sibling of [`emit_waiting`] — see the module note above.
pub(super) async fn emit_waiting_async(
    event_sink:  &Arc<dyn EventSink>,
    db:          &Arc<dyn SchedulerDb>,
    workflow_id: &str,
    next_run_at: Option<DateTime<Utc>>,
) {
    let row = scheduler_get_async(db, workflow_id).await;
    event_sink.emit("scheduler-status",
        serde_json::to_value(waiting_event(workflow_id, row, next_run_at)).unwrap_or_default());
}

fn error_event(workflow_id: &str, row: Option<ScheduledJobRow>, message: &str) -> SchedulerStatusEvent {
    let count = row.as_ref().map(|r| r.run_count).unwrap_or(0);
    let name  = row.map(|r| r.workflow_name).unwrap_or_else(|| workflow_id.to_string());
    SchedulerStatusEvent {
        workflow_id:   workflow_id.to_string(),
        workflow_name: name,
        status:        "error".to_string(),
        run_count:     count,
        last_run_at:   None,
        next_run_at:   None,
        last_error:    Some(message.to_string()),
        last_result:   None,
    }
}

pub(super) fn emit_error(
    event_sink:  &Arc<dyn EventSink>,
    db:          &Arc<dyn SchedulerDb>,
    workflow_id: &str,
    message:     &str,
) {
    let row = db.scheduler_get(workflow_id).ok().flatten();
    event_sink.emit("scheduler-status",
        serde_json::to_value(error_event(workflow_id, row, message)).unwrap_or_default());
}

/// Async sibling of [`emit_error`] — see the module note above.
pub(super) async fn emit_error_async(
    event_sink:  &Arc<dyn EventSink>,
    db:          &Arc<dyn SchedulerDb>,
    workflow_id: &str,
    message:     &str,
) {
    let row = scheduler_get_async(db, workflow_id).await;
    event_sink.emit("scheduler-status",
        serde_json::to_value(error_event(workflow_id, row, message)).unwrap_or_default());
}

fn done_event(workflow_id: &str, row: Option<ScheduledJobRow>) -> SchedulerStatusEvent {
    let count = row.as_ref().map(|r| r.run_count).unwrap_or(0);
    let name  = row.as_ref().map(|r| r.workflow_name.clone())
                   .unwrap_or_else(|| workflow_id.to_string());
    let last  = row.and_then(|r| r.last_run_at);
    SchedulerStatusEvent {
        workflow_id:   workflow_id.to_string(),
        workflow_name: name,
        status:        "done".to_string(),
        run_count:     count,
        last_run_at:   last,
        next_run_at:   None,
        last_error:    None,
        last_result:   None,
    }
}

// Note: no synchronous `emit_done` — unlike `emit_waiting`/`emit_error`,
// nothing in `scheduler/mod.rs` ever called it (grep-confirmed); its only
// pre-existing call site was this file's own (now-async) `run_job_loop`.
// Keeping an unused sync sibling around would be dead code.

/// Async — see the module note above. No sync sibling; see the note below.
async fn emit_done_async(
    event_sink:  &Arc<dyn EventSink>,
    db:          &Arc<dyn SchedulerDb>,
    workflow_id: &str,
) {
    let row = scheduler_get_async(db, workflow_id).await;
    event_sink.emit("scheduler-status",
        serde_json::to_value(done_event(workflow_id, row)).unwrap_or_default());
}

pub(super) fn update_next_run(db: &Arc<dyn SchedulerDb>, workflow_id: &str, next: &DateTime<Utc>) {
    db.scheduler_update_next_run_at(workflow_id, &next.to_rfc3339()).ok();
}

/// Async sibling of [`update_next_run`] — see the module note above.
pub(super) async fn update_next_run_async(db: &Arc<dyn SchedulerDb>, workflow_id: &str, next: &DateTime<Utc>) {
    let db          = Arc::clone(db);
    let workflow_id = workflow_id.to_string();
    let next_str    = next.to_rfc3339();
    let _ = tokio::task::spawn_blocking(move || db.scheduler_update_next_run_at(&workflow_id, &next_str)).await;
}

fn log_skip(event_sink: &Arc<dyn EventSink>, workflow_id: &str, reason: &str) {
    event_sink.emit("scheduler-skip", serde_json::json!({
        "workflow_id": workflow_id,
        "reason": reason
    }));
}

/// Converts a plugin trigger's `trigger-event.data` (an opaque JSON string,
/// per `wit/node.wit`) into the `vars` map `fire_once_with_vars` expects. A
/// JSON object's own top-level keys become the run's vars directly —
/// matching Webhook's own structured payload. Anything else (a bare string,
/// number, array, or invalid JSON) is wrapped under a single `"data"` key,
/// so the resulting shape is predictable regardless of what the plugin
/// sends.
fn plugin_event_to_vars(data: &str) -> std::collections::HashMap<String, Value> {
    match serde_json::from_str::<Value>(data) {
        Ok(Value::Object(map)) => map.into_iter().collect(),
        Ok(other) => std::iter::once(("data".to_string(), other)).collect(),
        Err(_) => std::iter::once(("data".to_string(), Value::String(data.to_string()))).collect(),
    }
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

/// Header lines shared by every webhook HTTP response — success, preflight,
/// and error alike — each already terminated with its own `\r\n`. See
/// `with_cors` in `nodes::webhook` for why the origin is a wildcard rather
/// than an echoed `Origin` header: this endpoint accepts arbitrary
/// third-party callers, not just the app's own UI.
///
/// Includes `Connection: close`: this handler reads exactly one request per
/// accepted `TcpStream` and drops the stream when its task returns. Without
/// this header an HTTP/1.1 client is entitled to assume the connection is
/// still open for reuse and pool it, and the next request sent over that
/// pooled connection then hits a socket the server already tore down.
const CORS_HEADER_LINES: &str = "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, PUT, OPTIONS\r\nAccess-Control-Allow-Headers: content-type, x-webhook-secret, x-webhook-timestamp\r\nConnection: close\r\n";

/// Full response for an OPTIONS preflight — see the `OPTIONS` check in
/// `parse_http_request`.
fn cors_preflight_response() -> String {
    format!("HTTP/1.1 204 No Content\r\n{CORS_HEADER_LINES}Content-Length: 0\r\n\r\n")
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
        _ => return Err(format!("HTTP/1.1 400 Bad Request\r\n{CORS_HEADER_LINES}\r\n")),
    };

    let parts: Vec<&str> = req_line.split_whitespace().collect();
    let req_method = parts.first().copied().unwrap_or("GET");
    let req_path   = parts.get(1).copied().unwrap_or("/");

    // A browser preflights any cross-origin request whose Content-Type isn't
    // form-encoded (the desktop Chat panel sends application/json) with an
    // OPTIONS request before it will send the real one. Answered here,
    // ahead of the method/path checks below, and via the same `Err` path
    // those checks use — the caller (run_job_loop's accept loop) writes it
    // straight back and keeps waiting, so a preflight never counts as the
    // request this listener is waiting for.
    if req_method == "OPTIONS" {
        return Err(cors_preflight_response());
    }

    if expected_method != "ANY" && req_method != expected_method {
        return Err(format!("HTTP/1.1 405 Method Not Allowed\r\n{CORS_HEADER_LINES}\r\n"));
    }
    if req_path != expected_path {
        return Err(format!("HTTP/1.1 404 Not Found\r\n{CORS_HEADER_LINES}\r\n"));
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
            return Err(format!("HTTP/1.1 401 Unauthorized\r\n{CORS_HEADER_LINES}\r\n"));
        }
    }

    // Reject requests with a body but no Content-Length rather than silently
    // discarding the body. Chunked transfer encoding omits Content-Length, so
    // callers that don't set it get a clear 411 instead of body: null.
    let body_methods = matches!(req_method, "POST" | "PUT" | "PATCH");
    let content_length = match content_length {
        Some(n) => n,
        None if body_methods => {
            return Err(format!("HTTP/1.1 411 Length Required\r\n{CORS_HEADER_LINES}Content-Length: 0\r\n\r\n"));
        }
        None => 0,
    };

    if content_length > MAX_BODY_BYTES {
        return Err(format!("HTTP/1.1 413 Payload Too Large\r\n{CORS_HEADER_LINES}Content-Length: 0\r\n\r\n"));
    }

    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body_bytes).await.is_err() {
        return Err(format!("HTTP/1.1 400 Bad Request\r\n{CORS_HEADER_LINES}\r\n"));
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

/// Integration test for the webhook trigger's full lifecycle: drives
/// `SchedulerDaemon::start_job` for real — the same call path used by both
/// `src-tauri/src/commands/scheduler.rs::start_scheduled_workflow` and
/// `aerini-server` — binds a real `TcpListener`, fires a real HTTP POST at
/// it, and asserts the workflow completes and the port is claimed cleanly.
#[cfg(test)]
mod integration_tests {
    use crate::db::WorkflowDb;
    use crate::executor::CredentialResolver;
    use crate::model::{NodeType, Workflow, WorkflowEdge, WorkflowNode};
    use crate::node::{NodeRegistry, Reloadable};
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
            // this reproduction test must not incidentally rely on.
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

    ///This test fires a real HTTP request at a really-bound listener and asserts the run completes.
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
            Arc::new(Reloadable::new(registry)),
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

        // the run must succeed, not abort on the Webhook node trying (and failing) to bind a second time.
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

    /// Same shape as `webhook_repro_workflow` but with `dedup_window_secs` set,
    /// and no `extract`/`output` nodes — the dedup test only needs to count
    /// how many times the workflow ran, not inspect its output.
    fn webhook_dedup_workflow(port: u16, dedup_window_secs: u64) -> Workflow {
        let mut wf = Workflow::new("wf-dedup-repro", "Dedup Repro");
        wf.nodes.push(WorkflowNode {
            id: "trigger".to_string(),
            node_type_id: "webhook".to_string(),
            node_type: NodeType::Action,
            name: "Webhook".to_string(),
            config: json!({
                "port": port,
                "path": "/hook",
                "method": "POST",
                "dedup_window_secs": dedup_window_secs
            }),
            credentials: Default::default(),
            input_schema: json!({}),
            output_schema: json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        });
        wf
    }

    /// Reproduces the #8 backlog scenario directly: a provider (Stripe,
    /// GitHub, ...) resends the same event body because it didn't see the
    /// ack in time. With `dedup_window_secs` set, the second identical
    /// delivery must not re-run the workflow — asserted by counting
    /// `scheduler-status` events carrying a `last_result`, which only fire
    /// once per actual run.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn duplicate_webhook_body_within_window_runs_workflow_once() {
        const PORT: u16 = 38476;
        let data_dir = temp_data_dir("dedup-repro");
        let db_path = data_dir.join("scheduler.db");
        cleanup_db(&db_path);

        let wf_db = WorkflowDb::open(&db_path, 4).expect("WorkflowDb::open failed");
        let workflow = webhook_dedup_workflow(PORT, 30);
        wf_db.save(&workflow).expect("save workflow failed");
        let db: Arc<dyn SchedulerDb> = Arc::new(wf_db);

        let mut registry = NodeRegistry::new();
        register_builtins(&mut registry, &data_dir, None);

        let sink = CapturingSink::default();
        let daemon = SchedulerDaemon::new(
            db,
            Arc::new(Reloadable::new(registry)),
            Arc::new(NoopCredentials),
            Arc::new(sink.clone()),
        );

        daemon.start_job(&workflow.id, Some(PORT), Some(false))
            .expect("start_job failed to arm the webhook listener");

        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/hook", PORT);
        let body = json!({ "id": "evt_same_delivery_retried" });

        // First delivery — retry the connect briefly since the listener
        // binds in a just-spawned background task.
        let mut first = None;
        for _ in 0..40 {
            match client.post(&url).json(&body).send().await {
                Ok(r) => { first = Some(r); break; }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }
        let first = first.expect("webhook listener never accepted the first connection");
        assert!(first.status().is_success());

        // Second delivery — identical body, simulating the provider's retry.
        let second = client.post(&url).json(&body).send().await
            .expect("second (retried) delivery failed to send");
        assert!(second.status().is_success(), "retried delivery must still get a 2xx ack");

        // Wait for exactly one run to be recorded — bounded polling, same
        // convention as scheduler_webhook_run_completes_against_real_listener,
        // rather than a fixed sleep that could flake under a slow CI runner.
        let mut run_count: usize;
        for _ in 0..60 {
            run_count = {
                let events = sink.0.lock().expect("CapturingSink mutex poisoned");
                events.iter()
                    .filter(|(name, p)| name.as_str() == "scheduler-status" && !p["last_result"].is_null())
                    .count()
            };
            if run_count >= 1 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        // A little extra time for a wrongly-not-deduped second run to also
        // land, so a regression shows up as run_count == 2, not a flaky pass.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        run_count = {
            let events = sink.0.lock().expect("CapturingSink mutex poisoned");
            events.iter()
                .filter(|(name, p)| name.as_str() == "scheduler-status" && !p["last_result"].is_null())
                .count()
        };
        assert_eq!(run_count, 1, "expected exactly one run for two identical deliveries within the dedup window");

        // The skip must be logged, not silent.
        let skip_logged = {
            let events = sink.0.lock().expect("CapturingSink mutex poisoned");
            events.iter().any(|(name, p)| {
                name.as_str() == "scheduler-skip" && p["reason"] == json!("duplicate webhook delivery skipped (dedup window)")
            })
        };
        assert!(skip_logged, "duplicate delivery must be logged via scheduler-skip");

        daemon.stop_job(&workflow.id).ok();
        cleanup_db(&db_path);
        let _ = std::fs::remove_dir_all(&data_dir);
    }

    /// A browser (including the desktop Chat panel's own webview) preflights
    /// a JSON POST with an OPTIONS request first. This asserts that
    /// preflight (a) gets CORS headers back so the browser will actually
    /// send the real request, and (b) does not itself get treated as the
    /// request the listener is waiting for — the workflow must still be
    /// waiting, untriggered, afterward, and the real POST that follows must
    /// still complete normally.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scheduler_webhook_options_preflight_gets_cors_and_does_not_trigger_run() {
        const PORT: u16 = 38466;
        let data_dir = temp_data_dir("cors-repro");
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
            Arc::new(Reloadable::new(registry)),
            Arc::new(NoopCredentials),
            Arc::new(sink.clone()),
        );

        daemon.start_job(&workflow.id, Some(PORT), Some(false))
            .expect("start_job failed to arm the webhook listener");

        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/hook", PORT);

        // Listener binding happens in a just-spawned background task — retry
        // briefly instead of guessing a fixed sleep duration (same pattern
        // as the POST test above).
        let mut preflight = None;
        for _ in 0..40 {
            match client.request(reqwest::Method::OPTIONS, &url).send().await {
                Ok(r) => { preflight = Some(r); break; }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }
        let preflight = preflight.expect("webhook listener never accepted the OPTIONS preflight");
        assert!(preflight.status().is_success(), "preflight response was not success: {}", preflight.status());
        let allow_origin = preflight.headers().get("access-control-allow-origin")
            .expect("preflight response missing access-control-allow-origin");
        assert_eq!(allow_origin, "*");

        // The preflight must not have consumed the wait: give a real run
        // a brief window to (wrongly) appear, and confirm it doesn't.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        {
            let events = sink.0.lock().expect("CapturingSink mutex poisoned");
            assert!(
                !events.iter().any(|(name, p)| name.as_str() == "scheduler-status" && !p["last_result"].is_null()),
                "OPTIONS preflight incorrectly triggered a workflow run"
            );
        }

        // The listener must still be waiting for the real request afterward.
        let body = json!({ "hello": "world" });
        let response = client.post(&url).json(&body).send().await
            .expect("real POST after preflight failed to reach the still-waiting listener");
        assert!(response.status().is_success(), "post-preflight POST was not success: {}", response.status());

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
        let event = last_result.expect("workflow run never produced a scheduler-status event after the real POST");
        assert_eq!(event["last_result"]["success"], true, "workflow run did not succeed: {:?}", event["last_result"]);

        daemon.stop_job(&workflow.id).ok();
        cleanup_db(&db_path);
        let _ = std::fs::remove_dir_all(&data_dir);
    }

    /// Builds a `Schedule (once, already-due) -> Database (sqlite)` workflow.
    /// The database node deliberately runs a `query` containing a
    /// single-quoted literal with `allow_raw_sql: true`, the exact
    /// precondition `sqlite.rs`'s `SQL_INJECTION_BLOCKED` gate exists to
    /// catch, and the exact thing `__caller_is_admin` must unlock before
    /// `allow_raw_sql` has any effect (see `sqlite.rs::execute_sqlite`).
    fn once_trigger_database_workflow(wf_id: &str, db_path: &std::path::Path) -> Workflow {
        let mut wf = Workflow::new(wf_id, "Caller-Is-Admin Repro");

        wf.nodes.push(WorkflowNode {
            id: "sched".to_string(),
            node_type_id: "schedule".to_string(),
            node_type: NodeType::Action,
            name: "Schedule".to_string(),
            // Already in the past -> run_job_loop's TriggerKind::Once arm
            // fires immediately, no sleep, no network needed for this test.
            config: json!({
                "mode": "once",
                "run_at": (chrono::Utc::now() - chrono::Duration::seconds(2)).to_rfc3339(),
            }),
            credentials: Default::default(),
            input_schema: json!({}),
            output_schema: json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        });
        wf.nodes.push(WorkflowNode {
            id: "db".to_string(),
            node_type_id: "database".to_string(),
            node_type: NodeType::Action,
            name: "Database".to_string(),
            config: json!({
                "db_type": "sqlite",
                "db_path": db_path.to_str().expect("temp db path must be valid UTF-8"),
                "operation": "query",
                // The single-quoted literal is what trips check_query_for_inline_values.
                "query": "SELECT 1 WHERE 'x' = 'x'",
                "allow_raw_sql": true,
            }),
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
            from_node: "sched".to_string(),
            from_port: "output".to_string(),
            to_node: "db".to_string(),
            to_port: "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: None,
        });

        wf
    }

    /// Polls `sink` for the scheduler-status event carrying the post-run
    /// `WorkflowResult` (mirrors the webhook test's own identical polling
    /// pattern above — same event, same 50ms/60-attempt budget).
    async fn wait_for_last_result(sink: &CapturingSink) -> Value {
        for _ in 0..60 {
            {
                let events = sink.0.lock().expect("CapturingSink mutex poisoned");
                if let Some((_, payload)) = events.iter().rev()
                    .find(|(name, p)| name.as_str() == "scheduler-status" && !p["last_result"].is_null())
                {
                    return payload["last_result"].clone();
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("workflow run never produced a scheduler-status event with last_result");
    }

    /// Normal case for a desktop-shaped scheduler (`with_caller_is_admin(true)`,
    /// mirroring `src-tauri/src/lib.rs`'s own
    /// call site) must actually unlock `allow_raw_sql` for a scheduled run,
    /// not just for a manual `run_workflow` invocation — this is the whole
    /// point of threading the flag through `run_job_loop`/`fire_once*`
    /// instead of only setting it on the desktop manual-run executor.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scheduled_run_with_caller_is_admin_unlocks_allow_raw_sql() {
        let data_dir = temp_data_dir("caller_admin_true");
        let db_path = data_dir.join("scheduler.db");
        cleanup_db(&db_path);
        let sqlite_target = data_dir.join("target.db");
        let _ = std::fs::remove_file(&sqlite_target);

        let wf_db = WorkflowDb::open(&db_path, 4).expect("WorkflowDb::open failed");
        let workflow = once_trigger_database_workflow("wf-caller-admin-true", &sqlite_target);
        wf_db.save(&workflow).expect("save workflow failed");
        let db: Arc<dyn SchedulerDb> = Arc::new(wf_db);

        let mut registry = NodeRegistry::new();
        register_builtins(&mut registry, &data_dir, None);

        let sink = CapturingSink::default();
        let daemon = SchedulerDaemon::new(
            db,
            Arc::new(Reloadable::new(registry)),
            Arc::new(NoopCredentials),
            Arc::new(sink.clone()),
        ).with_caller_is_admin(true);

        daemon.start_job(&workflow.id, None, Some(false))
            .expect("start_job failed to arm the once-trigger");

        let result = wait_for_last_result(&sink).await;
        assert_eq!(
            result["success"], true,
            "expected the query to succeed once caller_is_admin unlocks allow_raw_sql; got: {:?}",
            result
        );
    }

    /// Edge case / regression guard: the *default* scheduler config
    /// (`caller_is_admin` unset, matching every `aerini-server` call site)
    /// must still reject the identical workflow — this must not
    /// accidentally widen the gate for anyone who doesn't opt in.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scheduled_run_without_caller_is_admin_still_blocks_allow_raw_sql() {
        let data_dir = temp_data_dir("caller_admin_false");
        let db_path = data_dir.join("scheduler.db");
        cleanup_db(&db_path);
        let sqlite_target = data_dir.join("target.db");
        let _ = std::fs::remove_file(&sqlite_target);

        let wf_db = WorkflowDb::open(&db_path, 4).expect("WorkflowDb::open failed");
        let workflow = once_trigger_database_workflow("wf-caller-admin-false", &sqlite_target);
        wf_db.save(&workflow).expect("save workflow failed");
        let db: Arc<dyn SchedulerDb> = Arc::new(wf_db);

        let mut registry = NodeRegistry::new();
        register_builtins(&mut registry, &data_dir, None);

        let sink = CapturingSink::default();
        // Deliberately NOT calling .with_caller_is_admin(true) — default false,
        // same as every aerini-server SchedulerDaemon::new call site.
        let daemon = SchedulerDaemon::new(
            db,
            Arc::new(Reloadable::new(registry)),
            Arc::new(NoopCredentials),
            Arc::new(sink.clone()),
        );

        daemon.start_job(&workflow.id, None, Some(false))
            .expect("start_job failed to arm the once-trigger");

        let result = wait_for_last_result(&sink).await;
        assert_eq!(
            result["success"], false,
            "expected SQL_INJECTION_BLOCKED without caller_is_admin; got: {:?}",
            result
        );
        let err = result["error"].as_str().unwrap_or_default();
        assert!(
            err.contains("single-quoted"),
            "expected the SQL_INJECTION_BLOCKED message in top-level error; got: {:?}",
            err
        );
    }

    // ── Regression tests ─────────────────

    /// Deliberately panics on every `execute()` call — used only by the two
    /// panic-isolation regression tests below. Violates the `Node` trait's
    /// own "never panic" contract on purpose, standing in for one of the
    /// engine's many production `.unwrap()`/`.expect()` call sites failing
    /// against unexpected input.
    struct PanicNode;
    #[async_trait::async_trait]
    impl crate::node::Node for PanicNode {
        fn type_id(&self) -> &'static str { "test_panic_node" }
        fn display_name(&self) -> &'static str { "Test Panic Node" }
        fn node_type(&self) -> NodeType { NodeType::Action }
        fn version(&self) -> &'static str { "0.0.0" }
        fn input_schema(&self) -> Value { json!({}) }
        fn output_schema(&self) -> Value { json!({}) }
        async fn execute(&self, _input: crate::model::NodeInput) -> crate::model::NodeOutput {
            panic!("simulated node panic for panic-isolation test");
        }
    }

    /// `Schedule -> PanicNode`. `schedule_config` is supplied by the caller
    /// so both regression tests below (`Once` and `Interval` triggers) can
    /// share this one workflow shape.
    fn panic_node_workflow(wf_id: &str, schedule_config: Value) -> Workflow {
        let mut wf = Workflow::new(wf_id, "Panic Isolation Repro");

        wf.nodes.push(WorkflowNode {
            id: "sched".to_string(),
            node_type_id: "schedule".to_string(),
            node_type: NodeType::Action,
            name: "Schedule".to_string(),
            config: schedule_config,
            credentials: Default::default(),
            input_schema: json!({}),
            output_schema: json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        });
        wf.nodes.push(WorkflowNode {
            id: "boom".to_string(),
            node_type_id: "test_panic_node".to_string(),
            node_type: NodeType::Action,
            name: "Boom".to_string(),
            config: json!({}),
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
            from_node: "sched".to_string(),
            from_port: "output".to_string(),
            to_node: "boom".to_string(),
            to_port: "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: None,
        });

        wf
    }

    /// Case 1 (fast): a node that panics must still produce a visible
    /// `scheduler-status` "error" event — not be silently dropped. Uses a
    /// `Once` trigger (already-past `run_at`, fires immediately via
    /// `start_job`'s `fire_immediately = true`) so this resolves in well
    /// under a second.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panicking_node_reports_visible_error_not_silent_drop() {
        let data_dir = temp_data_dir("panic_visible_error");
        let db_path = data_dir.join("scheduler.db");
        cleanup_db(&db_path);

        let wf = panic_node_workflow("wf-panic-visible", json!({
            "mode": "once",
            "run_at": (chrono::Utc::now() - chrono::Duration::seconds(2)).to_rfc3339(),
        }));

        let wf_db = WorkflowDb::open(&db_path, 4).expect("WorkflowDb::open failed");
        wf_db.save(&wf).expect("save workflow failed");
        let db: Arc<dyn SchedulerDb> = Arc::new(wf_db);

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(crate::nodes::schedule::ScheduleNode));
        registry.register(Arc::new(PanicNode));

        let sink = CapturingSink::default();
        let daemon = SchedulerDaemon::new(
            db,
            Arc::new(Reloadable::new(registry)),
            Arc::new(NoopCredentials),
            Arc::new(sink.clone()),
        );

        daemon.start_job(&wf.id, None, Some(false))
            .expect("start_job failed to arm the once-trigger");

        let mut saw_error = false;
        for _ in 0..60 {
            {
                let events = sink.0.lock().expect("CapturingSink mutex poisoned");
                saw_error = events.iter().any(|(name, p)| {
                    name.as_str() == "scheduler-status"
                        && p["status"].as_str() == Some("error")
                        && p["last_error"].as_str().unwrap_or("").contains("panicked")
                });
            }
            if saw_error { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        daemon.stop_job(&wf.id).ok();
        cleanup_db(&db_path);
        let _ = std::fs::remove_dir_all(&data_dir);

        assert!(
            saw_error,
            "expected a scheduler-status 'error' event mentioning the panic within 3s"
        );
    }

    /// Case 2: an `Interval` job whose only node panics on
    /// every run must keep scheduling and firing subsequent runs — not die
    /// after the first panic and silently, permanently stop: the schedule
    /// must not stop with zero operator-visible signal, and the job must
    /// not get stuck `AlreadyRunning` forever with `jobs_map`'s cleanup
    /// line unreached.
    ///
    /// Budgeted for the enforced 10s minimum interval (`extract_trigger`,
    /// `scheduler/mod.rs`'s `secs.max(10)`) plus slack. This test takes
    /// >10s wall-clock by design — no mocked-time facility is used
    /// anywhere else in this file either (e.g. `wait_for_last_result`'s own
    /// real-time polling), so this matches existing convention rather than
    /// introducing a new one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panicking_node_does_not_kill_the_scheduling_loop() {
        let data_dir = temp_data_dir("panic_loop_survives");
        let db_path = data_dir.join("scheduler.db");
        cleanup_db(&db_path);

        // interval_secs: 1 is clamped to the enforced 10s minimum by
        // extract_trigger — set low here only to make the intent explicit.
        let wf = panic_node_workflow("wf-panic-loop-survives", json!({
            "mode": "interval",
            "interval_secs": 1,
        }));

        let wf_db = WorkflowDb::open(&db_path, 4).expect("WorkflowDb::open failed");
        wf_db.save(&wf).expect("save workflow failed");
        let db: Arc<dyn SchedulerDb> = Arc::new(wf_db);

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(crate::nodes::schedule::ScheduleNode));
        registry.register(Arc::new(PanicNode));

        let sink = CapturingSink::default();
        let daemon = SchedulerDaemon::new(
            db,
            Arc::new(Reloadable::new(registry)),
            Arc::new(NoopCredentials),
            Arc::new(sink.clone()),
        );

        daemon.start_job(&wf.id, None, Some(false))
            .expect("start_job failed to arm the interval-trigger");

        let mut running_count = 0usize;
        for _ in 0..300 { // 300 * 100ms = 30s budget
            {
                let events = sink.0.lock().expect("CapturingSink mutex poisoned");
                running_count = events.iter()
                    .filter(|(name, p)| {
                        name.as_str() == "scheduler-status" && p["status"].as_str() == Some("running")
                    })
                    .count();
            }
            if running_count >= 2 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // stop_job succeeding here alone doesn't prove the schedule survived
        // a panic: a panic that never reaches jobs_map's cleanup line would
        // still leave the job registered, so stop_job could succeed either
        // way. The running_count assertion below is what actually proves
        // the loop kept going, not this call.
        daemon.stop_job(&wf.id).ok();
        cleanup_db(&db_path);
        let _ = std::fs::remove_dir_all(&data_dir);

        assert!(
            running_count >= 2,
            "expected at least 2 'running' events (job survives the first panic \
             and fires again on schedule) within 30s; got {}",
            running_count
        );
    }
}
