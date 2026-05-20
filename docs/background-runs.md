# Background Runs

Background runs let a workflow execute on a recurring schedule or wait for a webhook — while you keep using the desktop app for other things, or while the app is minimized to the system tray.

## How it works

When you start a background run, Flowo hands the workflow to a scheduler daemon that runs inside the Tauri process. The daemon owns the loop: it sleeps the configured interval, wakes up, runs the workflow through the same executor as a manual run, and goes back to sleep. The **Background Runs** section in the sidebar shows live status for all active jobs.

The daemon runs as long as the Flowo window is open. When you close Flowo, background runs stop. For runs that need to survive app restarts or run 24/7, export the workflow and deploy it to a server — see [Server Deployment](server-deploy.md).

## Start a background run

A workflow must have a **Schedule** or **Webhook** trigger node as its entry point. Workflows with only a Manual Trigger can't be scheduled.

1. Make sure the first node is a Schedule or Webhook node, configured as you want it.
2. Save the workflow (`Ctrl+S`).
3. Click the **Run in background** button in the toolbar (the play-with-clock icon).

Flowo validates the workflow, saves it, and hands it to the daemon. Within a few seconds the job appears in the **Background Runs** sidebar section with a pulsing green dot.

## Schedule trigger

The Schedule node supports three modes:

**Interval** — runs every N seconds.

```
mode: interval
interval_secs: 3600    # every hour
```

**Cron** — runs on a cron schedule. Standard 5-field cron syntax:

```
mode: cron
cron_expr: 0 9 * * 1-5    # 9 AM Monday–Friday
```

Common expressions:

| Expression | When |
|---|---|
| `* * * * *` | Every minute |
| `0 * * * *` | Every hour |
| `0 9 * * *` | Daily at 9 AM |
| `0 9 * * 1-5` | Weekdays at 9 AM |
| `0 9 1 * *` | Monthly on the 1st at 9 AM |
| `*/15 * * * *` | Every 15 minutes |

**Once** — runs at a specific time, then stops.

```
mode: once
run_at: 2025-03-01T09:00:00Z
```

## Webhook trigger

The Webhook node listens for an incoming HTTP request, then runs the rest of the workflow with the request data as input.

```
port: 3456        # must be ≥ 1024
path: /webhook
method: POST
secret: my-secret # optional
timeout_secs: 60
```

When the workflow is backgrounded, the webhook listener stays open. Any matching request fires a run. In the desktop app the webhook binds to `127.0.0.1` (your own machine only) — it is **not** reachable from other computers or the internet. If you need the webhook to be reachable from the internet (e.g. for GitHub webhooks or payment callbacks), deploy to a server instead — see [Server Deployment](server-deploy.md).

If you set a `secret`, incoming requests must include an `X-Flowo-Secret` header matching that value. The comparison is timing-safe (no timing attacks). **Always set a secret for any webhook that handles sensitive actions** — without it, anyone who discovers the port can trigger your workflow.

Always set `timeout_secs`. Without it, a sender that connects but never sends data will block the workflow executor until the server restarts.

### Testing a webhook locally

With the webhook workflow running in the background, you can fire it from your own computer's terminal:

```bash
# Basic POST — no secret
curl -X POST http://127.0.0.1:3456/webhook \
  -H "Content-Type: application/json" \
  -d '{"test": "hello"}'

# POST with a secret header
curl -X POST http://127.0.0.1:3456/webhook \
  -H "Content-Type: application/json" \
  -H "X-Flowo-Secret: my-secret" \
  -d '{"event": "test", "data": {"key": "value"}}'
```

Replace `3456` with the port you configured, and `/webhook` with the path you set. After sending, the run should appear in the Background Runs sidebar and in the output drawer.

## Manage background runs

The **Background Runs** sidebar section shows all active and recently completed jobs. Each job shows:

- Status dot: green (running/waiting), grey (stopped), red (failed)
- Workflow name
- Run count
- Next scheduled run time (for interval/cron workflows)

**Stop a job:** click the stop button next to the job, or right-click → **Stop**.

**Always On:** right-click a job → **Always On**. The daemon restarts the workflow automatically if it errors or if Flowo restarts. Use this for workflows you want running indefinitely without manual intervention.

**View run history:** click the job name to open the output drawer at the History tab. Browse past runs, click any entry to see the full Summary/Results/Logs for that run.

## Port conflicts

If two webhook workflows try to listen on the same port, the second one fails to start with a port conflict error. The error message names the workflow that's holding the port. Stop that workflow first, or change the port on one of them.

## Retry behavior

If a node fails and has `max_attempts > 1` configured, the executor retries that node. Nodes with side effects fire on every retry:

- An email node with `max_attempts: 3` may send up to 3 emails on transient failure.
- A Stripe node may create multiple payment intents.

Default `max_attempts` is `1` (no retry). Only set retries > 1 on idempotent nodes: HTTP GET requests, database reads, AI prompts. For nodes with side effects, handle failures explicitly using the `on_error` port.

The `on_error` port is a secondary output port on every node. When wired, a failed node routes execution to that branch instead of stopping the workflow. To wire it, hover the node on the canvas — the `on_error` port appears on the right edge. Drag from it to a downstream node (a Stop node, a Slack alert, or a Send Email to notify you of the failure).

See [Nodes Reference — Per-node retry and error routing](nodes.md#per-node-retry-and-error-routing) for full details.

---

## Importing workflows from n8n

Flowo can import n8n workflow JSON. If you paste a valid n8n workflow, Flowo detects the format automatically and converts it.

### How to import

1. In n8n, open the workflow and choose **Download** (or copy the workflow JSON from the editor).
2. In Flowo, press `Ctrl+N` for a new workflow (so you're not overwriting anything).
3. Go to **File → Import** (or drag-and-drop the `.json` file onto the canvas).
4. Flowo shows a preview modal with the workflow name, node count, and a compatibility check — each node type is marked green (supported) or amber (not recognized).
5. Click **Import**.

### Supported node mappings

| n8n node | Flowo node |
|---|---|
| `manualTrigger` | Manual Trigger |
| `webhook` | Webhook |
| `scheduleTrigger` | Schedule |
| `httpRequest` | HTTP Request |
| `code` / `function` | Code (JS) |
| `if` | If / Condition |
| `switch` | Switch |
| `emailSend` / `gmail` | Send Email |
| `readWriteFile` | File |
| `set` | Set Variable |
| `merge` | Merge |
| `slack` | Slack |
| `googleSheets` | Google Sheets |
| `postgres` / `mySql` | Database |
| `github` | GitHub |
| `notion` | Notion |
| `discord` | Discord |
| `telegram` | Telegram |

### After importing

- **Check amber nodes.** Any node marked amber in the preview was imported as `unsupported` — it's a placeholder on the canvas. You'll need to replace it with a different approach.
- **Re-add credentials.** n8n credentials don't transfer. Open each node that needs a credential and assign one from your Connections panel.
- **Test with a single-node run.** Right-click each node and run it in isolation before running the full workflow.

n8n workflows carry their original canvas positions, so the layout should look familiar after import. The config values from n8n's `parameters` object are carried over directly — how well they map to Flowo's parameter names varies by node type.
