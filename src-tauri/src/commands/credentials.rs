use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aerini_engine::{
    db::WorkflowDb,
    error::EngineError,
    store::{CredentialEntry, CredentialMetadata, CredentialStore, CreateCredentialRequest},
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

const KEY_EXPORT_CANCELLED: &str = "CANCELLED";

struct UsageScan {
    usage:      HashMap<String, Vec<String>>,
    unreadable: Vec<String>,
}

/// Scans every workflow once. `usage` maps each credential ID referenced by at
/// least one node to the distinct workflow names using it; `unreadable` names
/// the workflows that could not be loaded, whose references are unknown.
/// Shared by `list_credential_usage` and `delete_credential`.
fn scan_credential_usage(db: &WorkflowDb) -> Result<UsageScan, String> {
    let summaries = db.list()?;
    let mut usage: HashMap<String, Vec<String>> = HashMap::new();
    let mut unreadable = Vec::new();
    for summary in &summaries {
        match db.load(&summary.id) {
            Ok(Some(workflow)) => {
                let mut seen_cred_ids: HashSet<String> = HashSet::new();
                for node in &workflow.nodes {
                    for cred_id in node.credentials.values() {
                        if seen_cred_ids.insert(cred_id.clone()) {
                            usage.entry(cred_id.clone()).or_default().push(workflow.name.clone());
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(workflow = %summary.id, error = %e, "workflow unreadable during credential usage scan");
                unreadable.push(summary.name.clone());
            }
        }
    }
    Ok(UsageScan { usage, unreadable })
}

/// Runs a synchronous, mutex-guarded store call off the async runtime.
async fn blocking<T, F>(work: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, EngineError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_credential_usage(
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<HashMap<String, Vec<String>>, String> {
    let db_clone = Arc::clone(&db);
    let scan = tokio::task::spawn_blocking(move || scan_credential_usage(&db_clone))
        .await
        .map_err(|e| e.to_string())??;
    Ok(scan.usage)
}

#[tauri::command]
pub async fn list_credentials(
    store: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<Vec<CredentialEntry>, String> {
    let store = Arc::clone(&store);
    blocking(move || store.list()).await
}

#[tauri::command]
pub async fn get_credential_metadata(
    id:    String,
    store: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<Option<CredentialMetadata>, String> {
    let store = Arc::clone(&store);
    blocking(move || store.get_metadata(&id)).await
}

/// Creates a credential, or with `update` replaces an existing one. On update
/// an empty `value` keeps the stored secret.
#[tauri::command]
pub async fn save_credential(
    req:    CreateCredentialRequest,
    update: bool,
    store:  tauri::State<'_, Arc<CredentialStore>>,
) -> Result<(), String> {
    let store = Arc::clone(&store);
    blocking(move || if update { store.update(&req) } else { store.create(&req) }).await
}

/// Lists a provider's models using a saved credential's secret, which is
/// read here and never sent to the webview.
#[tauri::command]
pub async fn list_credential_models(
    id:       String,
    provider: String,
    base_url: String,
    store:    tauri::State<'_, Arc<CredentialStore>>,
) -> Result<Vec<String>, String> {
    let store = Arc::clone(&store);
    let secret = blocking(move || store.retrieve(&id))
        .await
        .map_err(|e| format!("CREDENTIAL_ERROR: {e}"))?
        .ok_or_else(|| "MISSING_API_KEY: credential not found".to_string())?;
    crate::commands::providers::models_for(&provider, &base_url, &secret).await
}

/// Returns the raw AES-256 credential-encryption key, base64-encoded, after a
/// native confirmation dialog the webview cannot answer.
/// See `CredentialStore::export_key_base64`.
#[tauri::command]
pub async fn export_encryption_key(
    app:   tauri::AppHandle,
    store: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<String, String> {
    let store = Arc::clone(&store);
    tokio::task::spawn_blocking(move || {
        let approved = app
            .dialog()
            .message(
                "This reveals the master key that encrypts every credential saved here. \
                 Anyone who gets it can decrypt all of them. Store it somewhere as secure \
                 as the credentials themselves, such as a password manager.",
            )
            .title("Reveal encryption key")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancel)
            .blocking_show();
        if approved {
            Ok(store.export_key_base64().to_string())
        } else {
            Err(KEY_EXPORT_CANCELLED.to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn delete_credential(
    id:    String,
    store: tauri::State<'_, Arc<CredentialStore>>,
    db:    tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let db_clone = Arc::clone(&db);
    let scan = tokio::task::spawn_blocking(move || scan_credential_usage(&db_clone))
        .await
        .map_err(|e| e.to_string())??;

    if let Some(referencing) = scan.usage.get(&id) {
        return Err(format!(
            "Cannot delete: credential is used by {} workflow(s): {}",
            referencing.len(),
            referencing.join(", ")
        ));
    }
    if !scan.unreadable.is_empty() {
        return Err(format!(
            "Cannot confirm the credential is unused: {} workflow(s) could not be read: {}",
            scan.unreadable.len(),
            scan.unreadable.join(", ")
        ));
    }

    let store = Arc::clone(&store);
    blocking(move || store.delete(&id)).await
}
