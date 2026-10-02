use std::sync::Arc;

use aerini_engine::scheduler::{ScheduledJobRow, SchedulerDaemon};

use aerini_engine::db::WorkflowDb;

#[tauri::command]
pub async fn start_scheduled_workflow(
    workflow_id:   String,
    port_override: Option<u16>,
    always_on:     Option<bool>,
    daemon: tauri::State<'_, Arc<SchedulerDaemon>>,
    db:     tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    // The plugin directory is a setting the user can change while the app
    // runs; pick up its current value for the job being started.
    let setting_db = Arc::clone(&db);
    let plugin_dir = tokio::task::spawn_blocking(move || setting_db.get_setting("plugin_dir"))
        .await
        .map_err(|e| e.to_string())??;
    daemon.set_plugin_dir(plugin_dir.map(std::path::PathBuf::from));
    daemon.start_job(&workflow_id, port_override, always_on)
        .map_err(|e| serde_json::to_string(&e).unwrap_or_else(|_| format!("{:?}", e)))
}

#[tauri::command]
pub async fn stop_scheduled_workflow(
    workflow_id: String,
    daemon: tauri::State<'_, Arc<SchedulerDaemon>>,
) -> Result<(), String> {
    daemon.stop_job(&workflow_id)
}

#[tauri::command]
pub async fn get_scheduled_jobs(
    daemon: tauri::State<'_, Arc<SchedulerDaemon>>,
) -> Result<Vec<ScheduledJobRow>, String> {
    // Never return a webhook trigger's plaintext secret over IPC to the
    // frontend — the scheduler's own listener still holds the real secret
    // internally for HMAC comparison.
    Ok(daemon.list_jobs()?.iter().map(|row| row.redacted()).collect())
}

#[tauri::command]
pub async fn get_scheduled_job(
    workflow_id: String,
    daemon: tauri::State<'_, Arc<SchedulerDaemon>>,
) -> Result<Option<ScheduledJobRow>, String> {
    // Same redaction as get_scheduled_jobs, above — this is still the IPC
    // boundary to the frontend, just scoped to one row instead of the list.
    Ok(daemon.get_job(&workflow_id)?.map(|row| row.redacted()))
}

#[tauri::command]
pub async fn set_always_on(
    workflow_id: String,
    always_on:   bool,
    daemon: tauri::State<'_, Arc<SchedulerDaemon>>,
) -> Result<(), String> {
    daemon.set_always_on(&workflow_id, always_on)
}

#[tauri::command]
pub async fn stop_all_jobs(
    daemon: tauri::State<'_, Arc<SchedulerDaemon>>,
) -> Result<(), String> {
    daemon.stop_all();
    Ok(())
}

/// Called by the frontend after its `scheduler-status` listener is registered.
/// Replays current scheduler state for all active jobs and emits `scheduler-ready`.
/// Replaces the old 800 ms startup delay.
#[tauri::command]
pub async fn request_scheduler_state(
    daemon: tauri::State<'_, Arc<SchedulerDaemon>>,
) -> Result<(), String> {
    daemon.replay_state();
    Ok(())
}
