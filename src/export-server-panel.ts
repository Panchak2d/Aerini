/// Export for Server panel — generates a deployment package zip.
///
/// Flow:
///   1. User opens panel (must have a workflow open on canvas)
///   2. Panel calls validate_workflow_for_export — shows trigger + credential table
///   3. User picks deployment type (Linux Server / Docker), sets port, clicks Generate Package
///   4. Panel calls generate_server_package or generate_docker_package — saves zip via dialog
///   5. Linux: user runs ./install.sh   Docker: user runs docker compose up --build -d

import { invoke } from "@tauri-apps/api/core";
import { escapeHtml } from "./utils";

interface CredentialExport {
  credential_id: string;
  env_var_name:  string;
  node_name:     string;
}

interface CredentialEntry {
  id:   string;
  name: string;
}

interface ValidateResult {
  trigger_desc: string;
  credentials:  CredentialExport[];
  variables:    string[];
}

interface ExportResult {
  zip_path:             string;
  workflow_name:        string;
  credentials:         CredentialExport[];
  trigger_desc:        string;
  run_secret_plaintext: string;
}

type DeployTarget = "linux" | "docker";

const CLOSE_SVG = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;

export function showExportServerPanel(
  workflowId: string | null,
  toast: (msg: string, level?: "info" | "error" | "success") => void
): void {
  const existing = document.getElementById("export-server-panel-overlay");
  if (existing) { existing.remove(); }

  const outputDrawer = document.getElementById("output-drawer");
  if (outputDrawer && !outputDrawer.classList.contains("hidden")) {
    outputDrawer.classList.add("hidden");
  }

  const overlay = document.createElement("div");
  overlay.id = "export-server-panel-overlay";
  overlay.className = "panel-overlay";

  const panel = document.createElement("div");
  panel.className = "panel-drawer export-server-panel";
  panel.setAttribute("role", "dialog");
  panel.setAttribute("aria-modal", "true");
  panel.setAttribute("aria-label", "Export for Server");

  overlay.appendChild(panel);
  document.body.appendChild(overlay);

  const closePanel = () => {
    document.removeEventListener("keydown", onEsc, true);
    overlay.remove();
  };
  const onEsc = (e: KeyboardEvent) => { if (e.key === "Escape") closePanel(); };
  document.addEventListener("keydown", onEsc, true);
  overlay.addEventListener("click", (e) => { if (e.target === overlay) closePanel(); });

  if (!workflowId) {
    panel.innerHTML = `
      <div class="panel-header">
        <h2 class="panel-title">Export for Server</h2>
        <button class="panel-close-btn" aria-label="Close">${CLOSE_SVG}</button>
      </div>
      <div class="panel-body esp-body">
        <div class="esp-callout-warn">
          No workflow is open. Open a workflow on the canvas first, then export it.
        </div>
      </div>`;
    panel.querySelector(".panel-close-btn")?.addEventListener("click", closePanel);
    return;
  }

  panel.innerHTML = `
    <div class="panel-header">
      <h2 class="panel-title">Export for Server</h2>
      <button class="panel-close-btn" aria-label="Close">${CLOSE_SVG}</button>
    </div>
    <div class="panel-body esp-body">
      <div id="esp-loading" class="esp-loading">Checking workflow…</div>
      <div id="esp-content" hidden></div>
    </div>`;

  panel.querySelector(".panel-close-btn")?.addEventListener("click", closePanel);

  Promise.all([
    invoke<ValidateResult>("validate_workflow_for_export", { workflowId }),
    invoke<CredentialEntry[]>("list_credentials").catch(() => [] as CredentialEntry[]),
  ])
    .then(([result, storedCreds]) => {
      const filledIds = new Set(storedCreds.map(c => c.id));
      renderReady(panel, workflowId, result, filledIds, toast);
    })
    .catch(err => renderError(panel, String(err)));
}

function renderError(panel: HTMLElement, message: string): void {
  const loading = panel.querySelector<HTMLElement>("#esp-loading");
  const content = panel.querySelector<HTMLElement>("#esp-content");
  if (loading) loading.hidden = true;
  if (content) {
    content.hidden = false;
    content.innerHTML = `<div class="esp-callout-error">${escapeHtml(message)}</div>`;
  }
}

function renderReady(
  panel:      HTMLElement,
  workflowId: string,
  result:     ValidateResult,
  filledIds:  Set<string>,
  toast:      (msg: string, level?: "info" | "error" | "success") => void
): void {
  const loading = panel.querySelector<HTMLElement>("#esp-loading");
  const content = panel.querySelector<HTMLElement>("#esp-content");
  if (loading) loading.hidden = true;
  if (!content) return;
  content.hidden = false;

  const anyEmpty = result.credentials.some(c => !filledIds.has(c.credential_id));

  const credRows = result.credentials.length === 0
    ? `<p class="esp-note">This workflow uses no credentials.</p>`
    : `${anyEmpty ? `<p class="esp-cred-warning">⚠ Fill the credentials marked below before deploying.</p>` : ""}
      <table class="esp-cred-table">
        <thead><tr><th>Status</th><th>Credential</th><th>Environment variable</th><th>Used by</th></tr></thead>
        <tbody>
          ${result.credentials.map(c => {
            const filled   = filledIds.has(c.credential_id);
            const statusEl = filled
              ? `<span class="esp-cred-ok"  title="Credential is filled">✓</span>`
              : `<span class="esp-cred-warn" title="Credential is empty">⚠</span>`;
            return `<tr class="${filled ? "" : "esp-cred-row--empty"}">
              <td class="esp-cred-status">${statusEl}</td>
              <td>${escapeHtml(c.credential_id)}</td>
              <td><code>${escapeHtml(c.env_var_name)}</code></td>
              <td class="esp-muted">${escapeHtml(c.node_name)}</td>
            </tr>`;
          }).join("")}
        </tbody>
      </table>
      <p class="esp-note">
        You will set these as environment variables on your server.
        They are never stored in the package — only their names are listed.
      </p>`;

  const varsSection = result.variables.length === 0 ? "" : `
    <table class="esp-cred-table">
      <thead><tr><th>Variable</th><th>Suggested env key</th></tr></thead>
      <tbody>
        ${result.variables.map(v => `
          <tr>
            <td><code>$vars.${escapeHtml(v)}</code></td>
            <td><code>AERINI_VAR_${escapeHtml(v.toUpperCase())}</code></td>
          </tr>
        `).join("")}
      </tbody>
    </table>
    <p class="esp-note">
      These are referenced via <code>{{$vars.x}}</code> in your workflow.
      Set them as environment variables on your server.
    </p>`;

  content.innerHTML = `
    <p class="esp-intro">
      Generates a self-contained deployment package for your workflow.
      Choose your target environment below.
    </p>

    <div class="esp-field-row">
      <div class="esp-field">
        <span class="esp-field-label">Trigger</span>
        <span class="esp-field-value">${escapeHtml(result.trigger_desc)}</span>
      </div>
    </div>

    ${result.variables.length > 0 ? `
    <h3 class="esp-section-title">Required variables</h3>
    ${varsSection}` : ""}

    <div class="esp-tab-bar" role="tablist" aria-label="Deployment target">
      <button id="esp-tab-linux" class="esp-tab esp-tab--active" role="tab"
              aria-selected="true"  aria-controls="esp-pane-linux">
        🖥 Linux Server
      </button>
      <button id="esp-tab-docker" class="esp-tab" role="tab"
              aria-selected="false" aria-controls="esp-pane-docker">
        🐳 Docker
      </button>
    </div>

    <div id="esp-pane-linux" role="tabpanel" aria-labelledby="esp-tab-linux">
      <p class="esp-note esp-pane-desc">
        Generates a zip with a self-installing systemd service.
        Unzip it on any Linux server and run <code>./install.sh</code>.
        No Rust required on the server.
      </p>

      <h3 class="esp-section-title">Credentials</h3>
      ${credRows}

      <h3 class="esp-section-title">Status page port</h3>
      <p class="esp-note">
        The server will serve a live status page on this port.
        Make sure it's open in your server's firewall.
      </p>
      <div class="esp-port-action-row">
        <input id="esp-port-linux" class="esp-input esp-port-input" type="number"
               min="1024" max="65535" value="7700" />
        <div class="esp-generate-wrap">
          <button id="esp-generate-linux-btn" class="esp-primary-btn">
            ⬇ Generate Linux Package
          </button>
          <span id="esp-generating-linux" hidden class="esp-generating-label">
            <span class="esp-spinner"></span>Building package…
          </span>
        </div>
      </div>

      <div id="esp-success-linux" hidden class="esp-success-block">
        <div class="esp-callout esp-mt">
          <strong>Package saved.</strong><br>
          Upload it to your server and run:<br>
          <code>unzip aerini-server-*.zip &amp;&amp; chmod +x install.sh &amp;&amp; ./install.sh</code>
        </div>
      </div>
    </div>

    <div id="esp-pane-docker" role="tabpanel" aria-labelledby="esp-tab-docker" hidden>
      <p class="esp-note esp-pane-desc">
        Generates a zip with a <code>Dockerfile</code> and <code>docker-compose.yml</code>
        for single-workflow serve mode.
        Run <code>docker compose up --build -d</code> — no Rust required on the host.
      </p>

      <h3 class="esp-section-title">Credentials</h3>
      ${credRows}

      <h3 class="esp-section-title">Status page port (host)</h3>
      <p class="esp-note">
        The status page will be mapped to this port on the host machine.
        The container always runs on port 7700 internally.
      </p>
      <div class="esp-port-action-row">
        <input id="esp-port-docker" class="esp-input esp-port-input" type="number"
               min="1024" max="65535" value="7700" />
        <div class="esp-generate-wrap">
          <button id="esp-generate-docker-btn" class="esp-primary-btn">
            ⬇ Generate Docker Package
          </button>
          <span id="esp-generating-docker" hidden class="esp-generating-label">
            <span class="esp-spinner"></span>Building package…
          </span>
        </div>
      </div>

      <div id="esp-success-docker" hidden class="esp-success-block">
        <div class="esp-callout esp-mt">
          <strong>Package saved.</strong><br>
          Extract the zip, fill in <code>.env</code>, then run:<br>
          <code>docker compose up --build -d</code>
        </div>
      </div>
    </div>`;

  // Tab switching
  const tabLinux   = content.querySelector<HTMLButtonElement>("#esp-tab-linux");
  const tabDocker  = content.querySelector<HTMLButtonElement>("#esp-tab-docker");
  const paneLinux  = content.querySelector<HTMLElement>("#esp-pane-linux");
  const paneDocker = content.querySelector<HTMLElement>("#esp-pane-docker");

  const activateTab = (target: DeployTarget) => {
    const isLinux = target === "linux";
    tabLinux?.classList.toggle("esp-tab--active", isLinux);
    tabDocker?.classList.toggle("esp-tab--active", !isLinux);
    tabLinux?.setAttribute("aria-selected", String(isLinux));
    tabDocker?.setAttribute("aria-selected", String(!isLinux));
    if (paneLinux)  paneLinux.hidden  = !isLinux;
    if (paneDocker) paneDocker.hidden = isLinux;
  };

  tabLinux?.addEventListener("click",  () => activateTab("linux"));
  tabDocker?.addEventListener("click", () => activateTab("docker"));

  // Linux generate
  const generateLinuxBtn  = content.querySelector<HTMLButtonElement>("#esp-generate-linux-btn");
  const generatingLinuxEl = content.querySelector<HTMLElement>("#esp-generating-linux");
  const successLinuxEl    = content.querySelector<HTMLElement>("#esp-success-linux");
  const portLinuxEl       = content.querySelector<HTMLInputElement>("#esp-port-linux");

  generateLinuxBtn?.addEventListener("click", async () => {
    const port = parseInt(portLinuxEl?.value ?? "7700", 10);
    if (isNaN(port) || port < 1024 || port > 65535) {
      toast("Port must be between 1024 and 65535", "error");
      return;
    }

    generateLinuxBtn.disabled = true;
    if (generatingLinuxEl) generatingLinuxEl.hidden = false;

    try {
      const exportResult = await invoke<ExportResult>("generate_server_package", {
        request: { workflow_id: workflowId, status_port: port }
      });

      const filename = `aerini-server-${exportResult.workflow_name.replace(/[^a-z0-9-]/gi, "_")}.zip`;
      const savedPath = await invoke<string>("save_export_zip", {
        zipPath: exportResult.zip_path,
        filename,
      }).catch((err: string) => {
        if (err !== "cancelled") throw new Error(err);
        return null;
      });

      if (savedPath && successLinuxEl) successLinuxEl.hidden = false;
      if (savedPath) {
        toast("Linux server package saved", "success");
        showRunSecretBanner(content, exportResult.run_secret_plaintext);
      }
    } catch (err) {
      toast(`Export failed: ${String(err)}`, "error");
    } finally {
      generateLinuxBtn.disabled = false;
      if (generatingLinuxEl) generatingLinuxEl.hidden = true;
    }
  });

  // Docker generate
  const generateDockerBtn  = content.querySelector<HTMLButtonElement>("#esp-generate-docker-btn");
  const generatingDockerEl = content.querySelector<HTMLElement>("#esp-generating-docker");
  const successDockerEl    = content.querySelector<HTMLElement>("#esp-success-docker");
  const portDockerEl       = content.querySelector<HTMLInputElement>("#esp-port-docker");

  generateDockerBtn?.addEventListener("click", async () => {
    const port = parseInt(portDockerEl?.value ?? "7700", 10);
    if (isNaN(port) || port < 1024 || port > 65535) {
      toast("Port must be between 1024 and 65535", "error");
      return;
    }

    generateDockerBtn.disabled = true;
    if (generatingDockerEl) generatingDockerEl.hidden = false;

    try {
      const exportResult = await invoke<ExportResult>("generate_docker_package", {
        request: { workflow_id: workflowId, status_port: port }
      });

      const filename = `aerini-docker-${exportResult.workflow_name.replace(/[^a-z0-9-]/gi, "_")}.zip`;
      const savedPath = await invoke<string>("save_export_zip", {
        zipPath: exportResult.zip_path,
        filename,
      }).catch((err: string) => {
        if (err !== "cancelled") throw new Error(err);
        return null;
      });

      if (savedPath && successDockerEl) successDockerEl.hidden = false;
      if (savedPath) {
        toast("Docker package saved", "success");
        showRunSecretBanner(content, exportResult.run_secret_plaintext);
      }
    } catch (err) {
      toast(`Export failed: ${String(err)}`, "error");
    } finally {
      generateDockerBtn.disabled = false;
      if (generatingDockerEl) generatingDockerEl.hidden = true;
    }
  });
}

/// Renders a one-time run-secret banner inside the export panel.
/// The raw secret is never stored on disk — this is the only opportunity
/// to record it. The banner includes a copy button and a clear warning.
function showRunSecretBanner(container: Element | null, secret: string): void {
  if (!container) return;
  // Remove any existing banner from a prior export in this panel session.
  container.querySelector(".esp-run-secret-banner")?.remove();

  const banner = document.createElement("div");
  banner.className = "esp-run-secret-banner";
  banner.innerHTML = `
    <div class="esp-run-secret-title">⚠ Save your run secret — shown once</div>
    <div class="esp-run-secret-desc">
      This secret authenticates <code>POST /api/run</code> and <code>GET /api/logs</code>.
      It is <strong>not stored in the zip</strong> — only its hash is. Copy it now.
    </div>
    <div class="esp-run-secret-row">
      <code class="esp-run-secret-value">${secret}</code>
      <button class="esp-run-secret-copy" type="button">Copy</button>
    </div>`;

  const copyBtn = banner.querySelector<HTMLButtonElement>(".esp-run-secret-copy")!;
  copyBtn.addEventListener("click", () => {
    navigator.clipboard.writeText(secret).then(() => {
      copyBtn.textContent = "Copied!";
      setTimeout(() => { copyBtn.textContent = "Copy"; }, 2000);
    }).catch(() => {
      copyBtn.textContent = "Copy failed";
      setTimeout(() => { copyBtn.textContent = "Copy"; }, 2000);
    });
  });

  container.appendChild(banner);
}
