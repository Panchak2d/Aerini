use aerini_engine::scheduler::job::TriggerKind;

use super::CredentialExport;

const SAFE_NAME_MAX: usize = 48;

/// Turns a workflow name into a lowercase ASCII name that systemd, Docker image
/// references, Docker volume names and shell paths all accept.
pub(super) fn safe_name(name: &str) -> String {
    let mut out = String::new();
    let mut pending_separator = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_separator && !out.is_empty() {
                out.push('-');
            }
            pending_separator = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_separator = true;
        }
    }
    out.truncate(SAFE_NAME_MAX);
    let trimmed = out.trim_end_matches('-').len();
    out.truncate(trimmed);
    if out.is_empty() {
        "workflow".to_string()
    } else {
        out
    }
}

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

pub(super) fn build_install_sh(workflow_name: &str, port: u16, uses_code_node: bool) -> String {
    // The Code node never searches PATH; it only uses node-bundled beside the
    // binary or AERINI_NODE_BIN from .env. Only workflows with a Code node need to hear it.
    let node_note = if uses_code_node {
        r#"
if [ ! -f "$INSTALL_DIR/node-bundled" ] && ! grep -qs '^AERINI_NODE_BIN=[^[:space:]]' "$INSTALL_DIR/.env"; then
  echo ""
  echo "NOTE: This workflow has a Code (JS) node, which needs one of these:"
  echo "  - AERINI_NODE_BIN=/path/to/node in $INSTALL_DIR/.env"
  echo "  - a Node.js binary named node-bundled in $INSTALL_DIR"
  echo "Installing Node.js with a package manager is not enough on its own."
  echo "Details: https://docs.aerini.org/operations/server-deploy#code-js-nodes-need-a-bundled-runtime-not-a-system-node-install"
fi
"#
    } else {
        ""
    };
    format!(r#"#!/usr/bin/env bash
set -euo pipefail

umask 077

INSTALL_DIR="$HOME/.aerini-server/{safe_name}"
SERVICE_NAME="aerini-{safe_name}"
VERSION="{version}"
RELEASE_URL="https://github.com/Panchak2d/aerini/releases/download/v$VERSION"
DOWNLOAD_DIR=""
trap 'if [ -n "$DOWNLOAD_DIR" ]; then rm -rf "$DOWNLOAD_DIR"; fi' EXIT

echo "Installing Aerini Server for workflow: {safe_name}"

cannot_download() {{
  echo ""
  echo "ERROR: $1"
  echo "Add aerini-server to this folder yourself, then re-run this installer:"
  echo "  https://docs.aerini.org/operations/server-deploy#getting-the-aerini-server-binary"
  exit 1
}}

# Use the program beside this script if there is one. Otherwise fetch the build
# that matches the Aerini version that exported this package, so the workflow
# file and the program agree on the schema version.
if [ ! -f "aerini-server" ]; then
  echo "aerini-server not found in this folder. Downloading v$VERSION from GitHub Releases."
  [ "$(uname -s)" = "Linux" ] || cannot_download "This installer only supports Linux."
  case "$(uname -m)" in
    x86_64|amd64) TARGET="x86_64-unknown-linux-musl" ;;
    *) cannot_download "No download is published for the $(uname -m) architecture." ;;
  esac
  for tool in curl sha256sum tar mktemp; do
    command -v "$tool" >/dev/null 2>&1 || cannot_download "$tool is required to download aerini-server but is not installed."
  done
  ARCHIVE="aerini-server_${{VERSION}}_${{TARGET}}.tar.gz"
  DOWNLOAD_DIR="$(mktemp -d)"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$DOWNLOAD_DIR/$ARCHIVE" "$RELEASE_URL/$ARCHIVE" \
    || cannot_download "Could not download $RELEASE_URL/$ARCHIVE. This version may not be released yet."
  curl -fsSL --proto '=https' --tlsv1.2 -o "$DOWNLOAD_DIR/$ARCHIVE.sha256" "$RELEASE_URL/$ARCHIVE.sha256" \
    || cannot_download "Could not download the checksum file for $ARCHIVE."
  (cd "$DOWNLOAD_DIR" && sha256sum -c "$ARCHIVE.sha256" >/dev/null 2>&1) \
    || cannot_download "The checksum of $ARCHIVE does not match, so it was not installed."
  tar -xzf "$DOWNLOAD_DIR/$ARCHIVE" -C "$DOWNLOAD_DIR" aerini-server \
    || cannot_download "Could not extract aerini-server from $ARCHIVE."
  mv "$DOWNLOAD_DIR/aerini-server" ./aerini-server
  echo "Downloaded and verified $ARCHIVE"
fi

# A running program cannot be overwritten, so stop it before an in-place update.
systemctl --user stop "$SERVICE_NAME" 2>/dev/null || true

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
{node_note}
# Install systemd user service
SERVICE_DIR="$HOME/.config/systemd/user"
mkdir -p "$SERVICE_DIR"

sed "s|{{{{INSTALL_DIR}}}}|$INSTALL_DIR|g" aerini.service > "$SERVICE_DIR/$SERVICE_NAME.service"

# Lingering keeps the service running after logout and starts it at boot.
linger_enabled() {{
  local state
  state="$(loginctl show-user "$USER" 2>/dev/null || true)"
  [[ "$state" == *Linger=yes* ]]
}}
if ! linger_enabled; then
  loginctl enable-linger "$USER" 2>/dev/null || sudo -n loginctl enable-linger "$USER" 2>/dev/null || true
fi

systemctl --user daemon-reload
systemctl --user enable "$SERVICE_NAME"
systemctl --user start  "$SERVICE_NAME"

echo ""
echo "Installation complete."
echo "  Status page: http://127.0.0.1:{port} (the service listens on this server only)"
echo "  From your own computer: ssh -L {port}:127.0.0.1:{port} <user>@<this-server>, then open the same address"
echo "  To expose it, put a reverse proxy in front: https://docs.aerini.org/operations/server-deploy#tls-with-a-reverse-proxy"
echo "  Logs:        journalctl --user -u $SERVICE_NAME -f"
echo "  Stop:        systemctl --user stop $SERVICE_NAME"
echo "  Restart:     systemctl --user restart $SERVICE_NAME"

if ! linger_enabled; then
  echo ""
  echo "ACTION REQUIRED: lingering is off, so this service will NOT start again after a reboot."
  echo "Run this once (no need to run install.sh again):"
  echo "  sudo loginctl enable-linger $USER"
fi
"#,
        safe_name     = safe_name(workflow_name),
        port          = port,
        version       = env!("CARGO_PKG_VERSION"),
        node_note     = node_note,
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
    let safe_name = safe_name(workflow_name);
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
         ExecStart={{{{INSTALL_DIR}}}}/aerini-server serve --config {{{{INSTALL_DIR}}}}/aerini-server.json --port {port}\n\
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

/// Port the status server listens on inside the Docker container; the generated
/// commands pass it explicitly, so a Webhook trigger cannot use it too.
const CONTAINER_STATUS_PORT: u16 = 7700;

/// What the generated Docker package needs in order to expose a Webhook trigger.
pub(super) struct WebhookProxy {
    pub port:       u16,
    pub path:       String,
    pub method:     String,
    pub has_secret: bool,
}

/// The webhook listener binds 127.0.0.1 inside the container, which Docker cannot
/// publish, so a Webhook trigger gets a Caddy sidecar. Path and method come from
/// the workflow file and are written into the Caddyfile, so they are restricted
/// to characters that cannot change the file's structure.
pub(super) fn webhook_proxy_for(trigger: &TriggerKind) -> Result<Option<WebhookProxy>, String> {
    let TriggerKind::Webhook { port, path, method, secret, .. } = trigger else {
        return Ok(None);
    };
    if *port == CONTAINER_STATUS_PORT {
        return Err(format!(
            "The Webhook port {port} is the status page's port inside the Docker container. \
             Choose another port in the Webhook node."
        ));
    }
    let path_ok = path.starts_with('/')
        && path.len() <= 200
        && path.chars().all(|c| c.is_ascii_alphanumeric() || "/_.~-".contains(c));
    if !path_ok {
        return Err("The Webhook path may only use letters, digits and / _ . ~ - when exporting \
                    for Docker. Change the path in the Webhook node."
            .to_string());
    }
    let method_ok = method == "ANY"
        || (!method.is_empty() && method.len() <= 16 && method.chars().all(|c| c.is_ascii_uppercase()));
    if !method_ok {
        return Err("The Webhook method must be ANY or a plain HTTP method name such as POST."
            .to_string());
    }
    Ok(Some(WebhookProxy {
        port:       *port,
        path:       path.clone(),
        method:     method.clone(),
        has_secret: !secret.is_empty(),
    }))
}

pub(super) fn build_serve_dockerfile() -> String {
    r#"# Aerini Server — single-workflow serve mode, built from source
# Generated by Aerini Desktop. This file is only used through docker-compose.build.yml;
# the default docker-compose.yml runs the prebuilt official image and needs no build.
#
# This clones the official Aerini repository at the release tag that matches the
# Aerini Desktop that exported this package.
# Override with: docker compose -f docker-compose.yml -f docker-compose.build.yml \
#                  build --build-arg AERINI_REF=<tag-or-branch>
#                (or --build-arg AERINI_REPO=<your-fork>)
#
# Build and run:
#   docker compose -f docker-compose.yml -f docker-compose.build.yml up --build -d

FROM rust:1-slim AS builder

ARG AERINI_REPO=https://github.com/Panchak2d/aerini
ARG AERINI_REF=@AERINI_REF@

RUN apt-get update && \
    apt-get install -y git musl-tools curl ca-certificates xz-utils binutils && \
    rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /app
# src-tauri is dropped from the workspace members: its desktop dependencies
# cannot build in a headless container and aerini-server does not depend on it.
# The cloned manifest is edited rather than replaced so the [workspace.package]
# fields (version, edition, license) still reach the member crates. Dropping the
# member prunes Cargo.lock, so the build must not pass --locked.
RUN git clone --depth 1 --branch "$AERINI_REF" "$AERINI_REPO" . && \
    sed -i '/^[[:space:]]*"src-tauri",[[:space:]]*$/d' Cargo.toml && \
    if grep -q 'src-tauri' Cargo.toml; then \
      echo "ERROR: src-tauri is still referenced in Cargo.toml after the sed edit; the cloned repository's workspace layout is not supported by this Dockerfile" >&2; \
      exit 1; \
    fi && \
    cargo build --release --target x86_64-unknown-linux-musl -p aerini-server

# Node.js for the Code node's server-side sandbox. Version is read from the
# cloned repository's own NODE_VERSION file, the same source
# scripts/fetch-node-binaries.sh and this repository's root Dockerfile use,
# so a fork built via AERINI_REPO/AERINI_REF above pins whatever version that
# fork declares.
RUN NODE_VERSION="$(head -n1 NODE_VERSION | tr -d '\r')" && \
    if ! echo "$NODE_VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then \
      echo "ERROR: NODE_VERSION file must have a bare semver as its first line, got: '$NODE_VERSION'" >&2; \
      exit 1; \
    fi && \
    curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/SHASUMS256.txt" -o /tmp/node-shasums.txt && \
    curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" -o /tmp/node.tar.xz && \
    NODE_EXPECTED_SHA256="$(awk -v f="node-v${NODE_VERSION}-linux-x64.tar.xz" '$2==f {print $1; exit}' /tmp/node-shasums.txt)" && \
    [ -n "$NODE_EXPECTED_SHA256" ] && \
    echo "${NODE_EXPECTED_SHA256}  /tmp/node.tar.xz" | sha256sum -c - && \
    tar -xJf /tmp/node.tar.xz -C /tmp && \
    cp "/tmp/node-v${NODE_VERSION}-linux-x64/bin/node" /app/node-bundled && \
    chmod +x /app/node-bundled && \
    strip /app/node-bundled && \
    [ "$(/app/node-bundled --version)" = "v${NODE_VERSION}" ] && \
    rm -rf /tmp/node.tar.xz /tmp/node-shasums.txt "/tmp/node-v${NODE_VERSION}-linux-x64"

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libstdc++6 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder \
    /app/target/x86_64-unknown-linux-musl/release/aerini-server \
    /usr/local/bin/aerini-server
COPY --from=builder /app/node-bundled /usr/local/bin/node-bundled

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
CMD ["serve", "--config", "/data/aerini-server.json", "--bind", "0.0.0.0", "--port", "7700"]
"#
    .replace("@AERINI_REF@", &format!("v{}", env!("CARGO_PKG_VERSION")))
}

pub(super) fn build_serve_docker_compose(
    workflow_name: &str,
    status_port:   u16,
    creds:         &[CredentialExport],
    dangerous:     &[&str],
    webhook:       Option<&WebhookProxy>,
) -> String {
    let safe_name = safe_name(workflow_name);

    // The Caddy container joins the app's network namespace, so its ports are
    // published on the app service, and 127.0.0.1 in the Caddyfile is the app's own
    // loopback where the webhook listens.
    let (webhook_note, webhook_ports, caddy_service) = match webhook {
        None => (String::new(), String::new(), String::new()),
        Some(_) => (
            "#\n# This workflow starts on a webhook. The caddy service below publishes it over HTTPS:\n\
             # set AERINI_DOMAIN in .env to a name whose DNS points at this server (see README.md).\n"
                .to_string(),
            "      - \"80:80\"\n      - \"443:443\"\n      - \"443:443/udp\"\n".to_string(),
            format!(
                "\n  caddy:\n    image: caddy:2\n    network_mode: \"service:aerini-{safe_name}\"\n    \
                 environment:\n      AERINI_DOMAIN: ${{AERINI_DOMAIN:?Set AERINI_DOMAIN in .env}}\n    \
                 volumes:\n      - ./Caddyfile:/etc/caddy/Caddyfile:ro\n      - caddy_data:/data\n      - caddy_config:/config\n    \
                 restart: unless-stopped\n"
            ),
        ),
    };
    let caddy_volumes = if webhook.is_some() { "  caddy_data:\n  caddy_config:\n" } else { "" };

    // env_file injects all credential vars from .env directly into the container.
    // No need to list each var explicitly in environment: — that would require
    // the host shell to also have them set for docker-compose interpolation.
    let env_block = if creds.is_empty() {
        "    environment:\n      RUST_LOG: info\n".to_string()
    } else {
        "    env_file:\n      - .env\n    environment:\n      RUST_LOG: info\n".to_string()
    };

    // The command above never passes --allow-shell/--allow-code/--allow-database
    // (disabled by default, same reasoning as build_service_file). This hint is the
    // one place the exact flags this export needs can be shown, as a
    // ready-to-uncomment example; it still requires the operator to act.
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
             \x20   # To enable, audit the workflow, then replace the command above with:\n\
             \x20   # command: [\"serve\", \"--config\", \"/data/aerini-server.json\", \"--bind\", \"0.0.0.0\", \"--port\", \"7700\", {flags_json}]\n",
            types      = dangerous.join(", "),
            flags_json = flags_json,
        )
    };

    format!(
        r#"# Aerini Server — {safe_name}
# Generated by Aerini Desktop.
#
# Runs the official Aerini image, pinned to Aerini {ver}, the release that exported this package.
#
# Quick start:
#   1. cp .env.example .env && edit .env
#   2. docker compose up -d
#   3. Status page: http://localhost:{status_port}
{webhook_note}#
# The image is published for x86-64 (amd64) and only for released versions. On an
# ARM server, or if Docker reports that the image was not found, build from source:
#   docker compose -f docker-compose.yml -f docker-compose.build.yml up --build -d

services:
  aerini-{safe_name}:
    image: ghcr.io/panchak2d/aerini-server:{ver}
    command: ["serve", "--config", "/data/aerini-server.json", "--bind", "0.0.0.0", "--port", "7700"]
    ports:
      - "127.0.0.1:{status_port}:7700"
{webhook_ports}    volumes:
      - ./aerini-server.json:/data/aerini-server.json:ro
      - aerini_{safe_name}_data:/data
{env_block}{command_hint}    restart: unless-stopped
{caddy_service}
volumes:
  aerini_{safe_name}_data:
{caddy_volumes}"#,
        safe_name     = safe_name,
        status_port   = status_port,
        ver           = env!("CARGO_PKG_VERSION"),
        env_block     = env_block,
        command_hint  = command_hint,
        webhook_note  = webhook_note,
        webhook_ports = webhook_ports,
        caddy_service = caddy_service,
        caddy_volumes = caddy_volumes,
    )
}

pub(super) fn build_serve_docker_compose_build_override(workflow_name: &str) -> String {
    format!(
        r#"# Aerini Server — {safe_name}: build from source instead of pulling the image.
# Generated by Aerini Desktop. Use it together with docker-compose.yml:
#
#   docker compose -f docker-compose.yml -f docker-compose.build.yml up --build -d
#
# Use this on a server that is not x86-64, to run a fork or a branch, or when the
# image for this version has not been published. The first build compiles Aerini
# from source, so it needs internet access and takes a while.

services:
  aerini-{safe_name}:
    image: aerini-{safe_name}:local
    build:
      context: .
      dockerfile: Dockerfile.source
"#,
        safe_name = safe_name(workflow_name),
    )
}

pub(super) fn build_serve_caddyfile(workflow_name: &str, hook: &WebhookProxy) -> String {
    let method_line = if hook.method == "ANY" {
        String::new()
    } else {
        format!("\t\tmethod {}\n", hook.method)
    };
    format!(
        "# Aerini Server — {safe_name}: public HTTPS for the webhook.\n\
         # Generated by Aerini Desktop. This Caddy container shares the app container's\n\
         # network, so 127.0.0.1:{port} is the webhook listener inside the app.\n\
         # Only the webhook path and method are passed through; everything else answers 404.\n\
         \n\
         {{$AERINI_DOMAIN}} {{\n\
         \x20   @webhook {{\n\
         \x20       path {path}\n\
         {method_line}\
         \x20   }}\n\
         \x20   handle @webhook {{\n\
         \x20       reverse_proxy 127.0.0.1:{port}\n\
         \x20   }}\n\
         \x20   handle {{\n\
         \x20       respond 404\n\
         \x20   }}\n\
         }}\n",
        safe_name   = safe_name(workflow_name),
        port        = hook.port,
        path        = hook.path,
        method_line = method_line.replace('\t', "    "),
    )
}

pub(super) fn build_docker_env_example(
    creds:   &[CredentialExport],
    webhook: Option<&WebhookProxy>,
) -> String {
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
    if webhook.is_some() {
        s.push_str(
            "# Public domain for the Webhook (required). Its DNS record must point at this\n\
             # server; Caddy fetches the HTTPS certificate for it.\n\
             AERINI_DOMAIN=\n",
        );
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
    webhook:   Option<&WebhookProxy>,
) -> String {
    let raw_name = name;
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

    let safe = safe_name(raw_name);

    let webhook_section = match webhook {
        None => String::new(),
        Some(hook) => {
            let secret_text = if hook.has_secret {
                "Callers must send the secret from the Webhook node in the `x-webhook-secret` header. \
                 It is stored in `aerini-server.json`, not in `.env`."
            } else {
                "**This Webhook node has no secret, so anyone who finds the URL can run your workflow.** \
                 Set a secret in the node and export again before you open this to the internet."
            };
            let curl_method = if hook.method == "ANY" { "POST" } else { hook.method.as_str() };
            format!(
                "## Public HTTPS for the Webhook\n\
                 \n\
                 This workflow starts on a webhook at `{path}`. The webhook listens on the container's \
                 loopback address only, which Docker cannot publish, so a Caddy container in \
                 `docker-compose.yml` shares the app's network and serves it over HTTPS. Only that path \
                 and method are passed through; everything else answers 404, and the status page is not \
                 exposed this way.\n\
                 \n\
                 1. Point a DNS name at this server (an A or AAAA record).\n\
                 2. Set `AERINI_DOMAIN=<that name>` in `.env`.\n\
                 3. Open ports 80 and 443 (and 443/udp for HTTP/3) to the internet, in your cloud \
                 provider's firewall as well: Caddy needs them to get and renew the certificate.\n\
                 4. `docker compose up -d`\n\
                 \n\
                 {secret_text}\n\
                 \n\
                 ```bash\n\
                 curl -X {curl_method} https://<your-domain>{path} \\\n\
                 \x20 -H \"x-webhook-secret: <your-webhook-secret>\" \\\n\
                 \x20 -H \"content-type: application/json\" -d '{{\"hello\": \"world\"}}'\n\
                 ```\n\
                 \n",
                path        = hook.path,
                secret_text = secret_text,
                curl_method = curl_method,
            )
        }
    };

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
             To enable, audit every node in this workflow, then replace the \
             `command:` in `docker-compose.yml` with the commented-out example \
             beneath it, or add the flag(s) yourself: `{flags}`",
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
         # 2. Start (pulls the official image the first time)\n\
         docker compose up -d\n\
         \n\
         # 3. View status page\n\
         open http://localhost:{port}\n\
         \n\
         # 4. Follow logs\n\
         docker compose logs -f aerini-{safe}\n\
         ```\n\
         \n\
         {webhook_section}## Credentials\n\
         \n\
         {creds}\n\
         \n\
         Credentials are read from environment variables — they are never stored in\n\
         `aerini-server.json`. Set them in `.env` before starting the container.\n\
         \n\
         ## Requirements\n\
         \n\
         - Docker Engine 20.10+ with Compose V2 (`docker compose`)\n\
         - Internet access to pull the `ghcr.io/panchak2d/aerini-server:{ver}` image\n\
         - An x86-64 (amd64) host; for anything else, build from source (see below)\n\
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
         ## Building from Source\n\
         \n\
         `docker-compose.yml` runs the official image published for Aerini {ver}, the version that exported this package. To build from source instead (an ARM server, a fork, a branch, or a version whose image is not published), add the build override:\n\
         \n\
         ```bash\n\
         docker compose -f docker-compose.yml -f docker-compose.build.yml up --build -d\n\
         ```\n\
         \n\
         The override uses `Dockerfile.source`, which clones Aerini from the official repository at the release tag `v{ver}`. To build another release, a branch, or a fork:\n\
         \n\
         ```bash\n\
         docker compose -f docker-compose.yml -f docker-compose.build.yml build \\\n\
           --build-arg AERINI_REF=v{ver} \\\n\
           --build-arg AERINI_REPO=https://github.com/Panchak2d/aerini\n\
         ```\n\
         \n\
         ## File Permissions\n\
         \n\
         The container runs as an unprivileged user that is not you, so `aerini-server.json` must be readable by other users on the host. If `docker compose logs` shows a permission error for that file, run `chmod 644 aerini-server.json`. The server then warns that the file is readable by others; that is expected here, and the file holds only a hash of the run secret.\n\
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
        webhook_section = webhook_section,
        dangerous_section = dangerous_section,
    )
}

pub(super) fn build_readme(
    name: &str, trigger: &str, port: u16, creds: &[CredentialExport], dangerous: &[&str]
) -> String {
    let safe    = safe_name(name);
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
            safe  = safe,
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
         1. Upload this zip to your Linux server and unzip it (if unzip is missing: sudo apt install unzip).\n\
         2. aerini-server: install.sh downloads and checks the matching release for\n\
         \x20  x86-64 Linux if it is not in this folder. On another architecture, build it\n\
         \x20  from source (https://docs.aerini.org/operations/server-deploy#building-aerini-server-from-source)\n\
         \x20  and copy target/release/aerini-server here first.\n\
         3. {creds}\n\
         4. Run: chmod +x install.sh && ./install.sh\n\
         5. Status page: http://127.0.0.1:{port} on the server (listens locally only; use an SSH tunnel or a reverse proxy to reach it)\n\
         \n\
         REQUIREMENTS\n\
         ------------\n\
         - Linux x86_64 (Ubuntu 20.04+, Debian 11+, CentOS 7+, Alpine)\n\
         - systemd. install.sh tries to enable user lingering; if the system refuses, it prints the one command to run.\n\
         - curl, sha256sum and tar, only if install.sh has to download aerini-server.\n\
         - A Node.js binary, only if the workflow uses Code nodes and --allow-code is set.\n\
         \x20 Aerini does not search PATH: set AERINI_NODE_BIN=/path/to/node in .env, or put a\n\
         \x20 binary named node-bundled next to aerini-server.\n\
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
        safe    = safe,
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
        let docker_readme = build_docker_readme("wf", "Schedule", 7700, &[], &[], None);
        assert!(docker_readme.contains("nothing to enable"));
    }

    #[test]
    fn readme_and_docker_readme_surface_flags_when_dangerous_present() {
        let readme = build_readme("wf", "Schedule", 7700, &[], &["code"]);
        assert!(readme.contains("--allow-code"));
        assert!(readme.contains("DISABLED by default"));
        let docker_readme = build_docker_readme("wf", "Schedule", 7700, &[], &["code"], None);
        assert!(docker_readme.contains("--allow-code"));
        assert!(docker_readme.contains("DISABLED by default"));
    }

    #[test]
    fn readme_and_docker_readme_security_warning_reads_as_one_flowing_paragraph() {
        let readme = build_readme("wf", "Schedule", 7700, &[], &[]);
        assert!(!readme.contains("\\n\\"));
        assert!(readme.contains("run_secret.\nThe raw secret was shown once"));
        assert!(readme.contains("stored in this file.\nTreat this zip"));

        let docker_readme = build_docker_readme("wf", "Schedule", 7700, &[], &[], None);
        assert!(!docker_readme.contains("\\n\\"));
        assert!(docker_readme.contains("run_secret`.\nThe raw secret was shown once"));
        assert!(docker_readme.contains("stored in this file.\nTreat this zip"));
    }

    #[test]
    fn docker_compose_command_hint_only_present_when_dangerous() {
        let safe = build_serve_docker_compose("wf", 7700, &[], &[], None);
        assert!(!safe.contains("# command:"));
        let unsafe_ = build_serve_docker_compose("wf", 7700, &[], &["shell_exec"], None);
        assert!(unsafe_.contains("# command:"));
        assert!(unsafe_.contains("\"--allow-shell\""));
    }

    #[test]
    fn serve_dockerfile_bundles_node_next_to_the_binary() {
        let out = build_serve_dockerfile();
        // code_node.rs::resolve_node_bin_from only ever looks for `node-bundled`
        // next to its own executable, never PATH, so the exported image has to
        // put it exactly there for a Code (JS) node to work at all.
        assert!(out.contains("NODE_VERSION"));
        assert!(out.contains("sha256sum -c -"));
        assert!(out.contains(
            "COPY --from=builder /app/node-bundled /usr/local/bin/node-bundled"
        ));
    }

    #[test]
    fn serve_dockerfile_runtime_stage_can_run_the_bundled_node() {
        let out = build_serve_dockerfile();
        // Official nodejs.org Linux builds are dynamically linked against
        // glibc/libstdc++ — without this, node-bundled would be present but
        // fail to start in the debian:bookworm-slim runtime stage.
        assert!(out.contains("libstdc++6"));
    }

    #[test]
    fn serve_dockerfile_edits_cloned_manifest_instead_of_replacing_it() {
        let out = build_serve_dockerfile();
        // A replacement manifest has no [workspace.package], so every member
        // crate's `version.workspace`/`edition.workspace`/`license.workspace`
        // fails to resolve and the image never builds.
        assert!(!out.contains("> Cargo.toml"));
        assert!(!out.contains("printf '[workspace]"));
        let sed_pos = out
            .find("sed -i '/^[[:space:]]*\"src-tauri\",[[:space:]]*$/d' Cargo.toml")
            .expect("member-removal sed missing");
        let guard_pos = out
            .find("grep -q 'src-tauri' Cargo.toml")
            .expect("post-edit guard missing");
        let build_pos = out
            .find("cargo build --release")
            .expect("cargo build missing");
        assert!(sed_pos < guard_pos && guard_pos < build_pos);
    }

    #[test]
    fn serve_dockerfile_build_is_not_locked() {
        let out = build_serve_dockerfile();
        // Removing a member prunes Cargo.lock, which `--locked` rejects.
        let build_line = out
            .lines()
            .find(|l| l.trim_start().starts_with("cargo build"))
            .expect("cargo build line missing");
        assert!(!build_line.contains("--locked"));
        assert!(build_line.contains("-p aerini-server"));
    }
}

#[cfg(test)]
mod generated_name_tests {
    use super::*;

    #[test]
    fn safe_name_is_lowercase_ascii_with_single_hyphen_separators() {
        assert_eq!(safe_name("My Flow!"), "my-flow");
        assert_eq!(safe_name("a__b -- c"), "a-b-c");
        assert_eq!(safe_name("Daily Report 2"), "daily-report-2");
    }

    #[test]
    fn safe_name_never_ends_empty_or_with_a_separator() {
        assert_eq!(safe_name("***"), "workflow");
        assert_eq!(safe_name("\u{e9}\u{e8}"), "workflow");
        assert_eq!(safe_name(""), "workflow");
        let long = format!("{} tail", "a".repeat(SAFE_NAME_MAX - 1));
        let out = safe_name(&long);
        assert!(out.len() <= SAFE_NAME_MAX);
        assert!(!out.ends_with('-'));
    }

    #[test]
    fn compose_names_are_valid_for_a_mixed_case_name() {
        let compose = build_serve_docker_compose("My Flow!", 7700, &[], &[], None);
        assert!(compose.contains("aerini_my-flow_data:/data"));
        let build = build_serve_docker_compose_build_override("My Flow!");
        assert!(build.contains("image: aerini-my-flow:local"));
    }

    #[test]
    fn serve_compose_runs_the_pinned_image_on_a_fixed_container_port() {
        let out = build_serve_docker_compose("wf", 8080, &[], &[], None);
        assert!(out.contains(&format!(
            "image: ghcr.io/panchak2d/aerini-server:{}",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(!out.contains("build:"));
        // The server binds the port stored in aerini-server.json unless told otherwise,
        // so without --port a non-default export port would not match the mapping.
        assert!(out.contains("\"--port\", \"7700\"]"));
        assert!(out.contains("\"127.0.0.1:8080:7700\""));
    }

    fn webhook_trigger(port: u16, path: &str, method: &str, secret: &str) -> TriggerKind {
        TriggerKind::Webhook {
            port,
            path: path.to_string(),
            method: method.to_string(),
            secret: secret.to_string(),
            dedup_window_secs: 0,
        }
    }

    #[test]
    fn webhook_proxy_is_only_built_for_a_webhook_trigger_with_safe_values() {
        assert!(webhook_proxy_for(&TriggerKind::Interval { secs: 10 }).unwrap().is_none());
        let hook = webhook_proxy_for(&webhook_trigger(3456, "/hooks/in-1_v2.x", "POST", "s"))
            .unwrap()
            .unwrap();
        assert_eq!((hook.port, hook.has_secret), (3456, true));
        assert!(!webhook_proxy_for(&webhook_trigger(3456, "/webhook", "ANY", "")).unwrap().unwrap().has_secret);
        assert!(webhook_proxy_for(&webhook_trigger(7700, "/webhook", "ANY", "")).is_err());
        for bad_path in ["/a b", "/a{b}", "/a\"b", "/a*", "/a\nb", "webhook"] {
            assert!(webhook_proxy_for(&webhook_trigger(3456, bad_path, "ANY", "")).is_err(), "{bad_path}");
        }
        assert!(webhook_proxy_for(&webhook_trigger(3456, "/w", "po st", "")).is_err());
        assert!(webhook_proxy_for(&webhook_trigger(3456, "/w", "", "")).is_err());
    }

    #[test]
    fn webhook_export_adds_a_caddy_sidecar_in_the_apps_network() {
        let hook = webhook_proxy_for(&webhook_trigger(3456, "/hook", "POST", "s")).unwrap().unwrap();
        let out = build_serve_docker_compose("wf", 7700, &[], &[], Some(&hook));
        assert!(out.contains("network_mode: \"service:aerini-wf\""));
        assert!(out.contains("\"443:443/udp\""));
        assert!(out.contains("${AERINI_DOMAIN:?Set AERINI_DOMAIN in .env}"));
        assert!(out.contains("caddy_data:/data"));
        // Ports of a container that joins another's network belong to the app service.
        let caddy = &out[out.find("\n  caddy:").unwrap()..out.find("\nvolumes:").unwrap()];
        assert!(!caddy.contains("ports:"));
        let plain = build_serve_docker_compose("wf", 7700, &[], &[], None);
        assert!(!plain.contains("caddy") && !plain.contains("network_mode") && !plain.contains("80:80"));
        assert!(build_docker_env_example(&[], Some(&hook)).contains("\nAERINI_DOMAIN=\n"));
        assert!(!build_docker_env_example(&[], None).contains("AERINI_DOMAIN"));
    }

    #[test]
    fn caddyfile_passes_only_the_webhook_path_and_method_to_the_listener() {
        let hook = webhook_proxy_for(&webhook_trigger(3456, "/hook", "POST", "s")).unwrap().unwrap();
        let out = build_serve_caddyfile("wf", &hook);
        assert!(out.contains("{$AERINI_DOMAIN} {"));
        assert!(out.contains("path /hook\n"));
        assert!(out.contains("method POST\n"));
        assert!(out.contains("reverse_proxy 127.0.0.1:3456"));
        assert!(out.contains("respond 404"));
        let any = webhook_proxy_for(&webhook_trigger(3456, "/hook", "ANY", "s")).unwrap().unwrap();
        assert!(!build_serve_caddyfile("wf", &any).contains("\n        method "));
    }

    #[test]
    fn docker_readme_warns_when_a_public_webhook_has_no_secret() {
        let with = webhook_proxy_for(&webhook_trigger(3456, "/hook", "POST", "s")).unwrap().unwrap();
        let without = webhook_proxy_for(&webhook_trigger(3456, "/hook", "POST", "")).unwrap().unwrap();
        let a = build_docker_readme("wf", "Webhook", 7700, &[], &[], Some(&with));
        let b = build_docker_readme("wf", "Webhook", 7700, &[], &[], Some(&without));
        assert!(a.contains("## Public HTTPS for the Webhook") && a.contains("x-webhook-secret"));
        assert!(!a.contains("has no secret"));
        assert!(b.contains("has no secret, so anyone who finds the URL can run your workflow"));
        assert!(!build_docker_readme("wf", "Schedule", 7700, &[], &[], None).contains("Public HTTPS"));
    }

    #[test]
    fn build_override_builds_the_source_dockerfile() {
        let out = build_serve_docker_compose_build_override("wf");
        assert!(out.contains("build:"));
        assert!(out.contains("dockerfile: Dockerfile.source"));
    }

    #[test]
    fn source_dockerfile_command_pins_the_container_port() {
        let out = build_serve_dockerfile();
        assert!(out.contains("\"--bind\", \"0.0.0.0\", \"--port\", \"7700\"]"));
    }

    #[test]
    fn docker_readme_starts_from_the_image_and_names_the_build_override() {
        let out = build_docker_readme("wf", "Schedule", 7700, &[], &[], None);
        assert!(out.contains("# 2. Start (pulls the official image"));
        assert!(out.contains("-f docker-compose.yml -f docker-compose.build.yml up --build -d"));
        assert!(!out.contains("clones Aerini source"));
    }

    #[test]
    fn every_generated_file_uses_the_same_name() {
        let service = build_service_file("My Flow!", 7700, &[]);
        let script = build_install_sh("My Flow!", 7700, false);
        let readme = build_readme("My Flow!", "Schedule", 7700, &[], &[]);
        let docker_readme = build_docker_readme("My <b>Flow</b>", "Schedule", 7700, &[], &[], None);
        assert!(service.contains("Description=Aerini Workflow: my-flow"));
        assert!(script.contains("INSTALL_DIR=\"$HOME/.aerini-server/my-flow\""));
        assert!(readme.contains("journalctl --user -u aerini-my-flow -f"));
        assert!(docker_readme.contains("docker compose logs -f aerini-my-b-flow-b"));
    }

    #[test]
    fn dockerfile_defaults_to_the_exporting_release_tag() {
        let out = build_serve_dockerfile();
        assert!(out.contains(&format!("ARG AERINI_REF=v{}", env!("CARGO_PKG_VERSION"))));
        assert!(!out.contains("@AERINI_REF@"));
    }

    #[test]
    fn install_script_restricts_permissions_before_copying_files() {
        let out = build_install_sh("wf", 7700, false);
        let umask = out.find("umask 077").expect("umask line missing");
        let first_copy = out.find("cp aerini-server").expect("copy missing");
        assert!(umask < first_copy);
    }

    #[test]
    fn install_script_stops_a_running_service_before_replacing_the_binary() {
        let out = build_install_sh("wf", 7700, false);
        let stop = out.find("systemctl --user stop \"$SERVICE_NAME\"").expect("stop missing");
        let first_copy = out.find("cp aerini-server").expect("copy missing");
        assert!(stop < first_copy);
    }

    #[test]
    fn install_script_names_the_two_places_the_code_node_looks_for_node_only_when_it_has_one() {
        let with_code = build_install_sh("wf", 7700, true);
        assert!(!with_code.contains("command -v node"));
        assert!(with_code.contains("AERINI_NODE_BIN"));
        assert!(with_code.contains("$INSTALL_DIR/node-bundled"));

        let without_code = build_install_sh("wf", 7700, false);
        assert!(!without_code.contains("AERINI_NODE_BIN"));
        assert!(!without_code.contains("node-bundled"));
    }

    #[test]
    fn install_script_downloads_and_verifies_the_matching_release_when_the_binary_is_missing() {
        let out = build_install_sh("wf", 7700, false);
        let version = env!("CARGO_PKG_VERSION");
        assert!(out.contains(&format!("VERSION=\"{version}\"")));
        assert!(out.contains("https://github.com/Panchak2d/aerini/releases/download/v$VERSION"));
        assert!(out.contains("aerini-server_${VERSION}_${TARGET}.tar.gz"));
        assert!(out.contains("x86_64|amd64) TARGET=\"x86_64-unknown-linux-musl\""));
        assert!(out.contains("sha256sum -c"));
        let check = out.find("sha256sum -c").unwrap();
        let extract = out.find("tar -xzf").unwrap();
        let first_install = out.find("cp aerini-server").unwrap();
        assert!(check < extract && extract < first_install);
        assert!(out.contains("set -euo pipefail"));
    }

    #[test]
    fn install_script_prints_the_address_the_service_really_listens_on() {
        let out = build_install_sh("wf", 7700, false);
        assert!(!out.contains("hostname -I"));
        assert!(out.contains("http://127.0.0.1:7700"));
        assert!(out.contains("ssh -L 7700:127.0.0.1:7700"));
    }

    #[test]
    fn service_file_starts_the_serve_subcommand() {
        let out = build_service_file("wf", 7700, &[]);
        assert!(out.contains("/aerini-server serve --config {{INSTALL_DIR}}/aerini-server.json --port 7700"));
    }

    #[test]
    fn install_script_enables_linger_with_sudo_fallback_and_names_the_fix_when_it_stays_off() {
        let out = build_install_sh("wf", 7700, false);
        assert!(out.contains("sudo -n loginctl enable-linger"));
        assert!(out.contains("ACTION REQUIRED"));
        assert!(out.contains("  sudo loginctl enable-linger $USER"));
        assert!(!out.contains("| grep -q"));
    }

    #[test]
    fn install_script_replaces_the_placeholder_the_service_file_contains() {
        let service = build_service_file("wf", 7700, &[]);
        let script = build_install_sh("wf", 7700, false);
        assert!(service.contains("WorkingDirectory={{INSTALL_DIR}}"));
        assert!(script.contains("sed \"s|{{INSTALL_DIR}}|$INSTALL_DIR|g\" aerini.service"));
    }

    #[test]
    fn readme_tells_the_user_to_add_the_binary_and_how_code_nodes_find_node() {
        let readme = build_readme("wf", "Schedule", 7700, &[], &[]);
        let quick_start = readme.find("QUICK START").expect("quick start missing");
        let binary_step = readme.find("2. aerini-server: install.sh downloads").expect("binary step missing");
        let run_step = readme.find("./install.sh").expect("run step missing");
        assert!(quick_start < binary_step && binary_step < run_step);
        assert!(readme.contains("AERINI_NODE_BIN=/path/to/node"));
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
        let out = build_install_sh("wf", 7700, false);
        assert!(out.contains(r#"chmod 600 "$INSTALL_DIR/.env""#));
    }
}
