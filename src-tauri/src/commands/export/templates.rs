use aerini_engine::scheduler::job::TriggerKind;

use super::CredentialExport;

pub(super) fn build_env_example(creds: &[CredentialExport], trigger: &TriggerKind) -> String {
    let mut s = String::from("# Aerini Server — required environment variables\n\
                               # Copy this file to .env and fill in your values.\n\
                               # Never commit .env to version control.\n\n");
    if creds.is_empty() {
        s.push_str("# This workflow uses no credentials.\n");
    } else {
        for c in creds {
            s.push_str(&format!(
                "# Credential '{}' (used by node '{}')\n{}=\n\n",
                strip_newlines(&c.credential_id), strip_newlines(&c.node_name), c.env_var_name
            ));
        }
    }
    if let TriggerKind::Webhook { secret, .. } = trigger {
        if !secret.is_empty() {
            s.push_str("# Webhook secret (set in your workflow's Webhook node)\n");
            s.push_str("# This value is embedded in aerini-server.json — no env var needed.\n");
        }
    }
    s
}

pub(super) fn build_install_sh(workflow_name: &str, port: u16) -> String {
    format!(r#"#!/usr/bin/env bash
set -euo pipefail

INSTALL_DIR="$HOME/.aerini-server/{safe_name}"
SERVICE_NAME="aerini-{safe_name}"

echo "Installing Aerini Server for workflow: {safe_name}"

# Check for the binary
if [ ! -f "aerini-server" ]; then
  echo ""
  echo "ERROR: aerini-server binary not found in this directory."
  echo "Download the binary for your platform from:"
  echo "  https://github.com/Panchak2d/aerini/releases"
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
cp aerini-server    "$INSTALL_DIR/aerini-server"
cp aerini-server.json "$INSTALL_DIR/aerini-server.json"
chmod +x "$INSTALL_DIR/aerini-server"

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
chmod 600 "$INSTALL_DIR/.env"

# Install systemd user service
SERVICE_DIR="$HOME/.config/systemd/user"
mkdir -p "$SERVICE_DIR"

sed "s|{{INSTALL_DIR}}|$INSTALL_DIR|g" aerini.service > "$SERVICE_DIR/$SERVICE_NAME.service"

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
        port          = port,
    )
}

/// Maps canonical dangerous node-type ids (aerini_engine::nodes::DANGEROUS_NODE_TYPE_IDS)
/// to the exact CLI flag `aerini-server serve` needs to enable that node type.
fn allow_flags_for(dangerous: &[&str]) -> String {
    dangerous.iter().map(|id| match *id {
        "shell_exec" => "--allow-shell",
        "code"       => "--allow-code",
        "database"   => "--allow-database",
        other        => other, // unreachable for the current canonical list; fail loud, not silent
    }).collect::<Vec<_>>().join(" ")
}

pub(super) fn build_service_file(workflow_name: &str, port: u16, dangerous: &[&str]) -> String {
    let safe_name = workflow_name.replace(|c: char| !c.is_alphanumeric() && c != '-', "_");
    // dangerous node types (Shell/Code/Database) are DISABLED by
    // default in the generated service, matching aerini-server's own
    // default-secure posture (see main.rs::serve_mode) -- flags are never
    // auto-injected here, since that would grant elevated execution with no
    // human decision point at export time. If the workflow needs one, the
    // exact flag(s) are surfaced as a comment directly above ExecStart so an
    // operator editing this file sees them without having to open the README.
    let exec_flags_comment = if dangerous.is_empty() {
        String::new()
    } else {
        format!(
            "# This workflow uses: {types}. These node types are DISABLED by default.\n\
             # To enable, audit the workflow, then append to ExecStart below: {flags}\n",
            types = dangerous.join(", "),
            flags = allow_flags_for(dangerous),
        )
    };
    format!(
        "[Unit]\n\
         Description=Aerini Workflow: {safe_name}\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         WorkingDirectory={{{{INSTALL_DIR}}}}\n\
         EnvironmentFile={{{{INSTALL_DIR}}}}/.env\n\
         {exec_flags_comment}\
         ExecStart={{{{INSTALL_DIR}}}}/aerini-server --config {{{{INSTALL_DIR}}}}/aerini-server.json --port {port}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         NoNewPrivileges=yes\n\
         PrivateTmp=yes\n\
         StandardOutput=journal\n\
         StandardError=journal\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        safe_name           = safe_name,
        port                = port,
        exec_flags_comment  = exec_flags_comment,
    )
}

pub(super) fn build_serve_dockerfile() -> String {
    r#"# Aerini Server — single-workflow serve mode
# Generated by Aerini Desktop. Build and run with docker-compose.
#
# By default this clones from the official Aerini repository.
# Override with: docker compose build --build-arg AERINI_REPO=<your-fork>
#
# Build:  docker compose build
# Run:    docker compose up -d

FROM rust:1-slim AS builder

ARG AERINI_REPO=https://github.com/Panchak2d/aerini
ARG AERINI_REF=main

RUN apt-get update && \
    apt-get install -y git musl-tools && \
    rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /app
RUN git clone --depth 1 --branch "$AERINI_REF" "$AERINI_REPO" . && \
    printf '[workspace]\nmembers = ["aerini-engine", "aerini-server"]\nresolver = "2"\n' \
    > Cargo.toml && \
    cargo build --release --target x86_64-unknown-linux-musl -p aerini-server

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder \
    /app/target/x86_64-unknown-linux-musl/release/aerini-server \
    /usr/local/bin/aerini-server

RUN useradd -r -s /bin/false aerini && \
    mkdir -p /data && \
    chown aerini:aerini /data

USER aerini

VOLUME ["/data"]

ENV AERINI_DATA_DIR=/data
ENV RUST_LOG=info

EXPOSE 7700

ENTRYPOINT ["aerini-server"]
# SECURITY: The server binds to 0.0.0.0 so Docker port mapping works.
# The docker-compose.yml restricts host-side exposure to 127.0.0.1:<PORT>:7700.
# If running with docker run instead of docker compose, ALWAYS use the
# 127.0.0.1 host prefix to avoid exposing the status page on all interfaces:
#   docker run -p 127.0.0.1:<PORT>:7700 ...
# Never use:
#   docker run -p <PORT>:7700 ...  (exposes status page on all host interfaces)
CMD ["serve", "--config", "/data/aerini-server.json", "--bind", "0.0.0.0"]
"#.to_string()
}

pub(super) fn build_serve_docker_compose(
    workflow_name: &str,
    status_port:   u16,
    creds:         &[CredentialExport],
    dangerous:     &[&str],
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

    // the Dockerfile's CMD never passes --allow-shell/--allow-code/
    // --allow-database (disabled by default, same reasoning as
    // build_service_file). docker-compose's own `command:` override is the
    // one workflow-specific place we CAN surface the exact flags this export
    // needs, as a ready-to-uncomment example — still requires the operator to
    // act, never auto-enabled.
    let command_hint = if dangerous.is_empty() {
        String::new()
    } else {
        let flags_json = allow_flags_for(dangerous)
            .split(' ')
            .map(|f| format!("\"{}\"", f))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "\x20   # This workflow uses: {types}. These node types are DISABLED by default.\n\
             \x20   # To enable, audit the workflow, then uncomment and edit:\n\
             \x20   # command: [\"serve\", \"--config\", \"/data/aerini-server.json\", \"--bind\", \"0.0.0.0\", {flags_json}]\n",
            types      = dangerous.join(", "),
            flags_json = flags_json,
        )
    };

    format!(
        "# Aerini Server — {safe_name}\n\
         # Generated by Aerini Desktop.\n\
         #\n\
         # Quick start:\n\
         #   1. cp .env.example .env && edit .env\n\
         #   2. docker compose up --build -d\n\
         #   3. Status page: http://localhost:{status_port}\n\n\
         services:\n\
         \x20 aerini-{safe_name}:\n\
         \x20   build: .\n\
         \x20   image: aerini-{safe_name}:local\n\
         \x20   ports:\n\
         \x20     - \"127.0.0.1:{status_port}:7700\"\n\
         \x20   volumes:\n\
         \x20     - ./aerini-server.json:/data/aerini-server.json:ro\n\
         \x20     - aerini_{safe_name}_data:/data\n\
         {env_block}\
         {command_hint}\
         \x20   restart: unless-stopped\n\n\
         volumes:\n\
         \x20 aerini_{safe_name}_data:\n",
        safe_name     = safe_name,
        status_port   = status_port,
        env_block     = env_block,
        command_hint  = command_hint,
    )
}

pub(super) fn build_docker_env_example(creds: &[CredentialExport]) -> String {
    let mut s = String::from(
        "# Aerini Docker — required environment variables\n\
         # Copy this file to .env and fill in your values.\n\
         # Never commit .env to version control.\n\n"
    );
    if creds.is_empty() {
        s.push_str("# This workflow uses no credentials.\n");
    } else {
        for c in creds {
            s.push_str(&format!(
                "# Credential '{}' (used by node '{}')\n{}=\n\n",
                strip_newlines(&c.credential_id), strip_newlines(&c.node_name), c.env_var_name
            ));
        }
    }
    s
}

/// Strips newlines so a value can't inject extra lines when interpolated into
/// generated docs/configs — both `name` and `trigger` originate from
/// user-controlled workflow data and may be replayed via a shared/imported
/// workflow file.
fn strip_newlines(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Like `strip_newlines`, but also escapes angle brackets so the value can't
/// inject raw HTML/script tags when README.md is rendered by a viewer that
/// allows embedded HTML (e.g. GitHub).
fn escape_md(s: &str) -> String {
    strip_newlines(s).replace('<', "&lt;").replace('>', "&gt;")
}

pub(super) fn build_docker_readme(
    name:      &str,
    trigger:   &str,
    port:      u16,
    creds:     &[CredentialExport],
    dangerous: &[&str],
) -> String {
    let name    = escape_md(name);
    let trigger = escape_md(trigger);

    let cred_section = if creds.is_empty() {
        "This workflow uses no credentials.".to_string()
    } else {
        let lines: String = creds.iter()
            .map(|c| format!("  {}=<your-value>\n", c.env_var_name))
            .collect();
        format!("Edit `.env` and set:\n```\n{}```", lines)
    };

    let safe = name.replace(|c: char| !c.is_alphanumeric() && c != '-', "_");

    // Shell/Code/Database node types don't simply run — they require
    // explicit enabling, matching aerini-server's own default-secure
    // posture (see build_serve_dockerfile/build_serve_docker_compose).
    let dangerous_section = if dangerous.is_empty() {
        "This workflow does not use any node type from the dangerous list below \
         — nothing to enable.".to_string()
    } else {
        format!(
            "**This workflow uses:** {types}. These node types are **DISABLED by \
             default** — an attempt to run one reports success for the workflow but \
             fails that specific node until explicitly enabled.\n\
             \n\
             To enable, audit every node in this workflow, then uncomment the \
             `command:` override already present (commented out) in \
             `docker-compose.yml`, or add the flag(s) yourself: `{flags}`",
            types = dangerous.join(", "),
            flags = allow_flags_for(dangerous),
        )
    };

    format!(
        "# Aerini Docker Deployment\n\
         \n\
         **Workflow:** {name}  \n\
         **Trigger:** {trigger}\n\
         \n\
         ## Security Warning\n\
         \n\
         `aerini-server.json` contains the argon2id hash of your `run_secret`.\n\
The raw secret was shown once at export time and is not stored in this file.\n\
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
         docker compose logs -f aerini-{safe}\n\
         ```\n\
         \n\
         ## Credentials\n\
         \n\
         {creds}\n\
         \n\
         Credentials are read from environment variables — they are never stored in\n\
         `aerini-server.json`. Set them in `.env` before starting the container.\n\
         \n\
         ## Requirements\n\
         \n\
         - Docker Engine 20.10+ with Compose V2 (`docker compose`)\n\
         - Internet access during `docker compose build` (clones Aerini source)\n\
         - Node.js is **not** required on the host — it runs inside the container\n\
         \n\
         ## Updating the Workflow\n\
         \n\
         1. Edit the workflow in Aerini Desktop.\n\
         2. Click Export → Export for Server → Docker → Generate Package.\n\
         3. Replace `aerini-server.json` with the new file from the zip.\n\
         4. Restart: `docker compose restart aerini-{safe}`\n\
         \n\
         No rebuild required for workflow-only changes — `aerini-server.json` is mounted\n\
         read-only and reloaded on restart.\n\
         \n\
         ## Customising the Docker Build\n\
         \n\
         By default the `Dockerfile` clones Aerini from the official repository at `main`.\n\
         To pin a specific release or use a fork:\n\
         \n\
         ```bash\n\
         docker compose build \\\n\
           --build-arg AERINI_REF=v{ver} \\\n\
           --build-arg AERINI_REPO=https://github.com/Panchak2d/aerini\n\
         ```\n\
         \n\
         ## Port Mapping\n\
         \n\
         The status page is restricted to localhost via `docker-compose.yml` (`127.0.0.1:{port}:7700`).\n\
         Always deploy with `docker compose up` — this is the only supported launch method.\n\
         \n\
         **If you must use `docker run` directly**, always include the `127.0.0.1` host prefix:\n\
         ```bash\n\
         docker run -p 127.0.0.1:{port}:7700 ...   # safe: localhost only\n\
         # NOT: docker run -p {port}:7700 ...       # unsafe: exposes on all interfaces\n\
         ```\n\
         \n\
         To expose the status page publicly, edit `docker-compose.yml`:\n\
         ```yaml\n\
         ports:\n\
           - \"0.0.0.0:{port}:7700\"   # expose on all interfaces\n\
         ```\n\
         Use a reverse proxy (Caddy, Nginx) with TLS for public-facing deployments.\n\
         \n\
         ## Dangerous Node Types (Shell Command, Code, Database)\n\
         \n\
         {dangerous_section}\n\
         \n\
         | Node type | Flag | Risk |\n\
         |---|---|---|\n\
         | Shell Command | `--allow-shell` | Executes arbitrary OS commands as the container's `aerini` user (no elevated capabilities, but full command execution within the container). |\n\
         | Code (JS) | `--allow-code` | Spawns a Node.js subprocess as the container's `aerini` user. |\n\
         | Database | `--allow-database` | Connects to PostgreSQL/MySQL/SQLite/Redis; SSRF surface and RUSTSEC-2023-0071 (RSA timing side-channel in sqlx-mysql). |\n",
        ver       = env!("CARGO_PKG_VERSION"),
        name      = name,
        trigger   = trigger,
        port      = port,
        safe      = safe,
        creds     = cred_section,
        dangerous_section = dangerous_section,
    )
}

pub(super) fn build_readme(
    name: &str, trigger: &str, port: u16, creds: &[CredentialExport], dangerous: &[&str]
) -> String {
    let name    = strip_newlines(name);
    let trigger = strip_newlines(trigger);

    let cred_section = if creds.is_empty() {
        "This workflow uses no credentials.".to_string()
    } else {
        let lines: String = creds.iter()
            .map(|c| format!("  {}={}\n", c.env_var_name, "<your-value>"))
            .collect();
        format!("Edit .env and set:\n{}", lines)
    };

    // Shell/Code/Database node types are DISABLED by default — a workflow
    // using one would silently no-op that node with only a journalctl line
    // to explain why.
    let dangerous_section = if dangerous.is_empty() {
        "This workflow does not use any node type from the list below — nothing to enable.".to_string()
    } else {
        format!(
            "This workflow uses: {types}. These node types are DISABLED by default.\n\
             \n\
             To enable, audit every node in this workflow first, then edit\n\
             ~/.config/systemd/user/aerini-{safe}.service and append to the ExecStart\n\
             line:\n\
             \x20 {flags}\n\
             Then: systemctl --user daemon-reload && systemctl --user restart aerini-{safe}\n\
             (Re-running install.sh regenerates this file from aerini.service in this zip —\n\
             \x20add the flag there too, or re-apply it after any re-install.)",
            types = dangerous.join(", "),
            flags = allow_flags_for(dangerous),
            safe  = name.replace(|c: char| !c.is_alphanumeric() && c != '-', "_"),
        )
    };

    format!(
        "Aerini Server Deployment Package\n\
         ================================\n\
         Workflow : {name}\n\
         Trigger  : {trigger}\n\
         \n\
         SECURITY WARNING\n\
         ----------------\n\
         aerini-server.json contains the argon2id hash of your run_secret.\n\
The raw secret was shown once at export time and is not stored in this file.\n\
Treat this zip like a credentials file:\n\
         - Do NOT commit it to version control.\n\
         - Do NOT share it over unencrypted channels.\n\
         - Do NOT store it in a world-readable directory.\n\
         After deployment, restrict permissions:\n\
           chmod 600 ~/.aerini-server/{safe}/aerini-server.json\n\
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
         - Node.js (only required if workflow uses Code nodes, and only once enabled — see below)\n\
         \n\
         CREDENTIALS\n\
         -----------\n\
         Credentials are read from environment variables — they are never\n\
         stored in aerini-server.json or the binary.\n\
         Edit .env before running install.sh, or after installation:\n\
           nano ~/.aerini-server/{safe}/.env\n\
           systemctl --user restart aerini-{safe}\n\
         \n\
         DANGEROUS NODE TYPES (Shell Command, Code, Database)\n\
         ------------------------------------------------------\n\
         {dangerous_section}\n\
         \n\
         --allow-shell     Shell Command: executes arbitrary OS commands as the\n\
         \x20                 server process's own user. Commands that use macOS or\n\
         \x20                 Windows tooling will fail — this runs on your server's\n\
         \x20                 Linux environment, not your desktop.\n\
         --allow-code      Code (JS): spawns a Node.js subprocess as the server\n\
         \x20                 process's own user.\n\
         --allow-database  Database: connects to PostgreSQL/MySQL/SQLite/Redis;\n\
         \x20                 SSRF surface and RUSTSEC-2023-0071 (RSA timing\n\
         \x20                 side-channel in sqlx-mysql).\n\
         \n\
         DESKTOP NOTIFICATION NODES\n\
         --------------------------\n\
         Desktop Notification nodes will log an error and continue.\n\
         This does not crash the workflow.\n\
         \n\
         UPDATING THE WORKFLOW\n\
         ---------------------\n\
         1. Edit the workflow in Aerini Desktop.\n\
         2. Click Export → Export for Server → Generate Package.\n\
         3. Upload the new zip and run install.sh again.\n\
         \n\
         LOGS\n\
         ----\n\
           journalctl --user -u aerini-{safe} -f\n",
        name    = name,
        trigger = trigger,
        port    = port,
        creds   = cred_section,
        safe    = name.replace(|c: char| !c.is_alphanumeric() && c != '-', "_"),
        dangerous_section = dangerous_section,
    )
}

#[cfg(test)]
mod dangerous_export_tests {
    use super::*;

    #[test]
    fn allow_flags_for_maps_each_canonical_id() {
        assert_eq!(allow_flags_for(&["shell_exec"]), "--allow-shell");
        assert_eq!(allow_flags_for(&["code"]), "--allow-code");
        assert_eq!(allow_flags_for(&["database"]), "--allow-database");
        assert_eq!(
            allow_flags_for(&["shell_exec", "database"]),
            "--allow-shell --allow-database"
        );
        assert_eq!(allow_flags_for(&[]), "");
    }

    #[test]
    fn service_file_has_no_flag_comment_when_no_dangerous_nodes() {
        let out = build_service_file("wf", 7700, &[]);
        assert!(!out.contains("DISABLED by default"));
        // ExecStart must still be the very next non-blank line after
        // EnvironmentFile — no stray blank comment line left behind.
        assert!(out.contains("EnvironmentFile={{INSTALL_DIR}}/.env\nExecStart="));
    }

    #[test]
    fn service_file_lists_exact_flags_needed_above_execstart() {
        let out = build_service_file("wf", 7700, &["shell_exec", "database"]);
        assert!(out.contains("shell_exec, database"));
        assert!(out.contains("--allow-shell --allow-database"));
        // Comment must come before ExecStart, not after (an operator editing
        // top-to-bottom must see it before reaching the line it explains).
        let comment_pos = out.find("DISABLED by default").unwrap();
        let exec_pos    = out.find("ExecStart=").unwrap();
        assert!(comment_pos < exec_pos);
    }

    #[test]
    fn readme_and_docker_readme_state_nothing_to_enable_when_safe() {
        let readme = build_readme("wf", "Schedule", 7700, &[], &[]);
        assert!(readme.contains("nothing to enable"));
        let docker_readme = build_docker_readme("wf", "Schedule", 7700, &[], &[]);
        assert!(docker_readme.contains("nothing to enable"));
    }

    #[test]
    fn readme_and_docker_readme_surface_flags_when_dangerous_present() {
        let readme = build_readme("wf", "Schedule", 7700, &[], &["code"]);
        assert!(readme.contains("--allow-code"));
        assert!(readme.contains("DISABLED by default"));
        let docker_readme = build_docker_readme("wf", "Schedule", 7700, &[], &["code"]);
        assert!(docker_readme.contains("--allow-code"));
        assert!(docker_readme.contains("DISABLED by default"));
    }

    #[test]
    fn readme_and_docker_readme_security_warning_reads_as_one_flowing_paragraph() {
        let readme = build_readme("wf", "Schedule", 7700, &[], &[]);
        assert!(!readme.contains("\\n\\"));
        assert!(readme.contains("run_secret.\nThe raw secret was shown once"));
        assert!(readme.contains("stored in this file.\nTreat this zip"));

        let docker_readme = build_docker_readme("wf", "Schedule", 7700, &[], &[]);
        assert!(!docker_readme.contains("\\n\\"));
        assert!(docker_readme.contains("run_secret`.\nThe raw secret was shown once"));
        assert!(docker_readme.contains("stored in this file.\nTreat this zip"));
    }

    #[test]
    fn docker_compose_command_hint_only_present_when_dangerous() {
        let safe = build_serve_docker_compose("wf", 7700, &[], &[]);
        assert!(!safe.contains("# command:"));
        let unsafe_ = build_serve_docker_compose("wf", 7700, &[], &["shell_exec"]);
        assert!(unsafe_.contains("# command:"));
        assert!(unsafe_.contains("\"--allow-shell\""));
    }
}

#[cfg(test)]
mod security_hardening_tests {
    use super::*;

    #[test]
    fn service_file_includes_hardening_directives() {
        let out = build_service_file("wf", 7700, &[]);
        assert!(out.contains("NoNewPrivileges=yes"));
        assert!(out.contains("PrivateTmp=yes"));
    }

    #[test]
    fn install_sh_chmods_env_file() {
        let out = build_install_sh("wf", 7700);
        assert!(out.contains(r#"chmod 600 "$INSTALL_DIR/.env""#));
    }
}
