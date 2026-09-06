# Aerini

Aerini is a visual workflow automation desktop app for macOS, Windows, and Linux. Drag nodes onto a canvas, connect them together, and automate tasks: run them manually, on a schedule, or trigger them via HTTP request.

No account. No cloud. No subscription. Everything runs on your machine.

---

## What you can build

- **Scheduled reports:** fetch data from an API every morning, transform it, email or Slack the result
- **Alerts:** poll an endpoint on a timer, post to a channel when something looks wrong
- **AI pipelines:** chain an AI Prompt into a Transform into a Notion page create
- **Webhook receivers:** accept a GitHub or Stripe webhook, run logic, send a notification
- **File processing:** read files, transform their content, write results elsewhere
- **Social media uploads:** generate images with AI and post them to YouTube, Instagram, or TikTok

Aerini is a local-first tool for individuals and small setups. For workflows that need to run 24/7 on a server, `aerini-server` handles that without requiring the desktop app on the server.

---

## Guides

| Guide | What it covers |
|---|---|
| [Introduction](docs/index.md) | What Aerini is, why use it, what it can and can't do |
| [Getting Started](docs/getting-started/getting-started.md) | Install, build your first workflow, run it, schedule it |
| [Concepts](docs/getting-started/concepts.md) | What a workflow is, what nodes are, how everything fits together |
| [Glossary](docs/glossary.md) | Plain-language definitions of every term used across these docs |
| [Nodes Reference](docs/guide/nodes.md) | Every built-in node: parameters, outputs, and what each one does |
| [Expressions](docs/guide/expressions.md) | `{{...}}` syntax for wiring node outputs into other nodes |
| [Credentials](docs/guide/credentials.md) | Storing API keys securely and getting them from every supported service |
| [Background Runs](docs/guide/background-runs.md) | Schedules, webhook triggers, run history |
| [Examples](docs/guide/examples.md) | Index of worked example workflows in [`examples/`](examples/) |
| [Troubleshooting](docs/troubleshooting.md) | Common problems, organized by symptom |
| [FAQ](docs/faq.md) | Short answers to common questions |
| [Security](docs/guide/security.md) | Encryption, dangerous nodes, SSRF protection, server hardening |
| [Widget Embedding](docs/guide/widget-embedding.md) | Dropping the chat widget into a web page, token scoping, security tradeoffs |
| [Local Models](docs/guide/local-models.md) | Using Ollama and other OpenAI-compatible local servers with AI Prompt |
| [Server Deployment](docs/operations/server-deploy.md) | Running workflows 24/7 on a Linux server |
| [Server CLI Reference](docs/operations/server-cli-reference.md) | Every `aerini-server serve`/`api` flag |
| [Server API Reference](docs/operations/server-api-reference.md) | REST endpoints, SSE events, and scoped tokens exposed by `aerini-server --api` |
| [Updating](docs/operations/updating.md) | Manual desktop update checks, server binary updates, workflow schema migration |

Developer guides:

| Guide | What it covers |
|---|---|
| [Architecture](docs/development/architecture.md) | How the engine, Tauri shell, and server binary fit together |
| [Embedding aerini-engine](docs/development/embedding.md) | Use the engine as a Rust library in your own program |
| [Custom Node Authoring](docs/development/node-authoring.md) | Adding new built-in node types to Aerini |
| [Plugin Authoring](docs/development/plugin-authoring.md) | Writing and distributing `.wasm` plugin nodes |
| [Desktop IPC Reference](docs/development/desktop-ipc-reference.md) | Every command the frontend can call into the Tauri shell |
| [Workflow File Format](docs/reference/workflow-file-format.md) | The `.aerini`/`.json` workflow file schema |
| [Schema Migrations](docs/reference/schema-migrations.md) | How workflow format changes are handled across versions |
| [Testing](docs/development/testing.md) | Where tests live and how to run them |

---

## Node categories

Aerini ships 40 built-in nodes:

**Triggers:** Manual Trigger, Schedule, Webhook

**Logic:** If / Condition, Switch, Loop (For Each), Merge, Stop, Collect Files

**Flow Control:** Delay, Wait

**AI:** AI Prompt, AI Agent, AI Memory, Text Splitter, Image Generation

**Actions:** HTTP Request, Shell Command, Code (JS), Send Email, SendGrid, File, Desktop Notification, Save to Folder, Social Upload, Database, S3 Storage

**Integrations:** Slack, Discord, GitHub, Google Sheets, Notion, Telegram, Stripe

**Data & Utility:** Transform Data, JSON, Set Variable, Get Variable, Output, Text to File

---

## Plugins

Aerini supports `.wasm` plugin nodes. Write a new node type in Rust, compile it to `wasm32-wasip2`, and install it through **Settings → Plugins** — pick a plugin folder, then install via the file picker or just drag the `.wasm` file onto the window. Plugin nodes appear in the palette automatically (marked with a small "P" badge so you can tell them apart from built-ins) and execute with the same isolation guarantees as built-in nodes — each call runs in a sandboxed Wasmtime instance with a 64 MiB memory limit and outbound HTTP access but no filesystem access.

The Settings panel also lists installed plugins and surfaces any that failed to load (with the reason) instead of just dropping them, so a bad build doesn't disappear silently.

See [Plugin Authoring](docs/development/plugin-authoring.md) to get started, or copy `examples/plugin-template/` as a starting point.

---

## Embedding the engine

`aerini-engine` is a standalone Rust crate — the workflow model, executor, scheduler, and node registry don't depend on Tauri or any UI. You can pull it into your own Rust program, register your own nodes alongside (or instead of) the 40 built-ins, supply your own credential resolver and event sink, and run workflows headless — no desktop app, no database required unless you want run history.

This is the same engine that powers the desktop app and `aerini-server`, so anything documented for those (retries, parallel execution, expressions, plugins) works the same way when embedded.

See [Embedding aerini-engine](docs/development/embedding.md) for a complete working example, the stable API surface, and what's safe to depend on across versions.

---

## Requirements

| Tool | Version | Why |
|---|---|---|
| Rust (stable) | 1.77+ | Builds the engine and desktop shell |
| Node.js | 18+ | Only to run `npm`/Vite while building from source — not needed to use the app |
| Tauri CLI | 2.x | Packages the desktop app |

The Code (JS) node runs on a Node.js runtime bundled with Aerini itself. You don't need Node.js installed to use that node — only to build Aerini from source (above).

---

## Installing

**Most people want this.** Download the installer for your OS from the [Releases page](https://github.com/Panchak2d/aerini/releases) — `.dmg` (macOS), `.msi`/`.exe` (Windows), or `.deb`/`.AppImage` (Linux). No terminal, no Rust, no Node.js required. Run it like any other desktop app. See [Getting Started](docs/getting-started/getting-started.md) for what to do next.

### Building from source (contributors / unsupported platforms)

```bash
git clone https://github.com/Panchak2d/aerini
cd aerini
npm install
./scripts/fetch-node-binaries.sh
npm run dev
```

`fetch-node-binaries.sh` downloads and checksum-verifies the Node.js runtime that gets bundled into Aerini (used by the Code (JS) node) and stages it under `src-tauri/binaries/`. Required once per clone — the build fails without it. Needs `curl`, `tar`, `unzip` (or `powershell.exe` on Windows), and `sha256sum`/`shasum` on your PATH, plus network access to nodejs.org.

The first build takes 2-5 minutes while Rust compiles. After that, changes rebuild in seconds.

To produce a standalone installer: `npm run build`. The output goes to `src-tauri/target/release/bundle/`.

> `dist/` is intentionally committed. Tauri reads frontend assets from it at build time. Do not add it to `.gitignore`. See [CONTRIBUTING.md](CONTRIBUTING.md).

---

## License

[GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0).

You can use, modify, and distribute Aerini freely. If you modify Aerini and run it as a network-accessible service, you must release your modifications under AGPL-3.0.

---

## Commercial licensing

Aerini is open source under [AGPL-3.0](LICENSE). Under AGPL-3.0:

- You can use, modify, and self-host Aerini freely.
- If you run a **modified version** of `aerini-server` as a **network-accessible service** for others, you must publish your modifications under the same license.
- Running the **unmodified** binary as an internal tool for your own team does not trigger this obligation.

A commercial license removes the AGPL copyleft obligation entirely. No source disclosure required, proprietary modifications permitted.

You likely need a commercial license if you are building a product or SaaS on top of Aerini, distributing modified Aerini to clients, deploying a modified `aerini-server` as a service for others, or if your legal team requires a warranty or compliance document.

[View pricing and license terms](https://panchak2d.github.io/aerini/pricing)

For enterprise or volume licensing: see [CONTACT.md](CONTACT.md)

## Privacy

Aerini collects no telemetry, analytics, usage data, or crash reports. Beyond what you explicitly configure in your workflows, the only outbound connection Aerini's own UI makes is loading the Inter font from Google Fonts over HTTPS — see [Security §Desktop security model](docs/guide/security.md#desktop-security-model) for details. See the [transparency report template](transparency/TEMPLATE.md) for the full audit trail.

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

**CLA:** Required before your first PR is merged. [CLA Assistant](https://cla-assistant.io) posts a one-click sign link on your first PR. GitHub OAuth, done in seconds.
