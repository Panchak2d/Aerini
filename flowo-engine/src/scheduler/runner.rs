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

                // Draining: reject new connections immediately so no new runs start.
                if shutting_down.load(Ordering::SeqCst) {
                    use tokio::io::AsyncWriteExt;
                    let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n").await;
                    break;
                }

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
                let _permit = run_semaphore.acquire().await;
                fire_once_with_vars(
                    &workflow_id, &db, &registry, &cred_store, &event_sink, payload, &env_allowlist,
                    shell_exec_disabled, code_exec_disabled, database_exec_disabled, code_sandbox_enabled, code_max_memory_mb, parallel_execution, max_concurrent_nodes,
                    server_max_duration_secs, &file_sandbox_dir, &active_runs,
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
