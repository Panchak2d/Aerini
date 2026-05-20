# Getting Started

By the end of this guide you'll have Flowo installed, a working workflow that fetches data from a public API and posts it to Slack, and that workflow running on a schedule automatically. Takes about 20–30 minutes if you're new to development tools, about 15 if you've done this kind of thing before.

---

## Prerequisites

Flowo is a desktop app built with Rust and web technologies. To build it from source you need three tools installed on your computer. If you've never done this before, the explanations below walk through each one.

| Tool | Version | Install |
|---|---|---|
| Rust (stable) | 1.77+ | [rustup.rs](https://rustup.rs/) |
| Node.js | 18+ | [nodejs.org](https://nodejs.org/) |
| Tauri CLI | 2.x | `cargo install tauri-cli --version "^2" --locked` |

### What these are and why you need them

**Rust** is a programming language. Flowo's engine and the Tauri shell that wraps the app are written in Rust. Installing Rust also installs `cargo` — Rust's package manager, which you'll use to install the Tauri CLI.

**Node.js** is a JavaScript runtime. Two reasons it's needed: (1) the frontend of the Flowo app is built with JavaScript/TypeScript, and (2) the **Code (JS)** node inside Flowo executes JavaScript snippets at runtime — it spawns Node.js as a subprocess. Node.js must be on your PATH (see below) even after installation.

**Tauri CLI** is the build tool that packages everything into a desktop app. You install it via Rust's `cargo` command after Rust is installed.

### Installing Rust

Go to [rustup.rs](https://rustup.rs/) and follow the instructions for your OS. On macOS and Linux, you'll run a single command in your terminal. On Windows, you'll download and run an `.exe` installer.

After installation, open a new terminal window and verify:

```bash
rustc --version
cargo --version
```

Both should print a version number. If they don't, close and reopen your terminal — the PATH change from the installer sometimes needs a fresh session to take effect.

### Installing Node.js

Go to [nodejs.org](https://nodejs.org/) and download the **LTS** (Long-Term Support) version for your OS. Run the installer.

After installation, verify in a new terminal window:

```bash
node --version
npm --version
```

Both should print a version number.

**Making sure Node.js is on your PATH:** PATH is a list of directories your computer searches when you type a command. If Node.js isn't on your PATH, Flowo can find Node.js during installation but the **Code (JS)** node will fail at runtime with a "node not found" error.

To check: open a terminal and type `node --version`. If you see a version number, it's on your PATH. If you see "command not found" or similar, reinstall Node.js and on Windows ensure you check the "Add to PATH" option during installation.

### Installing the Tauri CLI

Once Rust is installed, run this in your terminal:

```bash
cargo install tauri-cli --version "^2" --locked
```

This takes a few minutes while it downloads and compiles. Verify with:

```bash
cargo tauri --version
```

---

## Install Flowo

```bash
git clone https://github.com/Panchak2d/flowo
cd flowo
npm install
npm run dev
```

`git clone` downloads the source code. `npm install` downloads the JavaScript dependencies. `npm run dev` compiles the Rust code (the first time this takes 2–5 minutes) and opens the Flowo window.

If you don't have `git` installed: on macOS, running `git` in the terminal prompts you to install Xcode Command Line Tools. On Windows, download from [git-scm.com](https://git-scm.com/). On Linux, use your package manager (`sudo apt install git`).

### Building a standalone installer

Once you've verified Flowo works in dev mode, build a proper installable app:

```bash
npm run build
```

The installer is written to `src-tauri/target/release/bundle/`:

| Platform | What you get | How to install it |
|---|---|---|
| macOS | `.dmg` file in `macos/` | Open it, drag Flowo to Applications |
| Windows | `.exe` or `.msi` in `msi/` | Run the installer |
| Linux | `.AppImage` in `appimage/` | Make it executable (`chmod +x Flowo*.AppImage`) and run it |

---

## The canvas

When Flowo opens you're looking at a blank canvas. The left sidebar has three sections:

- **Node Panel** — the search bar at the very top of the sidebar. Type any node name to find it, then click to place it on the canvas. You can also open it from anywhere with `Space` or `Ctrl+K`.
- **Workflows** — your saved workflows, sorted by last modified. Click one to load it.
- **Background Runs** — workflows currently running on a schedule or waiting for a webhook.

**Connecting nodes — what to look for:** Every node has small circles on its edges. The circle on the **right side** is the output port (data flows out of here). The circle on the **left side** is the input port (data flows in here). To connect two nodes, click and hold on the output port of one node and drag to the input port of another. You'll see a line following your cursor — release it over the destination port to create the connection.

**Canvas controls:**

| Action | How |
|---|---|
| Pan | Click and drag on empty space |
| Zoom | Scroll wheel |
| Select a node | Click it |
| Select multiple | Shift+click, or drag a selection box |
| Move a node | Drag it |
| Connect two nodes | Drag from one node's output port to another's input port |
| Disconnect | Drag a connection wire onto empty space |
| Fit everything in view | `Ctrl+Shift+F` or double-click empty space |

---

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+S` / `Cmd+S` | Save workflow |
| `Ctrl+N` / `Cmd+N` | New workflow |
| `Ctrl+Enter` / `Cmd+Enter` | Run workflow |
| `Ctrl+Z` / `Cmd+Z` | Undo |
| `Ctrl+Y` / `Cmd+Y` | Redo |
| `Ctrl+D` / `Cmd+D` | Duplicate selected node(s) |
| `Ctrl+A` / `Cmd+A` | Select all nodes |
| `Delete` / `Backspace` | Delete selected node(s) |
| `Ctrl+Shift+F` | Fit canvas to screen |
| `Space` or `Ctrl+K` | Open node search |
| `F` | Toggle focus mode (hides sidebar) |
| `M` | Toggle minimap |
| `?` or `/` | Show keyboard shortcuts |

---

## Build your first workflow

This example fetches a to-do item from a public API and posts it to Slack. You'll need a Slack workspace where you have permission to add apps.

**Don't have Slack or just want to test?** You can skip the Slack node entirely and replace it with a **Desktop Notification** node instead. Everything else in this guide is the same.

### Step 1 — Get a Slack bot token

Skip this step if you're using Desktop Notification instead.

1. Go to [api.slack.com/apps](https://api.slack.com/apps) and sign in with your Slack account.
2. Click **Create New App** → **From scratch**.
3. Give it a name (e.g. `Flowo Bot`) and select your workspace.
4. In the left sidebar, click **OAuth & Permissions**.
5. Scroll down to **Scopes → Bot Token Scopes**. Click **Add an OAuth Scope** and add `chat:write`.
6. Scroll back up and click **Install to Workspace**. Confirm the permissions.
7. After installing, you'll see a **Bot User OAuth Token** starting with `xoxb-`. Copy it — you'll need it in Step 4.
8. Finally, open Slack and invite the bot to the channel you want to post to: open the channel, type `/invite @Flowo Bot` (or whatever you named it), and send.

### Step 2 — Add a Schedule trigger node

Open node search (`Space`), type `schedule`, click **Schedule**. A node appears on the canvas.

Click the node to open its config panel. Set:
- **Mode:** `interval`
- **Interval (seconds):** `3600` (runs every hour)

For now this just sets up the timing. You'll use the Manual Trigger option to test, so leave this as-is.

> **Why a Schedule node first?** A workflow needs a trigger as its first node to run on a schedule. If you build the workflow with HTTP Request as the first node, you'd need to swap it out before scheduling — better to start correctly.

### Step 3 — Add an HTTP Request node

Open node search (`Space`), type `http`, click **HTTP Request**. Connect **Schedule → HTTP Request** by dragging from Schedule's output port to HTTP Request's input port.

Click the HTTP Request node to open its config:
- **Method:** `GET`
- **URL:** `https://jsonplaceholder.typicode.com/todos/1`

This fetches a sample to-do item in JSON format. JSONPlaceholder is a free, stable public testing API with no authentication, no rate limits, and a documented schema — it's designed for exactly this kind of use.

> **Why not a weather API?** Real weather APIs change their schemas, enforce rate limits, or go down. JSONPlaceholder has none of these problems and is the standard tool for testing HTTP integrations.

### Step 4 — Add a Code node

Search for `code`, place **Code (JS)** on the canvas. Connect **HTTP Request → Code**.

Open the Code node's config. The code field is a JavaScript snippet. It receives `input` (the previous node's output) and returns a value by calling `output()`:

```javascript
const todo = input.body;
output({ message: `Todo #${todo.id}: ${todo.title} (completed: ${todo.completed})` });
```

The JSONPlaceholder response looks like `{"userId":1,"id":1,"title":"delectus aut autem","completed":false}`. This snippet extracts the `id`, `title`, and `completed` fields and packages them into a single `message` string.

> **Rename this node.** Double-click the node's title on the canvas and rename it from "Code (JS)" to `code`. This makes it much easier to reference in the next step.

### Step 5 — Add a Slack (or Notification) node

**If using Slack:**

Search for `slack`, place the **Slack** node, connect **Code → Slack**.

First, save your bot token as a credential:
1. Click the **Connections** button in the sidebar (lock icon).
2. Click **Add Credential**.
3. Set the **ID** to `slack-bot` (this is what you'll reference in nodes — keep it short, no spaces).
4. Set the **Name** to `Slack Bot Token` (the friendly label you'll see in dropdowns).
5. Paste your `xoxb-...` token in the **Value** field.
6. Save.

Now open the Slack node's config:
- **Channel:** `#general` (or the channel you invited the bot to)
- **Text:** `{{code.output.message}}`
- **API Key:** select `Slack Bot Token` from the dropdown

The `{{code.output.message}}` expression tells Flowo to pull the `message` field from the Code node's output. See [Expressions](expressions.md) for how this syntax works.

**If using Desktop Notification instead:**

Search for `notification`, place the **Desktop Notification** node, connect **Code → Desktop Notification**.

Config:
- **Title:** `Weather Update`
- **Body:** `{{code.output.message}}`

### Step 6 — Run it manually first

Before scheduling, test that everything works. Right-click the Schedule node and choose **Run from here** — this runs the workflow from that point forward with a simulated trigger, without waiting for the actual schedule.

Alternatively press `Ctrl+Enter` to run the full workflow.

The output drawer opens at the bottom. Each node turns green as it completes. If something fails, the failing node turns red — click the **Errors** tab to see exactly what went wrong.

If the Slack message arrives, everything is working.

### Step 7 — Save and schedule

`Ctrl+S`. Flowo prompts for a name if the workflow is still "Untitled". Name it something like `Hourly Weather Update`.

Now click the **Run in background** button in the toolbar (the play-with-clock icon). Flowo saves the workflow and hands it to the scheduler. The **Background Runs** section in the sidebar shows it as active with a pulsing green dot.

The workflow will run every hour as long as Flowo is open. Close Flowo and the scheduler stops. For 24/7 scheduling without keeping the app open, see [Server Deployment](server-deploy.md).

**Always On:** right-click the job in the Background Runs sidebar and enable **Always On**. The scheduler restarts the workflow automatically if it errors or if Flowo is restarted — useful for workflows you want running indefinitely.

---

## Reading run results

After a run, the output drawer has these tabs:

| Tab | What's in it |
|---|---|
| **Summary** | Pass/fail, total duration, node-by-node status |
| **Results** | Full JSON output of every node |
| **Errors** | Failed nodes with error messages and full context |
| **Logs** | All log lines from the run, in order. Click a log line to select that node on the canvas. |
| **Debug** | Raw execution data for deep troubleshooting |
| **History** | Past runs for this workflow |

**Running a single node:** right-click any node and choose **Run from here**. Flowo builds a subgraph of that node plus all its ancestor nodes (everything upstream of it) and runs just that portion. This is the fastest way to test one step without triggering downstream side effects — no emails sent, no Slack messages posted — while you're still configuring things.

---

## Version history

Every time you save, Flowo takes a snapshot of the workflow. To restore an earlier version:

1. Right-click the workflow in the left sidebar and choose **Versions**.
2. Browse the list of snapshots by date.
3. Click any snapshot to preview it, then click **Restore** to roll back.

The History tab in the output drawer shows past run results (what each run produced), not workflow snapshots — these are two different things.

---

## Data storage

All user data lives in the OS application data directory. You can find it here:

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/com.flowo.app/` |
| Windows | `%APPDATA%\com.flowo.app\` |
| Linux | `~/.local/share/com.flowo.app/` |

Two SQLite databases: `workflows.db` (workflows and run history) and `credentials.db` (encrypted API keys). To completely reset Flowo during development, delete this directory.

---

## What's next

- [Nodes Reference](nodes.md) — full parameter docs for all 34 nodes
- [Expressions](expressions.md) — how to wire node outputs into other nodes' inputs
- [Credentials](credentials.md) — managing API keys securely, and how to get them for common services
- [Background Runs](background-runs.md) — scheduling, webhook triggers, and n8n import
