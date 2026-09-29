use std::sync::Arc;

use aerini_engine::db::{ChatMessageRecord, ChatSessionMeta, ChatSessionRecord, WorkflowDb};

/// A workflow's chat sessions without their messages, newest-created first.
#[tauri::command]
pub async fn list_chat_session_meta(
    workflow_id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<Vec<ChatSessionMeta>, String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.list_chat_session_meta(&workflow_id))
        .await.map_err(|e| e.to_string())?
}

/// One session's messages in chronological order.
#[tauri::command]
pub async fn load_chat_messages(
    session_id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<Vec<ChatMessageRecord>, String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.load_chat_messages(&session_id))
        .await.map_err(|e| e.to_string())?
}

/// Adds one message, upserting its session row in the same transaction.
/// Repeating a call with the same message id does not add a second row.
#[tauri::command]
pub async fn append_chat_message(
    session: ChatSessionMeta,
    message: ChatMessageRecord,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.append_chat_message(&session, &message))
        .await.map_err(|e| e.to_string())?
}

/// Upserts a session's metadata without touching its messages.
#[tauri::command]
pub async fn save_chat_session_meta(
    session: ChatSessionMeta,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tokio::task::spawn_blocking(move || db.save_chat_session_meta(&session))
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
