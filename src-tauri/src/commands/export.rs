use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};

use flowo_engine::{
    db::WorkflowDb,
    model::Workflow,
    scheduler::extract_trigger,
    scheduler::job::TriggerKind,
};
use serde::{Deserialize, Serialize};
use tauri::Manager;

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
    /// stores only the BLAKE3 hash.
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
    // Uses uuid v4 for 122 bits of entropy — adequate for a status page token.
    // Stored as an argon2id hash (brute-force resistant); the raw secret is
    // returned to the UI for one-time display and never written to disk.
    // spawn_blocking: argon2 is CPU-intensive and must not block the async runtime.
    let run_secret_raw   = uuid::Uuid::new_v4().to_string().replace('-', "");
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

    let env_example = build_env_example(&credentials, &trigger);

    let install_sh = build_install_sh(&wf.name, request.status_port);

    let service_file = build_service_file(&wf.name, request.status_port);

    let readme = build_readme(&wf.name, &trigger_desc, request.status_port, &credentials);

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
        for (_, cred_id) in &node.credentials {
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
    format!("FLOWO_CRED_{}", id.to_uppercase().replace('-', "_").replace('.', "_"))
}

fn build_env_example(creds: &[CredentialExport], trigger: &TriggerKind) -> String {
    let mut s = String::from("# Flowo Server — required environment variables\n\
                               # Copy this file to .env and fill in your values.\n\
                               # Never commit .env to version control.\n\n");
    if creds.is_empty() {
        s.push_str("# This workflow uses no credentials.\n");
    } else {
        for c in creds {
            s.push_str(&format!(
                "# Credential '{}' (used by node '{}')\n{}=\n\n",
                c.credential_id, c.node_name, c.env_var_name
            ));
        }
    }
    if let TriggerKind::Webhook { secret, .. } = trigger {
        if !secret.is_empty() {
            s.push_str("# Webhook secret (set in your workflow's Webhook node)\n");
            s.push_str("# This value is embedded in flowo-server.json — no env var needed.\n");
        }
    }
    s
}

fn build_install_sh(workflow_name: &str, port: u16) -> String {
    format!(r#"#!/usr/bin/env bash
set -euo pipefail

INSTALL_DIR="$HOME/.flowo-server/{safe_name}"
SERVICE_NAME="flowo-{safe_name}"

echo "Installing Flowo Server for workflow: {workflow_name}"

# Check for the binary
if [ ! -f "flowo-server" ]; then
  echo ""
  echo "ERROR: flowo-server binary not found in this directory."
  echo "Download the binary for your platform from:"
  echo "  https://github.com/your-org/flowo/releases"
  echo "Then re-run this installer."
  exit 1
fi

# Check Node.js (required for Code nodes)
if ! command -v node &>/dev/null; then
  echo "WARNING: Node.js is not installed. Code nodes will fail."
  echo "Install with: sudo apt install -y nodejs   (Debian/Ubuntu)"
  echo "              sudo yum install -y nodejs    (RHEL/CentOS)"
fi

mkdir -p "$INSTALL_DIR"
cp flowo-server    "$INSTALL_DIR/flowo-server"
cp flowo-server.json "$INSTALL_DIR/flowo-server.json"
chmod +x "$INSTALL_DIR/flowo-server"

# Set up .env if it doesn't exist
if [ ! -f "$INSTALL_DIR/.env" ] && [ -f ".env" ]; then
  cp .env "$INSTALL_DIR/.env"
  echo "Copied .env to $INSTALL_DIR/.env"
elif [ ! -f "$INSTALL_DIR/.env" ]; then
  cp .env.example "$INSTALL_DIR/.env"
  echo ""
  echo "IMPORTANT: Edit $INSTALL_DIR/.env and fill in your credential values,"
  echo "then run: systemctl --user restart $SERVICE_NAME"
fi

# Install systemd user service
SERVICE_DIR="$HOME/.config/systemd/user"
mkdir -p "$SERVICE_DIR"

sed "s|{{INSTALL_DIR}}|$INSTALL_DIR|g" flowo.service > "$SERVICE_DIR/$SERVICE_NAME.service"

# Enable linger so service survives logout
if loginctl show-user "$USER" 2>/dev/null | grep -q "Linger=no"; then
  loginctl enable-linger "$USER" && echo "Enabled systemd linger for $USER" || \
    echo "WARNING: Could not enable linger. Service may not start on reboot."
fi

systemctl --user daemon-reload
systemctl --user enable "$SERVICE_NAME"
systemctl --user start  "$SERVICE_NAME"

echo ""
echo "Installation complete."
echo "  Status page: http://$(hostname -I | awk '{{print $1}}'):{port}"
echo "  Logs:        journalctl --user -u $SERVICE_NAME -f"
echo "  Stop:        systemctl --user stop $SERVICE_NAME"
echo "  Restart:     systemctl --user restart $SERVICE_NAME"
"#,
        safe_name     = workflow_name.replace(|c: char| !c.is_alphanumeric() && c != '-', "_"),
        workflow_name = workflow_name,
        port          = port,
    )
}

fn build_service_file(workflow_name: &str, port: u16) -> String {
    format!(
        "[Unit]\n\
         Description=Flowo Workflow: {workflow_name}\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         WorkingDirectory={{{{INSTALL_DIR}}}}\n\
         EnvironmentFile={{{{INSTALL_DIR}}}}/.env\n\
         ExecStart={{{{INSTALL_DIR}}}}/flowo-server --config {{{{INSTALL_DIR}}}}/flowo-server.json --port {port}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         StandardOutput=journal\n\
         StandardError=journal\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        workflow_name = workflow_name,
        port          = port,
    )
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

    let run_secret_raw   = uuid::Uuid::new_v4().to_string().replace('-', "");
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

    let dockerfile      = build_serve_dockerfile();
    let docker_compose  = build_serve_docker_compose(&wf.name, request.status_port, &credentials);
    let env_example     = build_docker_env_example(&credentials);
    let readme          = build_docker_readme(&wf.name, &trigger_desc, request.status_port, &credentials);

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

fn build_serve_dockerfile() -> String {
    r#"# Flowo Server — single-workflow serve mode
# Generated by Flowo Desktop. Build and run with docker-compose.
#
# By default this clones from the official Flowo repository.
# Override with: docker compose build --build-arg FLOWO_REPO=<your-fork>
#
# Build:  docker compose build
# Run:    docker compose up -d

FROM rust:1-slim AS builder

ARG FLOWO_REPO=https://github.com/flowo-dev/flowo
ARG FLOWO_REF=main

RUN apt-get update && \
    apt-get install -y git musl-tools && \
    rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /app
RUN git clone --depth 1 --branch "$FLOWO_REF" "$FLOWO_REPO" . && \
    printf '[workspace]\nmembers = ["flowo-engine", "flowo-server"]\nresolver = "2"\n' \
    > Cargo.toml && \
    cargo build --release --target x86_64-unknown-linux-musl -p flowo-server

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder \
    /app/target/x86_64-unknown-linux-musl/release/flowo-server \
    /usr/local/bin/flowo-server

RUN useradd -r -s /bin/false flowo && \
    mkdir -p /data && \
    chown flowo:flowo /data

USER flowo

VOLUME ["/data"]

ENV FLOWO_DATA_DIR=/data
ENV RUST_LOG=info

EXPOSE 7700

ENTRYPOINT ["flowo-server"]
CMD ["serve", "--config", "/data/flowo-server.json", "--bind", "0.0.0.0"]
"#.to_string()
}

fn build_serve_docker_compose(
    workflow_name: &str,
    status_port:   u16,
    creds:         &[CredentialExport],
) -> String {
    let safe_name = workflow_name.replace(|c: char| !c.is_alphanumeric() && c != '-', "_");

    // env_file injects all credential vars from .env directly into the container.
    // No need to list each var explicitly in environment: — that would require
    // the host shell to also have them set for docker-compose interpolation.
    let env_block = if creds.is_empty() {
        "    environment:\n      RUST_LOG: info\n".to_string()
    } else {
        "    env_file:\n      - .env\n    environment:\n      RUST_LOG: info\n".to_string()
    };

    format!(
        "# Flowo Server — {workflow_name}\n\
         # Generated by Flowo Desktop.\n\
         #\n\
         # Quick start:\n\
         #   1. cp .env.example .env && edit .env\n\
         #   2. docker compose up --build -d\n\
         #   3. Status page: http://localhost:{status_port}\n\n\
         services:\n\
         \x20 flowo-{safe_name}:\n\
         \x20   build: .\n\
         \x20   image: flowo-{safe_name}:local\n\
         \x20   ports:\n\
         \x20     - \"127.0.0.1:{status_port}:7700\"\n\
         \x20   volumes:\n\
         \x20     - ./flowo-server.json:/data/flowo-server.json:ro\n\
         \x20     - flowo_{safe_name}_data:/data\n\
         {env_block}\
         \x20   restart: unless-stopped\n\n\
         volumes:\n\
         \x20 flowo_{safe_name}_data:\n",
        workflow_name = workflow_name,
        safe_name     = safe_name,
        status_port   = status_port,
        env_block     = env_block,
    )
}

fn build_docker_env_example(creds: &[CredentialExport]) -> String {
    let mut s = String::from(
        "# Flowo Docker — required environment variables\n\
         # Copy this file to .env and fill in your values.\n\
         # Never commit .env to version control.\n\n"
    );
    if creds.is_empty() {
        s.push_str("# This workflow uses no credentials.\n");
    } else {
        for c in creds {
            s.push_str(&format!(
                "# Credential '{}' (used by node '{}')\n{}=\n\n",
                c.credential_id, c.node_name, c.env_var_name
            ));
        }
    }
    s
}

fn build_docker_readme(
    name:    &str,
    trigger: &str,
    port:    u16,
    creds:   &[CredentialExport],
) -> String {
    let cred_section = if creds.is_empty() {
        "This workflow uses no credentials.".to_string()
    } else {
        let lines: String = creds.iter()
            .map(|c| format!("  {}=<your-value>\n", c.env_var_name))
            .collect();
        format!("Edit `.env` and set:\n```\n{}```", lines)
    };

    let safe = name.replace(|c: char| !c.is_alphanumeric() && c != '-', "_");

    format!(
        "# Flowo Docker Deployment\n\
         \n\
         **Workflow:** {name}  \n\
         **Trigger:** {trigger}\n\
         \n\
         ## Security Warning\n\
         \n\
         `flowo-server.json` contains the BLAKE3 hash of your `run_secret`.\\n\\
         The raw secret was shown once at export time and is not stored in this file.\\n\\
         Treat this zip like a credentials file: do not commit it to version\n\
         control, do not share it over unencrypted channels, and do not store\n\
         it in a world-readable directory.\n\
         \n\
         ## Quick Start\n\
         \n\
         ```bash\n\
         # 1. Fill in credentials\n\
         cp .env.example .env\n\
         $EDITOR .env\n\
         \n\
         # 2. Build and start\n\
         docker compose up --build -d\n\
         \n\
         # 3. View status page\n\
         open http://localhost:{port}\n\
         \n\
         # 4. Follow logs\n\
         docker compose logs -f flowo-{safe}\n\
         ```\n\
         \n\
         ## Credentials\n\
         \n\
         {creds}\n\
         \n\
         Credentials are read from environment variables — they are never stored in\n\
         `flowo-server.json`. Set them in `.env` before starting the container.\n\
         \n\
         ## Requirements\n\
         \n\
         - Docker Engine 20.10+ with Compose V2 (`docker compose`)\n\
         - Internet access during `docker compose build` (clones Flowo source)\n\
         - Node.js is **not** required on the host — it runs inside the container\n\
         \n\
         ## Updating the Workflow\n\
         \n\
         1. Edit the workflow in Flowo Desktop.\n\
         2. Click Export → Export for Server → Docker → Generate Package.\n\
         3. Replace `flowo-server.json` with the new file from the zip.\n\
         4. Restart: `docker compose restart flowo-{safe}`\n\
         \n\
         No rebuild required for workflow-only changes — `flowo-server.json` is mounted\n\
         read-only and reloaded on restart.\n\
         \n\
         ## Customising the Docker Build\n\
         \n\
         By default the `Dockerfile` clones Flowo from the official repository at `main`.\n\
         To pin a specific release or use a fork:\n\
         \n\
         ```bash\n\
         docker compose build \\\n\
           --build-arg FLOWO_REF=v0.2.0 \\\n\
           --build-arg FLOWO_REPO=https://github.com/your-org/flowo\n\
         ```\n\
         \n\
         ## Port Mapping\n\
         \n\
         The status page is bound to `127.0.0.1:{port}` by default (localhost only).\n\
         To expose it publicly, edit `docker-compose.yml`:\n\
         ```yaml\n\
         ports:\n\
           - \"0.0.0.0:{port}:7700\"   # expose on all interfaces\n\
         ```\n\
         Use a reverse proxy (Caddy, Nginx) with TLS for public-facing deployments.\n\
         \n\
         ## Shell Command and Code Nodes\n\
         \n\
         Shell Command and Code nodes execute inside the container with the privileges\n\
         of the `flowo` user. The container has no elevated capabilities.\n",
        name    = name,
        trigger = trigger,
        port    = port,
        safe    = safe,
        creds   = cred_section,
    )
}

fn build_readme(
    name: &str, trigger: &str, port: u16, creds: &[CredentialExport]
) -> String {
    let cred_section = if creds.is_empty() {
        "This workflow uses no credentials.".to_string()
    } else {
        let lines: String = creds.iter()
            .map(|c| format!("  {}={}\n", c.env_var_name, "<your-value>"))
            .collect();
        format!("Edit .env and set:\n{}", lines)
    };

    format!(
        "Flowo Server Deployment Package\n\
         ================================\n\
         Workflow : {name}\n\
         Trigger  : {trigger}\n\
         \n\
         SECURITY WARNING\n\
         ----------------\n\
         flowo-server.json contains the BLAKE3 hash of your run_secret.\\n\\
         The raw secret was shown once at export time and is not stored in this file.\\n\\
         Treat this zip like a credentials file:\n\
         - Do NOT commit it to version control.\n\
         - Do NOT share it over unencrypted channels.\n\
         - Do NOT store it in a world-readable directory.\n\
         After deployment, restrict permissions:\n\
           chmod 600 ~/.flowo-server/{safe}/flowo-server.json\n\
         \n\
         QUICK START\n\
         -----------\n\
         1. Upload this zip to your Linux server and unzip it.\n\
         2. {creds}\n\
         3. Run: chmod +x install.sh && ./install.sh\n\
         4. Status page: http://your-server-ip:{port}\n\
         \n\
         REQUIREMENTS\n\
         ------------\n\
         - Linux x86_64 (Ubuntu 20.04+, Debian 11+, CentOS 7+, Alpine)\n\
         - systemd with user lingering enabled (install.sh handles this)\n\
         - Node.js (only required if workflow uses Code nodes)\n\
         \n\
         CREDENTIALS\n\
         -----------\n\
         Credentials are read from environment variables — they are never\n\
         stored in flowo-server.json or the binary.\n\
         Edit .env before running install.sh, or after installation:\n\
           nano ~/.flowo-server/{safe}/.env\n\
           systemctl --user restart flowo-{safe}\n\
         \n\
         SHELL COMMAND NODES\n\
         -------------------\n\
         Shell Command nodes execute on the server's Linux environment,\n\
         not your desktop. Commands that use macOS or Windows tooling will fail.\n\
         \n\
         DESKTOP NOTIFICATION NODES\n\
         --------------------------\n\
         Desktop Notification nodes will log an error and continue.\n\
         This does not crash the workflow.\n\
         \n\
         UPDATING THE WORKFLOW\n\
         ---------------------\n\
         1. Edit the workflow in Flowo Desktop.\n\
         2. Click Export → Export for Server → Generate Package.\n\
         3. Upload the new zip and run install.sh again.\n\
         \n\
         LOGS\n\
         ----\n\
           journalctl --user -u flowo-{safe} -f\n",
        name    = name,
        trigger = trigger,
        port    = port,
        creds   = cred_section,
        safe    = name.replace(|c: char| !c.is_alphanumeric() && c != '-', "_"),
    )
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
