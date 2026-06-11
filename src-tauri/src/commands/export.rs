use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use argon2::{
    password_hash::{rand_core::{OsRng, RngCore}, PasswordHasher, SaltString},
    Argon2,
};
use base64::Engine as _;

use flowo_engine::{
    db::WorkflowDb,
    model::Workflow,
    scheduler::extract_trigger,
    scheduler::job::TriggerKind,
};
use serde::{Deserialize, Serialize};
use tauri::Manager;

mod templates;

/// Request from the frontend — workflow ID + user choices.
#[derive(Debug, Deserialize)]
pub struct ExportRequest {
    pub workflow_id:  String,
    pub status_port:  u16,
}

/// Response — path to the generated zip file.
#[derive(Debug, Serialize)]
pub struct ExportResult {
    pub zip_path:             String,
    pub workflow_name:        String,
    pub credentials:          Vec<CredentialExport>,
    pub trigger_desc:         String,
    /// The raw (unhashed) run secret generated for this export.
    /// Shown once in the UI and never stored to disk — the config file
    /// stores only the argon2id hash.
    pub run_secret_plaintext: String,
}

#[derive(Debug, Serialize)]
pub struct CredentialExport {
    pub credential_id: String,
    pub env_var_name:  String,
    pub node_name:     String,
}

/// Validate a workflow for export — returns error string if it can't be exported.
#[tauri::command]
pub async fn validate_workflow_for_export(
    workflow_id: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<ValidateResult, String> {
    let db_clone = Arc::clone(&db);
    let wf = tokio::task::spawn_blocking(move || db_clone.load(&workflow_id))
        .await.map_err(|e| e.to_string())??
        .ok_or("Workflow not found")?;

    let trigger = extract_trigger(&wf)
        .map_err(|e| e.to_string())?;

    if matches!(trigger, TriggerKind::Manual) {
        return Err(
            "Workflows with a Manual Trigger cannot be exported for server. \
             Add a Schedule or Webhook trigger node first.".to_string()
        );
    }

    let trigger_desc = trigger.describe();
    let credentials  = collect_credentials(&wf);

    Ok(ValidateResult { trigger_desc, credentials })
}

#[derive(Debug, Serialize)]
pub struct ValidateResult {
    pub trigger_desc: String,
    pub credentials:  Vec<CredentialExport>,
}

/// Generate the deployment zip and return its path so the frontend can
/// prompt the user to save it.
#[tauri::command]
pub async fn generate_server_package(
    request: ExportRequest,
    app:     tauri::AppHandle,
    db:      tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<ExportResult, String> {
    let db_clone = Arc::clone(&db);
    let wf_id    = request.workflow_id.clone();

    let wf = tokio::task::spawn_blocking(move || db_clone.load(&wf_id))
        .await.map_err(|e| e.to_string())??
        .ok_or("Workflow not found")?;

    let trigger = extract_trigger(&wf)
        .map_err(|e| e.to_string())?;

    if matches!(trigger, TriggerKind::Manual) {
        return Err("Cannot export a workflow with only a Manual trigger.".to_string());
    }

    let trigger_desc  = trigger.describe();
    let credentials   = collect_credentials(&wf);
    let workflow_json = wf.to_json_pretty().map_err(|e| e.to_string())?;

    let cred_env_vars: HashMap<String, String> = credentials.iter()
        .map(|c| (c.credential_id.clone(), c.env_var_name.clone()))
        .collect();

    // Generate a per-export run secret so POST /api/run is authenticated.
    // Uses 32 bytes from OsRng (256 bits of entropy), matching the entropy
    // source used for API tokens and credential keys throughout the codebase.
    // Stored as an argon2id hash (brute-force resistant); the raw secret is
    // returned to the UI for one-time display and never written to disk.
    // spawn_blocking: argon2 is CPU-intensive and must not block the async runtime.
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    let run_secret_raw   = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let secret_for_hash  = run_secret_raw.clone();
    let run_secret_hash  = tokio::task::spawn_blocking(move || hash_run_secret(&secret_for_hash))
        .await
        .map_err(|e| format!("Internal error hashing run secret: {}", e))?
        .map_err(|e| format!("Failed to hash run secret: {}", e))?;

    let server_config = serde_json::json!({
        "workflow_name":     wf.name,
        "workflow_json":     workflow_json,
        "status_port":       request.status_port,
        "exported_at":       chrono::Utc::now().to_rfc3339(),
        "credential_env_vars": cred_env_vars,
        "run_secret":        run_secret_hash,
    });
    let server_config_str = serde_json::to_string_pretty(&server_config)
        .map_err(|e| e.to_string())?;

    let env_example = templates::build_env_example(&credentials, &trigger);

    let install_sh = templates::build_install_sh(&wf.name, request.status_port);

    let service_file = templates::build_service_file(&wf.name, request.status_port);

    let readme = templates::build_readme(&wf.name, &trigger_desc, request.status_port, &credentials);

    let temp_dir  = std::env::temp_dir();
    let safe_name = wf.name.replace(|c: char| !c.is_alphanumeric() && c != '-', "_");
    let zip_path  = temp_dir.join(format!("flowo-server-{}.zip", safe_name));

    let zip_file = std::fs::File::create(&zip_path)
        .map_err(|e| format!("Cannot create zip: {}", e))?;

    // Write all zip entries inside a closure so we can clean up the temp file
    // on any failure without duplicating the remove_file call at every ? site.
    let write_result: Result<(), String> = (|| {
        let mut zip = zip::ZipWriter::new(zip_file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let exec_options = options.unix_permissions(0o755);

        zip.start_file("flowo-server.json", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(server_config_str.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.start_file(".env.example", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(env_example.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.start_file("install.sh", exec_options)
            .map_err(|e| e.to_string())?;
        zip.write_all(install_sh.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.start_file("flowo.service", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(service_file.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.start_file("README.txt", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(readme.as_bytes())
            .map_err(|e| e.to_string())?;

        // Bundle the flowo-server binary if it's available as a Tauri resource.
        // In CI the binary is compiled for x86_64-unknown-linux-musl and placed
        // in the Tauri resources directory. In dev mode it won't be present and
        // we skip it — install.sh will warn the user.
        let resource_path = app.path()
            .resource_dir()
            .ok()
            .map(|p| p.join("flowo-server-linux-x64"));

        if let Some(bin_path) = resource_path {
            if bin_path.exists() {
                let binary = std::fs::read(&bin_path)
                    .map_err(|e| format!("Cannot read bundled binary: {}", e))?;
                zip.start_file("flowo-server", exec_options)
                    .map_err(|e| e.to_string())?;
                zip.write_all(&binary)
                    .map_err(|e| e.to_string())?;
            }
        }

        zip.finish().map_err(|e| e.to_string())?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&zip_path);
        return Err(e);
    }

    Ok(ExportResult {
        zip_path:             zip_path.display().to_string(),
        workflow_name:        wf.name,
        credentials,
        trigger_desc,
        run_secret_plaintext: run_secret_raw,
    })
}

fn collect_credentials(wf: &Workflow) -> Vec<CredentialExport> {
    let mut out = Vec::new();
    for node in &wf.nodes {
        for cred_id in node.credentials.values() {
            let env_var = credential_id_to_env_var(cred_id);
            if !out.iter().any(|c: &CredentialExport| &c.credential_id == cred_id) {
                out.push(CredentialExport {
                    credential_id: cred_id.clone(),
                    env_var_name:  env_var,
                    node_name:     node.name.clone(),
                });
            }
        }
    }
    out
}

fn credential_id_to_env_var(id: &str) -> String {
    format!("FLOWO_CRED_{}", id.to_uppercase().replace(['-', '.'], "_"))
}

/// Generate a Docker deployment package for single-workflow serve mode.
/// Produces a zip containing: Dockerfile, docker-compose.yml, .env.example,
/// flowo-server.json, and README.md.
#[tauri::command]
pub async fn generate_docker_package(
    request: ExportRequest,
    db:      tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<ExportResult, String> {
    let db_clone = Arc::clone(&db);
    let wf_id    = request.workflow_id.clone();

    let wf = tokio::task::spawn_blocking(move || db_clone.load(&wf_id))
        .await.map_err(|e| e.to_string())??
        .ok_or("Workflow not found")?;

    let trigger = extract_trigger(&wf)
        .map_err(|e| e.to_string())?;

    if matches!(trigger, TriggerKind::Manual) {
        return Err("Cannot export a workflow with only a Manual trigger.".to_string());
    }

    let trigger_desc  = trigger.describe();
    let credentials   = collect_credentials(&wf);
    let workflow_json = wf.to_json_pretty().map_err(|e| e.to_string())?;

    let cred_env_vars: HashMap<String, String> = credentials.iter()
        .map(|c| (c.credential_id.clone(), c.env_var_name.clone()))
        .collect();

    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    let run_secret_raw   = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let secret_for_hash  = run_secret_raw.clone();
    let run_secret_hash  = tokio::task::spawn_blocking(move || hash_run_secret(&secret_for_hash))
        .await
        .map_err(|e| format!("Internal error hashing run secret: {}", e))?
        .map_err(|e| format!("Failed to hash run secret: {}", e))?;

    let server_config = serde_json::json!({
        "workflow_name":       wf.name,
        "workflow_json":       workflow_json,
        "status_port":         request.status_port,
        "exported_at":         chrono::Utc::now().to_rfc3339(),
        "credential_env_vars": cred_env_vars,
        "run_secret":          run_secret_hash,
    });
    let server_config_str = serde_json::to_string_pretty(&server_config)
        .map_err(|e| e.to_string())?;

    let dockerfile      = templates::build_serve_dockerfile();
    let docker_compose  = templates::build_serve_docker_compose(&wf.name, request.status_port, &credentials);
    let env_example     = templates::build_docker_env_example(&credentials);
    let readme          = templates::build_docker_readme(&wf.name, &trigger_desc, request.status_port, &credentials);

    let temp_dir  = std::env::temp_dir();
    let safe_name = wf.name.replace(|c: char| !c.is_alphanumeric() && c != '-', "_");
    let zip_path  = temp_dir.join(format!("flowo-docker-{}.zip", safe_name));

    let zip_file = std::fs::File::create(&zip_path)
        .map_err(|e| format!("Cannot create zip: {}", e))?;

    let write_result: Result<(), String> = (|| {
        let mut zip = zip::ZipWriter::new(zip_file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        zip.start_file("flowo-server.json", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(server_config_str.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.start_file("Dockerfile", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(dockerfile.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.start_file("docker-compose.yml", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(docker_compose.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.start_file(".env.example", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(env_example.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.start_file("README.md", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(readme.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.finish().map_err(|e| e.to_string())?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&zip_path);
        return Err(e);
    }

    Ok(ExportResult {
        zip_path:             zip_path.display().to_string(),
        workflow_name:        wf.name,
        credentials,
        trigger_desc,
        run_secret_plaintext: run_secret_raw,
    })
}

/// Hash `raw_secret` with argon2id for storage in `flowo-server.json`.
///
/// argon2id is brute-force resistant (unlike BLAKE3, which is a fast hash).
/// The stored value is an argon2 PHC string (~97 chars) that embeds the salt
/// and parameters, making it self-contained for verification.
fn hash_run_secret(raw: &str) -> Result<String, argon2::password_hash::Error> {
    let salt   = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    Ok(argon2.hash_password(raw.as_bytes(), &salt)?.to_string())
}
