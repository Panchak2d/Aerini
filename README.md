# Flowo

Flowo is a visual workflow automation desktop app for macOS, Windows, and Linux. Drag nodes onto a canvas, connect them together, and automate tasks — run them manually, on a schedule, or when an HTTP request arrives.

No account. No cloud. No subscription. Everything runs on your machine.

---

## What you can build

- **Scheduled reports** — fetch data from an API every morning, transform it, email or Slack the result
- **Alerts** — poll an endpoint on a timer, post to a channel when something looks wrong
- **AI pipelines** — chain an AI Prompt into a Transform into a Notion page create
- **Webhook receivers** — accept a GitHub or Stripe webhook, run logic, send a notification
- **File processing** — read files, transform their content, write results elsewhere
- **Social media uploads** — generate images with AI and post them to YouTube, Instagram, or TikTok

Flowo is a local-first tool for individuals and small setups. For workflows that need to run 24/7 on a server, `flowo-server` handles that without requiring the desktop app on the server.

---

## Guides

| Guide | What it covers |
|---|---|
| [Getting Started](docs/getting-started.md) | Install, build your first workflow, run it, schedule it |
| [Concepts](docs/concepts.md) | What a workflow is, what nodes are, how everything fits together — start here if you're new |
| [Nodes Reference](docs/nodes.md) | All 39 nodes — parameters, outputs, and what each one does |
| [Expressions](docs/expressions.md) | `{{...}}` syntax for wiring node outputs into other nodes |
| [Credentials](docs/credentials.md) | Storing API keys securely and getting them from every supported service |
| [Background Runs](docs/background-runs.md) | Schedules, webhook triggers, run history |
| [Server Deployment](docs/server-deploy.md) | Running workflows 24/7 on a Linux server |
| [Security](docs/security.md) | Encryption, dangerous nodes, SSRF protection, server hardening |

Developer guides:

| Guide | What it covers |
|---|---|
| [Architecture](docs/architecture.md) | How the engine, Tauri shell, and server binary fit together |
| [Custom Node Authoring](docs/node-authoring.md) | Adding new node types to Flowo |
| [Schema Migrations](docs/schema-migrations.md) | How workflow format changes are handled across versions |

---

## Node categories

Flowo ships 39 built-in nodes:

**Triggers** — Manual Trigger, Schedule, Webhook

**Logic** — If / Condition, Switch, Loop (For Each), Merge, Stop, Collect Files

**Flow Control** — Delay, Wait

**AI** — AI Prompt, AI Agent, AI Memory, Text Splitter, Image Generation

**Actions** — HTTP Request, Shell Command, Code (JS), Send Email, SendGrid, File, Desktop Notification, Save to Folder, Social Upload, Database, S3 Storage

**Integrations** — Slack, Discord, GitHub, Google Sheets, Notion, Telegram, Stripe

**Data & Utility** — Transform Data, JSON, Set Variable, Get Variable, Output

---

## Requirements

| Tool | Version | Why |
|---|---|---|
| Rust (stable) | 1.77+ | Builds the engine and desktop shell |
| Node.js | 18+ | Required at runtime for the Code (JS) node |
| Tauri CLI | 2.x | Packages the desktop app |

Node.js must be on your PATH — not just installed, but reachable as the `node` command in a terminal. The Code (JS) node spawns it as a subprocess at runtime.

---

## Installing

```bash
git clone https://github.com/Panchak2d/flowo
cd flowo
npm install
npm run dev
```

The first build takes 2–5 minutes while Rust compiles. After that, changes rebuild in seconds.

To produce a standalone installer: `npm run build`. The output goes to `src-tauri/target/release/bundle/`.

> `dist/` is intentionally committed — Tauri reads frontend assets from it at build time. Do not add it to `.gitignore`. See [CONTRIBUTING.md](CONTRIBUTING.md).

---

## License

[GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0).

You can use, modify, and distribute Flowo freely. If you modify Flowo and run it as a network-accessible service, you must release your modifications under AGPL-3.0.

---

## Commercial licensing

Flowo is open source under AGPL-3.0. A commercial license is available for proprietary use — no AGPL obligations, priority support included.

[View pricing →](https://panchak2d.github.io/flowo/pricing)
