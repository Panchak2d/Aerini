# Getting Started with Aerini

Aerini is a desktop app that lets you automate tasks by connecting steps together on a visual canvas — no coding required for most things. You drag boxes onto a screen, connect them like a flowchart, and press Run.

This guide walks you through installing Aerini, touring the interface, and building your first working automation from scratch.

---

## Before you start

**What you need:**

- A computer running macOS, Windows, or Linux
- About 10 minutes
- Nothing else — no account, no subscription, no cloud service

**Optional for the tutorial:** a Slack workspace where you can create a bot. If you don't have one, the tutorial includes a Desktop Notification alternative that needs nothing external.

---

## Install

Download the installer for your platform from the [Releases page](https://github.com/Panchak2d/aerini/releases/latest):

| Your computer | Download this |
|---|---|
| Mac (M1, M2, M3, M4) | `Aerini_x.x.x_aarch64.dmg` |
| Mac (older Intel) | `Aerini_x.x.x_x64.dmg` |
| Windows | `Aerini_x.x.x_x64-setup.exe` |
| Linux | `Aerini_x.x.x_amd64.AppImage` |

Not sure which Mac you have? Click the Apple menu → **About This Mac**. If it says "Apple M1" or higher, you have Apple Silicon. Intel Macs show "Intel Core."

Run the installer. On macOS you may see a security warning — go to **System Settings → Privacy & Security** and click **Open Anyway**. This is normal for apps distributed outside the App Store.

> **Building from source instead?** See [CONTRIBUTING.md](../CONTRIBUTING.md#building-from-source). First build takes 2–5 minutes while Rust compiles.

---

## The interface

When Aerini opens you'll see three areas:

**Left sidebar** — three sections stacked vertically:
- *Nodes* — search and browse every available node type
- *Workflows* — your saved automations
- *Background Runs* — workflows currently running on a schedule or waiting for a webhook

**Canvas** — the large empty area in the middle. This is where you build.

**Toolbar** — across the top. Has buttons for Save, Run, Run in Background, and settings.

### Placing a node

Press `Space` or `Ctrl+K` to open node search. Type any node name and click it — the node appears on the canvas.

### Connecting nodes

Each node has small circles on its edges. The circle on the **right** is the output (data leaves from here). The circle on the **left** is the input (data arrives here).

To connect two nodes: click and hold the output circle on the first node, drag to the input circle on the second, and release. A wire appears. To remove a connection, grab the wire and drag it onto empty canvas space.

### Canvas navigation

| Action | How |
|---|---|
| Pan | Click and drag on empty space |
| Zoom | Scroll wheel |
| Select a node | Click it |
| Select multiple | Shift+click, or drag a selection box |
| Move a node | Drag it |
| Fit everything in view | `Ctrl+Shift+F` |

---

## Keyboard shortcuts

| Shortcut | What it does |
|---|---|
| `Ctrl+S` / `Cmd+S` | Save workflow |
| `Ctrl+N` / `Cmd+N` | New workflow |
| `Ctrl+Enter` / `Cmd+Enter` | Run workflow |
| `Ctrl+Z` / `Cmd+Z` | Undo |
| `Ctrl+Y` / `Cmd+Y` | Redo |
| `Ctrl+D` / `Cmd+D` | Duplicate selected node(s) |
| `Delete` / `Backspace` | Delete selected |
| `Ctrl+Shift+F` | Fit canvas to screen |
| `Space` or `Ctrl+K` | Open node search |
| `F` | Hide/show sidebar |
| `M` | Toggle minimap |
| `?` | Show all shortcuts |

---

## Build your first workflow

This tutorial fetches a sample to-do item from a free test API and delivers it to you — either as a Slack message or a desktop notification. Both take about the same time to set up.

**What you'll build:** Schedule → HTTP Request → Code (JS) → Slack or Notification

**Time needed:** 10–15 minutes

---

### Step 1 — Get a Slack bot token (skip if using Desktop Notification)

If you'd rather skip Slack and just see a notification pop up on your computer, jump ahead to Step 2. The tutorial note at Step 5 tells you what to do differently.

1. Go to [api.slack.com/apps](https://api.slack.com/apps) and sign in.
2. Click **Create New App → From scratch**. Name it something like `Aerini Bot` and pick your workspace.
3. In the left sidebar, click **OAuth & Permissions**.
4. Scroll to **Bot Token Scopes**, click **Add an OAuth Scope**, and add `chat:write`.
5. Scroll back to the top, click **Install to Workspace**, and confirm.
6. Copy the **Bot User OAuth Token** — it starts with `xoxb-`. Keep it somewhere handy.
7. In Slack, open the channel you want the bot to post to. Type `/invite @Aerini Bot` and send.

---

### Step 2 — Place a Schedule node

Press `Space`, type `schedule`, and click **Schedule**. A node appears on the canvas.

Double-click the node to open its configuration panel on the right side of the screen. Set:
- **Mode:** `interval`
- **Interval (seconds):** `3600`

This means the workflow will run once per hour once you schedule it. For now you'll test it manually — the interval only matters when you start it as a background job.

---

### Step 3 — Place and connect an HTTP Request node

Press `Space`, type `http`, and click **HTTP Request**. Then connect it to the Schedule node: drag from the Schedule node's output circle (right side) to the HTTP Request node's input circle (left side).

Double-click the HTTP Request node to open its config:
- **Method:** `GET`
- **URL:** `https://jsonplaceholder.typicode.com/todos/1`

This URL is a free testing API. It always returns the same predictable JSON — no account or API key needed.

---

### Step 4 — Place and connect a Code node

Press `Space`, type `code`, and click **Code (JS)**.

Connect HTTP Request → Code: click and hold the **Success** output circle on HTTP Request (the top-right circle, labelled "Success"), drag to the input circle on the left side of the Code node, and release. Do not drag from the Error port — that only fires when the HTTP request itself fails.

Code (JS) runs on a Node.js runtime bundled with Aerini — nothing to install separately.

Double-click the Code node to open its config. In the **Code** field, paste:

```javascript
const todo = input.body;
output({
  message: `Todo #${todo.id}: ${todo.title} (completed: ${todo.completed})`
});
```

`input` is automatically set to the full output of the directly connected upstream node — in this case, the HTTP Request node. Its shape is `{ status, body, headers }`. So `input.body` is the parsed JSON response body, `input.status` is the HTTP status code, and `input.headers` is an object of response headers.

Alternatively, you can access any upstream node by name via the `context` object: `context["HTTP Request"].body` gives the same result and is useful when you have multiple upstream nodes.

**Rename this node.** In the config panel, find the **Name** field under the NODE section and change it to `code`. Short names make the next step easier.

---

### Step 5 — Place and connect a Slack or Notification node

**If you're using Slack:**

First, save your bot token as a credential so the key is stored securely:

1. Click the **Connections** button in the sidebar (the lock icon).
2. Click **Add Credential**.
3. **ID:** `slack-bot` (lowercase, no spaces — this is the internal reference name)
4. **Name:** `Slack Bot Token` (the label shown in dropdowns)
5. **Value:** paste your `xoxb-...` token
6. Click Save.

Now place the node: press `Space`, type `slack`, click **Slack**. Connect Code → Slack.

Open the Slack node's config:
- **Channel:** `#general` (or whichever channel you invited the bot to)
- **Text:** `{{code.output.result.message}}`
- **API Key:** select `Slack Bot Token` from the dropdown

The `{{code.output.result.message}}` part is an expression — it tells Aerini to insert the `message` value from the Code node's output when the workflow runs. Press `{{` in any text field to see a picker showing all available values; you can click to insert them without typing the path manually.

---

**If you're using Desktop Notification instead:**

Press `Space`, type `notification`, click **Desktop Notification**. Connect Code → Desktop Notification.

Config:
- **Title:** `Daily Todo`
- **Body:** `{{code.output.result.message}}`

---

### Step 6 — Run it manually

Before setting up a schedule, confirm the workflow actually works. Press `Ctrl+Enter` to run it.

The output drawer slides up from the bottom. Each node turns green as it completes. If a node fails, it turns red — click the **Errors** tab to see what went wrong.

If you see a Slack message appear in your channel (or a desktop notification pop up), everything is working.

---

### Step 7 — Save and schedule

Press `Ctrl+S`. Aerini asks for a name — something like `Hourly Todo Update` works fine.

Click the **Run in background** button in the toolbar (it looks like a play button with a clock). Aerini validates and saves the workflow, then hands it to the scheduler. The **Background Runs** section in the sidebar shows it with a pulsing green dot.

The workflow now runs every hour on its own, as long as Aerini is open. If you close Aerini, the job pauses until you reopen it. For round-the-clock execution on a server, see [Server Deployment](server-deploy.md).

> **Always On:** right-click the job in the Background Runs section and enable **Always On**. With this enabled, the scheduler automatically restarts the workflow if it errors or if Aerini itself restarts.

---

## Reading the results

After a run, the output drawer shows five tabs:

| Tab | What's there |
|---|---|
| **Summary** | Pass/fail, total time, node-by-node status |
| **Results** | The full JSON output from every node |
| **Errors** | Failed nodes with the exact error message |
| **Logs** | Every log line in order. Click a line to highlight that node on the canvas. |
| **History** | All previous runs for this workflow |

**Running a single node:** right-click any node and choose **Run from here**. Aerini runs that node and everything it depends on, but skips everything downstream. Useful for testing one step without triggering side effects like sending emails.

---

## Version history

Every save creates a snapshot. To restore an earlier version:

1. Right-click the workflow in the sidebar → **Versions**
2. Browse by date
3. Click any snapshot to preview, then click **Restore**

---

## Where your data lives

Aerini stores everything locally — no cloud, no account:

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/com.aerini.app/` |
| Windows | `%APPDATA%\com.aerini.app\` |
| Linux | `~/.local/share/com.aerini.app/` |

Two SQLite database files live there: `workflows.db` (your workflows and run history) and `credentials.db` (your encrypted API keys). To fully reset Aerini, delete this entire folder.

---

## Troubleshooting

**The node turned red. What do I do?**
Click the **Errors** tab in the output drawer. It shows the exact error message and which node failed. Common causes: wrong API key, bad URL, or a network issue.

**Code (JS) node says the bundled Node.js runtime is missing or corrupt.**
Aerini ships its own Node.js runtime for this node — it's not your system install, so a system `node --version` check won't help. Reinstall Aerini to restore the bundled runtime.

**Expressions show as blank in the output.**
The expression path is wrong. Check the **Logs** tab — it lists warnings for unresolved expressions, including the path that failed. Use the expression picker (press `{{` in any text field) to browse the correct paths rather than typing them manually.

**I changed a node's name and now other nodes break.**
Renaming a node breaks any expressions that reference it by name. Use the expression picker to rebuild them with the new name.

**The workflow ran once but now does nothing.**
Check that the Background Run is still active in the sidebar. If Aerini was closed and reopened, you may need to restart the background run by clicking **Run in background** again.

---

## What's next

- [Concepts](concepts.md) — plain-English explanations of every core idea in Aerini
- [Nodes Reference](nodes.md) — full documentation for all 39 built-in nodes
- [Expressions](expressions.md) — how to wire node outputs into other fields
- [Credentials](credentials.md) — adding and managing API keys securely
- [Background Runs](background-runs.md) — schedules, webhooks, and run history in depth
