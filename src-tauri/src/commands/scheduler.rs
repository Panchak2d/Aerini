use std::sync::Arc;

use aerini_engine::scheduler::{ScheduledJobRow, SchedulerDaemon};

#[tauri::command]
pub async fn start_scheduled_workflow(
    workflow_id:   String,
    port_override: Option<u16>,
    always_on:     Option<bool>,
    daemon: tauri::State<'_, Arc<SchedulerDaemon>>,
) -> Result<(), String> {
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
    daemon.list_jobs()
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
