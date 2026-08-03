//! Batch 3 (Performance Monitor Redesign — see `PLAN.md`): read-side Tauri
//! IPC surface over `perf_monitor`'s in-memory live state and the SQLite
//! `performance_reports` table Batch 2 persists. Additive only — none of
//! Batch 1/2's files change shape here; this only exposes what they already
//! built. Frontend wiring (MEM chip repoint, panel UI) is Batch 5/6's job,
//! not this one — nothing calls these commands yet.

use std::sync::Arc;

use aerini_engine::{
    db::{PerformanceReportRecord, WorkflowDb},
    perf_monitor::{self, PerformanceReport},
};

/// Point-in-time read of a single workflow currently in progress — `None`
/// if it isn't running right now. Mirrors `get_memory_breakdown`'s shape
/// exactly (sync, no Tauri state, no `Result`): both are cheap in-memory
/// reads over a registry the engine already owns process-wide, and neither
/// can fail. First real caller of `perf_monitor::get_live_snapshot`
/// (Batch 1 exposed it for this; nothing used it before now).
#[tauri::command]
pub fn get_live_performance(workflow_id: String) -> Option<PerformanceReport> {
    perf_monitor::get_live_snapshot(&workflow_id)
}

/// Frozen report for the most recently completed run of one workflow —
/// `None` if it has never run this session, or its recent slot was
/// cleared. Mirrors `get_live_performance` exactly (sync, no Tauri state,
/// no `Result`) — same cheap in-memory read, cannot fail. First real
/// caller of `perf_monitor::get_recent_report` (Batch 1 exposed it for
/// this; deferred from Batch 3 for lack of a caller — `PLAN.md`'s own
/// backlog named this Batch 5's job, alongside the MEM chip repoint that
/// actually calls it).
#[tauri::command]
pub fn get_recent_performance(workflow_id: String) -> Option<PerformanceReport> {
    perf_monitor::get_recent_report(&workflow_id)
}

/// Fetch one persisted report by its `run_history`-linked `run_id`.
#[tauri::command]
pub async fn get_performance_report(
    run_id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<Option<PerformanceReportRecord>, String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.get_performance_report(&run_id))
        .await.map_err(|e| e.to_string())?
}

/// Page through a workflow's persisted reports, newest first.
#[tauri::command]
pub async fn list_performance_reports(
    workflow_id: String,
    offset:      i64,
    limit:       i64,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<Vec<PerformanceReportRecord>, String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.list_performance_reports(&workflow_id, offset, limit))
        .await.map_err(|e| e.to_string())?
}

/// Delete one persisted report by `run_id`.
#[tauri::command]
pub async fn delete_performance_report(
    run_id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.delete_performance_report(&run_id))
        .await.map_err(|e| e.to_string())?
}

/// Delete every persisted report for a workflow.
#[tauri::command]
pub async fn clear_performance_reports(
    workflow_id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.clear_performance_reports(&workflow_id))
        .await.map_err(|e| e.to_string())?
}
