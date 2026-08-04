use std::sync::Arc;

use aerini_engine::db::{ChatSessionRecord, WorkflowDb};

/// All chat sessions (with messages) for a workflow, newest-created first.
#[tauri::command]
pub async fn list_chat_sessions(
    workflow_id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<Vec<ChatSessionRecord>, String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.list_chat_sessions(&workflow_id))
        .await.map_err(|e| e.to_string())?
}

/// Upserts one session's metadata and replaces its full message list.
#[tauri::command]
pub async fn save_chat_session(
    session: ChatSessionRecord,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.save_chat_session(&session))
        .await.map_err(|e| e.to_string())?
}

/// Deletes a session and its messages (FK cascade).
#[tauri::command]
pub async fn delete_chat_session(
    id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.delete_chat_session(&id))
        .await.map_err(|e| e.to_string())?
}
