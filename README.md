# Flowo

Flowo is a visual workflow automation desktop app for macOS, Windows, and Linux. Drag nodes onto a canvas, connect them together, and automate tasks: run them manually, on a schedule, or trigger them via HTTP request.

No account. No cloud. No subscription. Everything runs on your machine.

---

## What you can build

- **Scheduled reports:** fetch data from an API every morning, transform it, email or Slack the result
- **Alerts:** poll an endpoint on a timer, post to a channel when something looks wrong
- **AI pipelines:** chain an AI Prompt into a Transform into a Notion page create
- **Webhook receivers:** accept a GitHub or Stripe webhook, run logic, send a notification
- **File processing:** read files, transform their content, write results elsewhere
- **Social media uploads:** generate images with AI and post them to YouTube, Instagram, or TikTok

Flowo is a local-first tool for individuals and small setups. For workflows that need to run 24/7 on a server, `flowo-server` handles that without requiring the desktop app on the server.

---

## Guides

| Guide | What it covers |
|---|---|
| [Getting Started](docs/getting-started.md) | Install, build your first workflow, run it, schedule it |
| [Concepts](docs/concepts.md) | What a workflow is, what nodes are, how everything fits together |
| [Nodes Reference](docs/nodes.md) | All 39 nodes: parameters, outputs, and what each one does |
| [Expressions](docs/expressions.md) | `{{...}}` syntax for wiring node outputs into other nodes |
| [Credentials](docs/credentials.md) | Storing API keys securely and getting them from every supported service |
| [Background Runs](docs/background-runs.md) | Schedules, webhook triggers, run history |
| [Server Deployment](docs/server-deploy.md) | Running workflows 24/7 on a Linux server |
| [Security](docs/security.md) | Encryption, dangerous nodes, SSRF protection, server hardening |

Developer guides:

| Guide | What it covers |
|---|---|
| [Architecture](docs/architecture.md) | How the engine, Tauri shell, and server binary fit together |
| [Embedding flowo-engine](docs/embedding.md) | Use the engine as a Rust library in your own program |
| [Custom Node Authoring](docs/node-authoring.md) | Adding new node types to Flowo |
| [Plugin Authoring](docs/plugin-authoring.md) | Writing and distributing `.wasm` plugin nodes |
| [Desktop IPC Reference](docs/desktop-ipc-reference.md) | Every command the frontend can call into the Tauri shell |
| [Schema Migrations](docs/schema-migrations.md) | How workflow format changes are handled across versions |

---

## Node categories

Flowo ships 39 built-in nodes:

**Triggers:** Manual Trigger, Schedule, Webhook

**Logic:** If / Condition, Switch, Loop (For Each), Merge, Stop, Collect Files

**Flow Control:** Delay, Wait

**AI:** AI Prompt, AI Agent, AI Memory, Text Splitter, Image Generation

**Actions:** HTTP Request, Shell Command, Code (JS), Send Email, SendGrid, File, Desktop Notification, Save to Folder, Social Upload, Database, S3 Storage

**Integrations:** Slack, Discord, GitHub, Google Sheets, Notion, Telegram, Stripe

**Data & Utility:** Transform Data, JSON, Set Variable, Get Variable, Output

---

## Plugins

Flowo supports `.wasm` plugin nodes. Write a new node type in Rust, compile it to `wasm32-wasip2`, and install it through **Settings → Plugins** — pick a plugin folder, then install via the file picker or just drag the `.wasm` file onto the window. Plugin nodes appear in the palette automatically (marked with a small "P" badge so you can tell them apart from built-ins) and execute with the same isolation guarantees as built-in nodes — each call runs in a sandboxed Wasmtime instance with a 64 MiB memory limit and outbound HTTP access but no filesystem access.

The Settings panel also lists installed plugins and surfaces any that failed to load (with the reason) instead of just dropping them, so a bad build doesn't disappear silently.

See [Plugin Authoring](docs/plugin-authoring.md) to get started, or copy `examples/plugin-template/` as a starting point.

---

## Embedding the engine

`flowo-engine` is a standalone Rust crate — the workflow model, executor, scheduler, and node registry don't depend on Tauri or any UI. You can pull it into your own Rust program, register your own nodes alongside (or instead of) the 39 built-ins, supply your own credential resolver and event sink, and run workflows headless — no desktop app, no database required unless you want run history.

This is the same engine that powers the desktop app and `flowo-server`, so anything documented for those (retries, parallel execution, expressions, plugins) works the same way when embedded.

See [Embedding flowo-engine](docs/embedding.md) for a complete working example, the stable API surface, and what's safe to depend on across versions.

---

## Requirements

| Tool | Version | Why |
|---|---|---|
| Rust (stable) | 1.77+ | Builds the engine and desktop shell |
| Node.js | 18+ | Required at runtime for the Code (JS) node |
| Tauri CLI | 2.x | Packages the desktop app |

Node.js must be on your PATH, not just installed. The Code (JS) node spawns it as a subprocess at runtime; it needs to be reachable as `node` in a terminal.

---

## Installing

```bash
git clone https://github.com/Panchak2d/flowo
cd flowo
npm install
npm run dev
```

The first build takes 2-5 minutes while Rust compiles. After that, changes rebuild in seconds.

To produce a standalone installer: `npm run build`. The output goes to `src-tauri/target/release/bundle/`.

> `dist/` is intentionally committed. Tauri reads frontend assets from it at build time. Do not add it to `.gitignore`. See [CONTRIBUTING.md](CONTRIBUTING.md).

---

## License

[GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0).

You can use, modify, and distribute Flowo freely. If you modify Flowo and run it as a network-accessible service, you must release your modifications under AGPL-3.0.

---

## Commercial licensing

Flowo is open source under [AGPL-3.0](LICENSE). Under AGPL-3.0:

- You can use, modify, and self-host Flowo freely.
- If you run a **modified version** of `flowo-server` as a **network-accessible service** for others, you must publish your modifications under the same license.
- Running the **unmodified** binary as an internal tool for your own team does not trigger this obligation.

A commercial license removes the AGPL copyleft obligation entirely. No source disclosure required, proprietary modifications permitted.

You likely need a commercial license if you are building a product or SaaS on top of Flowo, distributing modified Flowo to clients, deploying a modified `flowo-server` as a service for others, or if your legal team requires a warranty or compliance document.

[View pricing and license terms](https://panchak2d.github.io/flowo/pricing)

For enterprise or volume licensing: see [CONTACT.md](CONTACT.md)

## Privacy

Flowo collects no telemetry, analytics, usage data, or crash reports. No network requests are made by the app or server beyond what you explicitly configure in your workflows. See the [transparency report template](transparency/TEMPLATE.md) for the full audit trail.

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

**CLA:** Required before your first PR is merged. [CLA Assistant](https://cla-assistant.io) posts a one-click sign link on your first PR. GitHub OAuth, done in seconds.
