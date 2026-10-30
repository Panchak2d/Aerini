use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use argon2::{
    password_hash::{rand_core::{OsRng, RngCore}, PasswordHasher, SaltString},
    Argon2,
};
use base64::Engine as _;

use aerini_engine::{
    credential_env::assign_env_vars,
    db::WorkflowDb,
    model::Workflow,
    scheduler::extract_trigger,
    scheduler::job::TriggerKind,
};
use serde::{Deserialize, Serialize};
use tauri::Manager;

mod templates;

const EXPORT_PREFIX: &str = "aerini-export-";
const EXPORT_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

static ISSUED_EXPORTS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn issued_exports() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
    ISSUED_EXPORTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Creates an owner-only zip file with a random name in the temp folder and
/// records it, so `save_export_zip` accepts only files made here.
fn create_export_file() -> Result<(std::fs::File, PathBuf), String> {
    sweep_stale_exports();
    let (file, path) = tempfile::Builder::new()
        .prefix(EXPORT_PREFIX)
        .suffix(".zip")
        .tempfile()
        .map_err(|e| format!("Cannot create zip: {}", e))?
        .keep()
        .map_err(|e| format!("Cannot create zip: {}", e.error))?;
    issued_exports().push(path.canonicalize().unwrap_or_else(|_| path.clone()));
    Ok((file, path))
}

/// Deletes export zips that were never saved, such as after a crash.
fn sweep_stale_exports() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(EXPORT_PREFIX) || !name.ends_with(".zip") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > EXPORT_MAX_AGE);
        if old {
            let path = entry.path();
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            discard_export(&canonical);
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Resolves `path` to an export zip created by this module, or fails.
pub fn issued_export(path: &str) -> Result<PathBuf, String> {
    let canonical = Path::new(path)
        .canonicalize()
        .map_err(|_| format!("Export zip not found at: {}", path))?;
    if issued_exports().contains(&canonical) {
        Ok(canonical)
    } else {
        Err("zip_path is not an export created by this session".to_string())
    }
}

/// Deletes an issued export zip and forgets it.
pub fn discard_export(canonical: &Path) {
    let _ = std::fs::remove_file(canonical);
    issued_exports().retain(|p| p != canonical);
}

/// Deletes a zip returned by `create_export_file` that will not be handed out.
fn discard_unissued(path: &Path) {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    discard_export(&canonical);
}

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
             Add a Schedule, Webhook, or trigger-plugin node first.".to_string()
        );
    }

    let trigger_desc = trigger.describe();
    let credentials  = collect_credentials(&wf)?;
    let variables    = collect_vars(&wf)?;

    Ok(ValidateResult { trigger_desc, credentials, variables })
}

#[derive(Debug, Serialize)]
pub struct ValidateResult {
    pub trigger_desc: String,
    pub credentials:  Vec<CredentialExport>,
    pub variables:    Vec<String>,
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
    let credentials   = collect_credentials(&wf)?;
    let workflow_json = wf.to_json_pretty().map_err(|e| e.to_string())?;

    // same canonical list every other execution entry point
    // consults. The generated package does NOT auto-pass --allow-shell/
    // --allow-code/--allow-database for these (see build_service_file's own
    // comment for why) -- this is only used to render an explicit warning so
    // the operator isn't surprised when such a node silently no-ops.
    let dangerous = aerini_engine::nodes::dangerous_node_types_present(&wf.nodes);

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

    let install_sh = templates::build_install_sh(&wf.name, request.status_port, dangerous.contains(&"code"));

    let service_file = templates::build_service_file(&wf.name, request.status_port, &dangerous);

    let readme = templates::build_readme(&wf.name, &trigger_desc, request.status_port, &credentials, &dangerous);

    let (zip_file, zip_path) = create_export_file()?;

    // Write all zip entries inside a closure so we can clean up the temp file
    // on any failure without duplicating the remove_file call at every ? site.
    let write_result: Result<(), String> = (|| {
        let mut zip = zip::ZipWriter::new(zip_file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let exec_options = options.unix_permissions(0o755);

        zip.start_file("aerini-server.json", options)
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

        zip.start_file("aerini.service", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(service_file.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.start_file("README.txt", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(readme.as_bytes())
            .map_err(|e| e.to_string())?;

        // Add aerini-server when this build ships it as a Tauri resource.
        // Without it the zip is still written, and install.sh stops with
        // build-from-source instructions.
        let resource_path = app.path()
            .resource_dir()
            .ok()
            .map(|p| p.join("aerini-server-linux-x64"));

        if let Some(bin_path) = resource_path {
            if bin_path.exists() {
                let binary = std::fs::read(&bin_path)
                    .map_err(|e| format!("Cannot read bundled binary: {}", e))?;
                zip.start_file("aerini-server", exec_options)
                    .map_err(|e| e.to_string())?;
                zip.write_all(&binary)
                    .map_err(|e| e.to_string())?;
            }
        }

        zip.finish().map_err(|e| e.to_string())?;
        Ok(())
    })();

    if let Err(e) = write_result {
        discard_unissued(&zip_path);
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

fn collect_credentials(wf: &Workflow) -> Result<Vec<CredentialExport>, String> {
    let mut found: Vec<(&str, &str)> = Vec::new();
    for node in &wf.nodes {
        let mut fields: Vec<(&String, &String)> = node.credentials.iter().collect();
        fields.sort();
        for (_, cred_id) in fields {
            if !found.iter().any(|(id, _)| *id == cred_id.as_str()) {
                found.push((cred_id.as_str(), node.name.as_str()));
            }
        }
    }
    let names = assign_env_vars(found.iter().map(|(id, _)| *id)).map_err(|e| e.to_string())?;
    Ok(found
        .into_iter()
        .zip(names)
        .map(|((id, node_name), (_, env_var))| CredentialExport {
            credential_id: id.to_string(),
            env_var_name:  env_var,
            node_name:     node_name.to_string(),
        })
        .collect())
}

/// Scans the full serialized workflow JSON for `$vars.<name>` references and
/// returns the unique variable names, sorted. Covers nested config objects
/// and trigger-node config alike, since it operates on the whole document.
/// No `regex` dependency — manual scan for the literal prefix, since `regex`
/// is not in either Cargo.toml.
fn collect_vars(wf: &Workflow) -> Result<Vec<String>, String> {
    let json = wf.to_json_pretty().map_err(|e| e.to_string())?;
    const PREFIX: &str = "$vars.";

    let mut names = std::collections::BTreeSet::new();
    let mut search_from = 0;
    while let Some(rel) = json[search_from..].find(PREFIX) {
        let start = search_from + rel + PREFIX.len();
        let end = json[start..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map(|i| start + i)
            .unwrap_or(json.len());
        if end > start {
            names.insert(json[start..end].to_string());
        }
        search_from = end.max(start + 1).min(json.len());
    }
    Ok(names.into_iter().collect())
}

/// Generate a Docker deployment package for single-workflow serve mode.
/// Produces a zip containing: docker-compose.yml (runs the official image),
/// docker-compose.build.yml and Dockerfile.source (build from source), .env.example,
/// aerini-server.json, and README.md, plus a Caddyfile when the trigger is a Webhook.
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
    let credentials   = collect_credentials(&wf)?;
    let workflow_json = wf.to_json_pretty().map_err(|e| e.to_string())?;

    // same canonical list every other execution entry point
    // consults. The generated package does NOT auto-pass --allow-shell/
    // --allow-code/--allow-database for these (see build_service_file's own
    // comment for why) -- this is only used to render an explicit warning so
    // the operator isn't surprised when such a node silently no-ops.
    let dangerous = aerini_engine::nodes::dangerous_node_types_present(&wf.nodes);

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

    let webhook         = templates::webhook_proxy_for(&trigger)?;
    let dockerfile      = templates::build_serve_dockerfile();
    let docker_compose  = templates::build_serve_docker_compose(&wf.name, request.status_port, &credentials, &dangerous, webhook.as_ref());
    let caddyfile       = webhook.as_ref().map(|hook| templates::build_serve_caddyfile(&wf.name, hook));
    let build_override  = templates::build_serve_docker_compose_build_override(&wf.name);
    let env_example     = templates::build_docker_env_example(&credentials, webhook.as_ref());
    let readme          = templates::build_docker_readme(&wf.name, &trigger_desc, request.status_port, &credentials, &dangerous, webhook.as_ref());

    let (zip_file, zip_path) = create_export_file()?;

    let write_result: Result<(), String> = (|| {
        let mut zip = zip::ZipWriter::new(zip_file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        zip.start_file("aerini-server.json", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(server_config_str.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.start_file("Dockerfile.source", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(dockerfile.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.start_file("docker-compose.yml", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(docker_compose.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.start_file("docker-compose.build.yml", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(build_override.as_bytes())
            .map_err(|e| e.to_string())?;

        if let Some(caddyfile) = &caddyfile {
            zip.start_file("Caddyfile", options)
                .map_err(|e| e.to_string())?;
            zip.write_all(caddyfile.as_bytes())
                .map_err(|e| e.to_string())?;
        }

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
        discard_unissued(&zip_path);
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

/// Response for `export_all_workflows` — path to the generated backup zip.
#[derive(Debug, Serialize)]
pub struct ExportAllResult {
    pub zip_path:       String,
    pub workflow_count: usize,
}

/// Bundle every workflow's `.aerini` export JSON (same shape the
/// single-workflow export produces — see the frontend's `handleExport`) into
/// one zip, so a full-library backup doesn't mean exporting one at a time.
#[tauri::command]
pub async fn export_all_workflows(
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<ExportAllResult, String> {
    let db_clone = Arc::clone(&db);

    let (zip_path, count) = tokio::task::spawn_blocking(move || -> Result<(std::path::PathBuf, usize), String> {
        let summaries = db_clone.list()?;

        let (zip_file, zip_path) = create_export_file()?;

        let write_result: Result<usize, String> = (|| {
            let mut zip = zip::ZipWriter::new(zip_file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            let mut used_names: HashMap<String, u32> = HashMap::new();
            let mut count = 0usize;

            for summary in &summaries {
                let wf = db_clone.load(&summary.id)?
                    .ok_or_else(|| format!("Workflow {} disappeared during export", summary.id))?;

                let mut file = serde_json::to_value(&wf).map_err(|e| e.to_string())?;
                // collection_id is sidebar-local folder organization, not a
                // portable property of the workflow itself; same exclusion
                // the single-workflow export applies.
                if let Some(metadata) = file.get_mut("metadata") {
                    metadata["collection_id"] = serde_json::Value::Null;
                }
                file["aerini_version"] = serde_json::Value::String("1".to_string());
                let content = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;

                let base = safe_filename(&wf.name);
                let n = used_names.entry(base.clone()).or_insert(0);
                *n += 1;
                let filename = if *n == 1 { format!("{}.aerini", base) } else { format!("{}-{}.aerini", base, n) };

                zip.start_file(&filename, options).map_err(|e| e.to_string())?;
                zip.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
                count += 1;
            }

            zip.finish().map_err(|e| e.to_string())?;
            Ok(count)
        })();

        match write_result {
            Ok(count) => Ok((zip_path, count)),
            Err(e) => {
                discard_unissued(&zip_path);
                Err(e)
            }
        }
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(ExportAllResult {
        zip_path:       zip_path.display().to_string(),
        workflow_count: count,
    })
}

/// Sanitizes a workflow name into a safe, lowercase zip-entry base filename.
/// Collision numbering (handled by the caller) covers workflows that sanitize
/// to the same base name.
fn safe_filename(name: &str) -> String {
    let s = name
        .replace(|c: char| !c.is_alphanumeric() && c != '-', "_")
        .to_lowercase();
    if s.is_empty() { "workflow".to_string() } else { s }
}

/// Hash `raw_secret` with argon2id for storage in `aerini-server.json`.
///
/// argon2id is brute-force resistant (unlike BLAKE3, which is a fast hash).
/// The stored value is an argon2 PHC string (~97 chars) that embeds the salt
/// and parameters, making it self-contained for verification.
fn hash_run_secret(raw: &str) -> Result<String, argon2::password_hash::Error> {
    let salt   = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    Ok(argon2.hash_password(raw.as_bytes(), &salt)?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_file_is_owner_only_and_only_issued_files_are_accepted() {
        let (_file, path) = create_export_file().expect("export file should be created");
        let as_text = path.to_str().expect("temp path is UTF-8");
        let canonical = issued_export(as_text).expect("an issued export is accepted");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("metadata failed").permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }

        let other = tempfile::NamedTempFile::new().expect("tempfile create failed");
        assert!(issued_export(other.path().to_str().expect("temp path is UTF-8")).is_err());

        discard_export(&canonical);
        assert!(!path.exists());
        assert!(issued_export(as_text).is_err());
    }
}
