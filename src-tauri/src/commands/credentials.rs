use std::sync::Arc;

use aerini_engine::{
    db::WorkflowDb,
    store::{CredentialEntry, CredentialMetadata, CredentialStore, CreateCredentialRequest},
};

#[tauri::command]
pub async fn list_credentials(
    store: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<Vec<CredentialEntry>, String> {
    store.list().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_credential_metadata(
    id:    String,
    store: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<Option<CredentialMetadata>, String> {
    store.get_metadata(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_credential(
    req:   CreateCredentialRequest,
    store: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<(), String> {
    store.store(&req).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_credential(
    id:    String,
    store: tauri::State<'_, Arc<CredentialStore>>,
    db:    tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    // Scan all workflows for nodes that reference this credential ID.
    let db_clone = Arc::clone(&db);
    let id_clone = id.clone();
    let referencing = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
        let summaries = db_clone.list()?;
        let mut names = Vec::new();
        for summary in &summaries {
            if let Ok(Some(workflow)) = db_clone.load(&summary.id) {
                if workflow.nodes.iter().any(|node| {
                    node.credentials.values().any(|cid| cid == &id_clone)
                }) {
                    names.push(workflow.name.clone());
                }
            }
        }
        Ok(names)
    })
    .await
    .map_err(|e| e.to_string())??;

    if !referencing.is_empty() {
        return Err(format!(
            "Cannot delete: credential is used by {} workflow(s): {}",
            referencing.len(),
            referencing.join(", ")
        ));
    }

    store.delete(&id).map_err(|e| e.to_string())
}
