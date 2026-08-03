use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use aerini_engine::{
    db::{RunRecord, VersionRow, WorkflowDb, WorkflowSummary},
    error::EngineError,
    executor::{CredentialResolver, WorkflowExecutor, WorkflowResult},
    model::{ExecutionContext, NodeInput, Workflow},
    node::{NodeDescriptor, NodeRegistry},
    scheduler::SchedulerDaemon,
    EventSink,
};
use tokio_util::sync::CancellationToken;

#[tauri::command]
pub async fn save_run_record(
    record: RunRecord,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.save_run(&record))
        .await.map_err(|e| e.to_string())?
}

/// Persists the just-finished run's performance report, linking it to the
/// same `run_id` its `RunRecord` was saved under. Deliberately does not
/// accept a caller-supplied report (PLAN.md, Batch 2 §3) — it fetches the
/// frozen report itself via `perf_monitor::get_recent_report`, the same
/// non-destructive read the MEM chip/popover will use once repointed
/// (Batch 5), so the frontend never sees or plumbs a `PerformanceReport`.
/// Called immediately after `run_workflow` resolves — `executor::run()`
/// (VERIFIED by direct read of `executor/mod.rs`: every `run()` call is
/// wrapped in `perf_monitor::monitor_run`) finalizes the report into
/// `RECENT` before returning, and the same `workflow_id` exec-lock
/// `run_workflow` already holds guarantees no other run can overwrite it
/// in between — so a `None` here means no run of this `workflow_id` has
/// completed since process start (stale/duplicate call), not a real
/// failure. Treated as a no-op, not an error, for that reason.
#[tauri::command]
pub async fn save_performance_report(
    run_id:      String,
    workflow_id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || {
        match aerini_engine::perf_monitor::get_recent_report(&workflow_id) {
            Some(report) => db.save_performance_report(&run_id, &report),
            None => {
                tracing::warn!(
                    run_id = %run_id, workflow_id = %workflow_id,
                    "save_performance_report: no recent report found for this workflow_id — skipping"
                );
                Ok(())
            }
        }
    })
    .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn save_run_started(
    id: String,
    workflow_id: String,
    workflow_name: String,
    ran_at: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.save_run_started(&id, &workflow_id, &workflow_name, &ran_at))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_run_records(
    workflow_id: String,
    offset:      i64,
    limit:       i64,
    filter:      String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<Vec<RunRecord>, String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.list_runs(&workflow_id, offset, limit, &filter))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn delete_run_record(
    id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.delete_run(&id))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clear_run_records(
    workflow_id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.clear_runs(&workflow_id))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn save_workflow(
    workflow_json: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let workflow = Workflow::from_json(&workflow_json).map_err(|e| e.to_string())?;
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.save(&workflow))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn load_workflow(
    id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<Option<String>, String> {
    let db = Arc::clone(&db);
    let wf = tokio::task::spawn_blocking(move || db.load(&id))
        .await.map_err(|e| e.to_string())??;
    match wf {
        None     => Ok(None),
        Some(wf) => Ok(Some(wf.to_json_pretty().map_err(|e| e.to_string())?)),
    }
}

#[tauri::command]
pub async fn list_workflows(
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<Vec<WorkflowSummary>, String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.list())
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn delete_workflow(
    id:      String,
    db:      tauri::State<'_, Arc<WorkflowDb>>,
    daemon:  tauri::State<'_, Arc<SchedulerDaemon>>,
) -> Result<(), String> {
    // Stop any running job for this workflow first.
    // stop_job can fail on a genuine DB write error, not just when the
    // workflow isn't scheduled (that case returns Ok — see scheduler/mod.rs).
    // Deletion still proceeds either way; the failure is now logged instead
    // of silently discarded.
    if let Err(e) = daemon.stop_job(&id) {
        tracing::warn!(workflow_id = %id, error = %e, "delete_workflow: stop_job failed before delete");
    }

    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || {
        db.delete_runs_for_workflow(&id)?;
        db.delete_scheduled_job(&id)?;
        db.delete_variables_for_workflow(&id)?;
        // delete() also removes versions and variables via explicit DELETEs,
        // but variables and versions are cleaned here first so the explicit
        // deletes in delete() are no-ops rather than relying on FK cascade alone.
        db.delete(&id)
    })
    .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn save_version(
    workflow_id:   String,
    snapshot_json: String,
    message:       Option<String>,
    db:            tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || {
        db.save_version(&workflow_id, &snapshot_json, message.as_deref())
    })
    .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_versions(
    workflow_id: String,
    db:          tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<Vec<VersionRow>, String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.list_versions(&workflow_id))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_version(
    id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<Option<String>, String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.get_version(&id))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn delete_version(
    id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.delete_version(&id))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn run_workflow(
    workflow_json:      String,
    initial_variables:  HashMap<String, Value>,
    run_id:             Option<String>,
    registry:           tauri::State<'_, Arc<NodeRegistry>>,
    cred_resolver:      tauri::State<'_, Arc<dyn CredentialResolver>>,
    event_sink:         tauri::State<'_, Arc<dyn EventSink>>,
    active_run:         tauri::State<'_, Arc<crate::ActiveRunToken>>,
) -> Result<WorkflowResult, String> {
    // Callers that never need to cancel this run by id (e.g. a popover node
    // preview) may omit run_id — matches start_scheduled_workflow's existing
    // Option<T> IPC parameter convention in this same file's sibling command.
    let run_id = run_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let workflow = Workflow::from_json(&workflow_json).map_err(|e| e.to_string())?;

    // log the same dangerous-node signal at every execution entry
    // point. Desktop's design posture is intentional full host access (no
    // default block, no opt-in flag exists) — this does not change execution
    // behavior, it only closes the gap where a manual run left zero backend
    // signal, unlike the frontend confirm dialog or the server's
    // own startup warnings for the same node types.
    let dangerous = aerini_engine::nodes::dangerous_node_types_present(&workflow.nodes);
    if !dangerous.is_empty() {
        tracing::warn!(
            workflow_id = %workflow.id,
            node_types  = ?dangerous,
            "run_workflow: workflow contains dangerous node type(s) {:?} — desktop executes \
             these unconditionally by design; review the source of this workflow if it was \
             imported or shared.",
            dangerous
        );
    }

    // reject a second concurrent run of the identical saved
    // workflow rather than letting both execute fully concurrently and
    // double every side effect (HTTP calls, file writes, DB rows, emails).
    // Mirrors the server's own reject-if-busy shape for the same scenario
    // (aerini-server/src/api_server/routes/workflows.rs, 5s timeout) — matched
    // here rather than inventing a new figure. Single-node test runs use a
    // distinct `${workflow_id}_sub` id (stream-handler.ts::handleRunSingleNode),
    // so they get their own lock bucket and are never rejected by, or block,
    // a concurrent full run of the same open canvas; that overlap is instead
    // prevented client-side by the shared isRunning guard.
    let _exec_guard = active_run
        .acquire_exec_lock(&workflow.id, std::time::Duration::from_secs(5))
        .await
        .map_err(String::from)?;

    let token = CancellationToken::new();
    active_run.register(&run_id, token.clone());

    let executor = WorkflowExecutor::new(
        Arc::clone(&*registry),
        Arc::clone(&*cred_resolver),
    )
    .with_event_sink(Arc::clone(&*event_sink))
    .with_parallel_execution(workflow.parallel_execution)
    .with_max_concurrent_nodes(workflow.max_concurrent_nodes.unwrap_or(8))
    .with_cancel_token(token)
    // T4: desktop is single-tenant — the person running this
    // workflow is the same person who owns the machine and its data.
    // Unlocks node-level admin gates (e.g. `allow_raw_sql`) the same way
    // aerini-server does for a `write`+`admin`-scoped token
    // (api_server/routes/workflows.rs). Never set for server call sites.
    .with_caller_is_admin(true);

    let workflow_id = workflow.id.clone();
    let result = executor.run(Arc::new(workflow), initial_variables).await;

    // Remove only this run's own token — never a different, still-in-flight
    // run's. (Previously a single shared `Option<CancellationToken>` slot was
    // unconditionally cleared here, which could wipe out a concurrent run's
    // still-active cancellation handle — see ActiveRunToken's doc comment.)
    active_run.unregister(&run_id);
    // _exec_guard drops here (end of scope), releasing this workflow_id's lock.

    match result {
        Ok(r) => Ok(r),
        Err(EngineError::ExecutionCancelled) => Ok(WorkflowResult {
            execution_id:      uuid::Uuid::new_v4().to_string(),
            workflow_id,
            success:           false,
            node_outputs:      HashMap::new(),
            logs:              vec![],
            error:             Some("Run cancelled by user".to_string()),
            validation_errors: vec![],
        }),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
pub async fn get_node_types(
    registry: tauri::State<'_, Arc<NodeRegistry>>,
) -> Result<Vec<NodeDescriptor>, String> {
    let mut descriptors = registry.all_descriptors();
    descriptors.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Ok(descriptors)
}

#[tauri::command]
pub async fn get_setting(
    key: String,
    db:  tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<Option<String>, String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.get_setting(&key))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_setting(
    key:   String,
    value: String,
    db:    tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.set_setting(&key, &value))
        .await.map_err(|e| e.to_string())?
}

/// Clears all AI Memory rows for one chat session (Chat Panel "Clear" button).
///
/// Implementation note: the AI Memory table lives in `ai_memory.rs`'s own
/// SQLite pool (`{app_data_dir}/ai_memory.db`), opened lazily inside
/// `AiMemoryNode`. There is no separate store module for it — `store.rs` is
/// the *credential* store, a different database entirely. Rather than reopen
/// a second connection to `ai_memory.db` and duplicate the DELETE query already
/// implemented (and exercised) by `AiMemoryNode`'s `"clear"` operation, this
/// command looks the node up in the registry and executes it directly with a
/// synthetic, single-purpose `NodeInput`. `workflow_id`/`context` are unused by
/// `AiMemoryNode::execute` and left as harmless placeholders.
#[tauri::command]
pub async fn clear_chat_session(
    session_id: String,
    registry:   tauri::State<'_, Arc<NodeRegistry>>,
) -> Result<(), String> {
    let node = registry.get("ai_memory")
        .ok_or_else(|| "ai_memory node type is not registered".to_string())?;

    let input = NodeInput {
        node_id:      "chat_panel_clear_chat_session".to_string(),
        workflow_id:  String::new(),
        execution_id: uuid::Uuid::new_v4().to_string(),
        input:        serde_json::json!({ "operation": "clear", "session_id": session_id }),
        context:      ExecutionContext::default(),
    };

    let output = node.execute(input).await;
    if output.success {
        Ok(())
    } else {
        Err(output.error
            .map(|e| e.message)
            .unwrap_or_else(|| "Failed to clear chat session".to_string()))
    }
}
