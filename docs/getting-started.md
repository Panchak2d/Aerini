# Getting Started

By the end of this guide you'll have Flowo installed and a working workflow that fetches data from a public API and posts it to Slack — running automatically on a schedule. Takes about 20 minutes.

If you've never used automation tools before, read [Concepts](concepts.md) first. It explains what workflows, nodes, and credentials are before you start building.

---

## What you need

Three tools need to be installed before Flowo will build. If you've already set up a Rust development environment, you probably have all of them.

| Tool | Version | Install |
|---|---|---|
| Rust | 1.77+ | [rustup.rs](https://rustup.rs/) |
| Node.js | 18+ | [nodejs.org](https://nodejs.org/) |
| Tauri CLI | 2.x | `cargo install tauri-cli --version "^2" --locked` |

### Rust

Rust is the programming language Flowo is written in. Installing it also installs `cargo`, which you'll use to install the Tauri CLI.

Go to [rustup.rs](https://rustup.rs/) and follow the instructions for your OS. On macOS and Linux: run one command in a terminal. On Windows: download and run an installer.

After installation, open a new terminal and verify it worked:

```bash
rustc --version
cargo --version
```

Both should print version numbers. If they don't, close and reopen your terminal — the installer sometimes needs a fresh session for PATH changes to take effect.

### Node.js

Node.js needs to be installed and on your PATH. Two things depend on it: (1) Flowo's frontend is built with JavaScript, and (2) the Code (JS) node inside Flowo spawns Node.js at runtime to run your scripts.

Go to [nodejs.org](https://nodejs.org/) and download the **LTS** version. Run the installer.

After installation, verify in a new terminal:

```bash
node --version
npm --version
```

**On Windows:** during installation, make sure the option to add Node.js to PATH is checked. If `node --version` returns "command not found" after installing, reinstall and check that box.

### Tauri CLI

Once Rust is installed, run this in a terminal:

```bash
cargo install tauri-cli --version "^2" --locked
```

This downloads and compiles the Tauri build tool. It takes a few minutes. Verify:

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

`npm run dev` compiles the Rust code the first time, which takes 2–5 minutes. After that, the Flowo window opens.

If you don't have `git`: on macOS, running `git` in the terminal prompts you to install Xcode Command Line Tools. On Windows, download from [git-scm.com](https://git-scm.com/). On Linux, use your package manager (`sudo apt install git` on Ubuntu).

### Building an installer

To create a proper installable app you can run without `npm run dev`:

```bash
npm run build
```

The installer appears in `src-tauri/target/release/bundle/`:

| Platform | File | How to install |
|---|---|---|
| macOS | `.dmg` in `macos/` | Open it, drag Flowo to Applications |
| Windows | `.exe` or `.msi` in `msi/` | Run the installer |
| Linux | `.AppImage` in `appimage/` | `chmod +x Flowo*.AppImage` then run it |

---

## The canvas

When Flowo opens you'll see a blank canvas. The left sidebar has three sections:

- **Node Panel** — a search bar at the top. Type any node name and click to place it on the canvas. Keyboard shortcut: `Space` or `Ctrl+K` from anywhere.
- **Workflows** — your saved workflows. Click one to load it.
- **Background Runs** — workflows currently running on a schedule or waiting for a webhook.

**How to connect nodes:** every node has small circles on its edges. The circle on the right side is the output port; the circle on the left side is the input port. Click and hold an output port, drag to an input port of another node, release. A line appears connecting them.

**Canvas controls:**

| Action | How |
|---|---|
| Pan | Click and drag on empty space |
| Zoom | Scroll wheel |
| Select a node | Click it |
| Select multiple | Shift+click, or drag a selection box |
| Move a node | Drag it |
| Connect two nodes | Drag from output port to input port |
| Disconnect | Drag a connection wire onto empty space |
| Fit everything in view | `Ctrl+Shift+F` |

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
| `Delete` / `Backspace` | Delete selected node(s) |
| `Ctrl+Shift+F` | Fit canvas to screen |
| `Space` or `Ctrl+K` | Open node search |
| `F` | Toggle focus mode (hides sidebar) |
| `M` | Toggle minimap |
| `?` | Show all shortcuts |

---

## Build your first workflow

This example fetches a to-do item from a free test API and posts it to Slack. If you don't have Slack, swap the Slack node for a **Desktop Notification** node at step 5 — the rest is identical.

### Step 1 — Get a Slack bot token

Skip this step if you're using Desktop Notification.

1. Go to [api.slack.com/apps](https://api.slack.com/apps) and sign in.
2. Click **Create New App → From scratch**. Name it (e.g. `Flowo Bot`) and select your workspace.
3. In the left sidebar, click **OAuth & Permissions**.
4. Scroll to **Scopes → Bot Token Scopes**, click **Add an OAuth Scope**, add `chat:write`.
5. Scroll back up, click **Install to Workspace**, confirm.
6. Copy the **Bot User OAuth Token** — it starts with `xoxb-`.
7. In Slack, open the channel you want the bot to post to. Type `/invite @Flowo Bot` and send.

### Step 2 — Place a Schedule node

Press `Space`, type `schedule`, click **Schedule**. A node appears on the canvas.

Click the node to open its config panel on the right. Set:
- **Mode:** `interval`
- **Interval (seconds):** `3600`

This configures the workflow to run every hour. For now you'll test it manually — the interval only matters when you start the background run.

### Step 3 — Place and connect an HTTP Request node

Press `Space`, type `http`, click **HTTP Request**. Connect Schedule → HTTP Request by dragging from the Schedule node's output port to the HTTP Request node's input port.

Open the HTTP Request config:
- **Method:** `GET`
- **URL:** `https://jsonplaceholder.typicode.com/todos/1`

This fetches a sample to-do item. JSONPlaceholder is a free, stable testing API with no authentication or rate limits — it always returns the same predictable JSON.

### Step 4 — Place and connect a Code node

Press `Space`, type `code`, click **Code (JS)**. Connect HTTP Request → Code.

Open the Code node config. In the **Code** field, enter:

```javascript
const todo = input.body;
output({
  message: `Todo #${todo.id}: ${todo.title} (completed: ${todo.completed})`
});
```

The `input` variable contains the HTTP Request node's output. `input.body` is the JSON response body. This script extracts the id, title, and completed status into a single message string.

> **Rename this node.** Double-click the node's title on the canvas and rename it to `code`. Short names are easier to reference in expressions.

### Step 5 — Place and connect a Slack (or Notification) node

**For Slack:**

First save your bot token as a credential:
1. Click **Connections** in the sidebar (the lock icon).
2. Click **Add Credential**.
3. **ID:** `slack-bot` (used to reference it — no spaces)
4. **Name:** `Slack Bot Token` (the label shown in dropdowns)
5. **Value:** paste your `xoxb-...` token
6. Save.

Press `Space`, type `slack`, click **Slack**. Connect Code → Slack.

Open the Slack config:
- **Channel:** `#general` (or whichever channel you invited the bot to)
- **Text:** `{{code.output.message}}`
- **API Key:** select `Slack Bot Token` from the dropdown

The `{{code.output.message}}` expression tells Flowo to insert the `message` field from the Code node's output. See [Expressions](expressions.md) for more on this syntax.

**For Desktop Notification instead:**

Press `Space`, type `notification`, click **Desktop Notification**. Connect Code → Desktop Notification.

Config:
- **Title:** `Daily Todo`
- **Body:** `{{code.output.message}}`

### Step 6 — Test it manually

Before scheduling, make sure it works. Press `Ctrl+Enter` to run the full workflow.

The output drawer opens at the bottom. Each node turns green as it completes. If any node fails, it turns red — click the **Errors** tab to see exactly what went wrong.

If the Slack message (or notification) appears, everything is working.

### Step 7 — Save and schedule

Press `Ctrl+S`. Flowo prompts for a name — call it something like `Hourly Todo Update`.

Click the **Run in background** button in the toolbar (the play-with-clock icon). Flowo saves the workflow and hands it to the scheduler. In the **Background Runs** sidebar section you'll see it appear with a pulsing green dot.

The workflow now runs every hour automatically, as long as Flowo is open. To keep it running after Flowo closes, or to deploy it to a server that runs 24/7, see [Server Deployment](server-deploy.md).

> **Always On:** right-click the job in the Background Runs section and enable **Always On**. The scheduler restarts the workflow automatically if it errors or if Flowo is restarted.

---

## Reading run results

After a run the output drawer has these tabs:

| Tab | What's in it |
|---|---|
| **Summary** | Pass/fail, total duration, node-by-node status |
| **Results** | Full JSON output of every node |
| **Errors** | Failed nodes with error messages |
| **Logs** | All log lines in order. Click a line to highlight that node on the canvas. |
| **Debug** | Raw execution data for deep troubleshooting |
| **History** | Past runs for this workflow |

**Running a single node:** right-click any node and choose **Run from here**. Flowo builds a subgraph of that node plus all its upstream dependencies and runs just that portion — useful for testing one step without triggering downstream side effects like sending emails or posting messages.

---

## Version history

Every save creates a snapshot. To restore:

1. Right-click the workflow in the sidebar → **Versions**.
2. Browse snapshots by date.
3. Click any snapshot to preview it, then click **Restore**.

---

## Where data is stored

All user data lives in:

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/com.flowo.app/` |
| Windows | `%APPDATA%\com.flowo.app\` |
| Linux | `~/.local/share/com.flowo.app/` |

Two SQLite files: `workflows.db` (workflows and run history) and `credentials.db` (encrypted API keys). To completely reset Flowo, delete this directory.

---

## What's next

- [Concepts](concepts.md) — if anything above was confusing, this page explains it in plain language
- [Nodes Reference](nodes.md) — full parameter docs for all 39 nodes
- [Expressions](expressions.md) — how to wire node outputs into other nodes' fields
- [Credentials](credentials.md) — managing API keys and how to get them for each service
- [Background Runs](background-runs.md) — schedules, webhooks, and run history
