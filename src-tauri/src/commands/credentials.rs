use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aerini_engine::{
    db::WorkflowDb,
    store::{CredentialEntry, CredentialMetadata, CredentialStore, CreateCredentialRequest},
};

/// Scans every workflow once and returns, for each credential ID referenced
/// by at least one node, the distinct workflow names using it. Shared by
/// `list_credential_usage` (proactive display) and `delete_credential` (the
/// delete-time guard) so both read the same signal instead of drifting apart.
fn scan_credential_usage(db: &WorkflowDb) -> Result<HashMap<String, Vec<String>>, String> {
    let summaries = db.list()?;
    let mut usage: HashMap<String, Vec<String>> = HashMap::new();
    for summary in &summaries {
        if let Ok(Some(workflow)) = db.load(&summary.id) {
            let mut seen_cred_ids: HashSet<String> = HashSet::new();
            for node in &workflow.nodes {
                for cred_id in node.credentials.values() {
                    if seen_cred_ids.insert(cred_id.clone()) {
                        usage.entry(cred_id.clone()).or_default().push(workflow.name.clone());
                    }
                }
            }
        }
    }
    Ok(usage)
}

#[tauri::command]
pub async fn list_credential_usage(
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<HashMap<String, Vec<String>>, String> {
    let db_clone = Arc::clone(&db);
    tokio::task::spawn_blocking(move || scan_credential_usage(&db_clone))
        .await
        .map_err(|e| e.to_string())?
}

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
pub async fn get_credential_secret(
    id:    String,
    store: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<Option<String>, String> {
    store.retrieve(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_credential(
    req:   CreateCredentialRequest,
    store: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<(), String> {
    store.store(&req).map_err(|e| e.to_string())
}

/// Returns the raw AES-256 credential-encryption key, base64-encoded.
/// See `CredentialStore::export_key_base64` for the full explanation of
/// what this is and how to restore from it — this command is a thin
/// pass-through with no additional logic of its own.
#[tauri::command]
pub async fn export_encryption_key(
    store: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<String, String> {
    Ok(store.export_key_base64())
}

#[tauri::command]
pub async fn delete_credential(
    id:    String,
    store: tauri::State<'_, Arc<CredentialStore>>,
    db:    tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db_clone = Arc::clone(&db);
    let usage = tokio::task::spawn_blocking(move || scan_credential_usage(&db_clone))
        .await
        .map_err(|e| e.to_string())??;

    if let Some(referencing) = usage.get(&id) {
        return Err(format!(
            "Cannot delete: credential is used by {} workflow(s): {}",
            referencing.len(),
            referencing.join(", ")
        ));
    }

    store.delete(&id).map_err(|e| e.to_string())
}
