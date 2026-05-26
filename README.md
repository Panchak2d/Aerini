# Flowo Documentation

Flowo is a visual workflow automation desktop app for macOS, Windows, and Linux. You drag nodes onto a canvas, wire them together, and run the resulting workflow — either manually, on a cron schedule, or triggered by an incoming webhook.

No account, no cloud, no subscription. Everything runs locally. Workflows and credentials are stored on your machine in SQLite, credentials encrypted with AES-256-GCM.

---

## What you can build

Some examples of what people use Flowo for:

- **Daily reports** — fetch data from an API at 9 AM on weekdays, transform it with a Code node, email the result via SendGrid or SMTP.
- **Slack/Discord alerts** — poll an endpoint every 5 minutes, run a condition check, post to a channel if something's wrong.
- **AI pipelines** — chain an AI Prompt node into a Transform node into a Notion page create. Or use the AI Agent node to reason over structured data and take action on the result.
- **Webhook receivers** — listen for a GitHub webhook, run a shell script, post back a Slack notification.
- **File processing** — watch for new files, read them, transform the content, write output somewhere else.

Flowo isn't trying to replace n8n or Zapier for large teams — it's a local-first tool for individuals and small setups. If you need to deploy a workflow that runs 24/7 on a server, `flowo-server` covers that without needing the desktop app on the server.

---

## Node categories

Flowo ships 34 built-in nodes across six categories:

**Triggers** — Manual Trigger, Schedule (interval / cron / once), Webhook

**Logic** — If / Condition, Switch, Loop, Merge, Stop

**Flow control** — Delay, Wait

**AI** — AI Prompt, AI Agent (ReAct loop), AI Memory, Text Splitter

**Actions** — HTTP Request, Shell Command, Code (JS), Send Email, SendGrid, File, Desktop Notification, Database (SQLite)

**Integrations** — Slack, Discord, GitHub, Google Sheets, Notion, Telegram, Stripe

**Data & Utility** — Transform Data, JSON, Set Variable, Get Variable, Output

Full parameter docs, output schemas, and constraints for every node: [Nodes Reference](docs/nodes.md).

---

## Quick orientation

The left sidebar has three sections:

- **Node Panel** — the search bar at the very top of the sidebar. Type any node name and click to place it on the canvas. You can also press `Space` or `Ctrl+K` to open node search from anywhere.
- **Workflows** — your saved workflows, sorted by last modified. Click one to load it onto the canvas.
- **Background Runs** — workflows currently executing on a schedule or waiting for an incoming webhook.

Connect nodes by dragging from an output port to an input port. Press `Ctrl+Enter` to run the current workflow.

After a run, the output drawer opens at the bottom with tabs for Summary, Results, Errors, Logs, Debug, and History.

---

## Deploying to a server

The desktop app includes an **Export for Server** feature. Open any workflow with a Schedule or Webhook trigger, click **File → Export for Server**, and Flowo generates a zip containing the `flowo-server` binary, a config file, a `.env.example` listing the credentials you need to set, a systemd service file, and an `install.sh` that handles everything. Upload the zip to a Linux server and run `./install.sh`.

For managing many workflows on one server, `flowo-server api` mode exposes a REST API and a CLI. See [Server Deployment](docs/server-deploy.md).

---

## Guides

| Guide | What it covers |
|---|---|
| [Getting Started](docs/getting-started.md) | Install, build your first workflow, read run results, schedule it |
| [Nodes Reference](docs/nodes.md) | All 34 nodes — parameters, outputs, credentials, constraints |
| [Expressions](docs/expressions.md) | `{{...}}` template syntax, `$run.*`, `$env.*`, inline functions |
| [Credentials](docs/credentials.md) | Add API keys, how encryption works, how to get keys for every supported service |
| [Security](docs/security.md) | Full security model — encryption, dangerous nodes, SSRF, prompt injection, server hardening |
| [Background Runs](docs/background-runs.md) | Scheduled and webhook-triggered workflows, system tray, n8n import |
| [Server Deployment](docs/server-deploy.md) | Run workflows 24/7 on Linux — serve mode, API mode, Docker, HTTPS |
| [Architecture](docs/architecture.md) | How the engine, Tauri shell, and server binary fit together |

---

## Requirements

| Tool | Version |
|---|---|
| Rust (stable) | 1.77+ |
| Node.js | 18+ |
| Tauri CLI | 2.x |

Node.js must be on PATH at runtime — the Code (JS) node spawns it as a subprocess.

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the dev environment setup, how to add a new node, code conventions, and the protected interfaces you must not change.

These docs cover Flowo `0.2.0` (engine) and `flowo-server` `0.1.0`. The project is pre-1.0 — rough edges exist and are noted throughout the guides where relevant.

---

## License

Flowo is licensed under the [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0).

**What this means:**

- You can use, modify, and distribute Flowo freely.
- If you modify Flowo and run it as a **network-accessible service** (including `flowo-server`), you must release your modifications under AGPL-3.0. This applies to SaaS, hosted services, and any deployment where users interact with the software over a network.
- If you redistribute Flowo (binary or source), you must include the license and make source available.

For projects where AGPL-3.0 is incompatible with your license, contact the maintainers to discuss alternatives.

All bundled dependencies are MIT or Apache-2.0 licensed and are compatible with AGPL-3.0 distribution.
