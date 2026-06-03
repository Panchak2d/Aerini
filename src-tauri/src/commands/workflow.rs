use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use flowo_engine::{
    db::{RunRecord, VersionRow, WorkflowDb, WorkflowSummary},
    error::EngineError,
    executor::{CredentialResolver, WorkflowExecutor, WorkflowResult},
    model::Workflow,
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
    // stop_job is a no-op if the workflow is not scheduled.
    let _ = daemon.stop_job(&id);

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
    registry:           tauri::State<'_, Arc<NodeRegistry>>,
    cred_resolver:      tauri::State<'_, Arc<dyn CredentialResolver>>,
    event_sink:         tauri::State<'_, Arc<dyn EventSink>>,
    active_run:         tauri::State<'_, Arc<crate::ActiveRunToken>>,
) -> Result<WorkflowResult, String> {
    let workflow = Workflow::from_json(&workflow_json).map_err(|e| e.to_string())?;

    let token = CancellationToken::new();
    *active_run.0.lock().unwrap() = Some(token.clone());

    let executor = WorkflowExecutor::new(
        Arc::clone(&*registry),
        Arc::clone(&*cred_resolver),
    )
    .with_event_sink(Arc::clone(&*event_sink))
    .with_parallel_execution(workflow.parallel_execution)
    .with_max_concurrent_nodes(workflow.max_concurrent_nodes.unwrap_or(8))
    .with_cancel_token(token);

    let workflow_id = workflow.id.clone();
    let result = executor.run(Arc::new(workflow), initial_variables).await;

    // Clear stored token regardless of outcome.
    *active_run.0.lock().unwrap() = None;

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
