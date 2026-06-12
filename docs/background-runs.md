# Background Runs

A background run is a workflow that keeps executing on its own — on a schedule, or waiting for incoming HTTP requests — while you use Flowo for other things, or while it runs minimized to the system tray.

---

## How it works

When you start a background run, Flowo hands the workflow to a built-in scheduler daemon. The daemon owns the loop: it sleeps until the next scheduled time, runs the workflow, records the result, and starts waiting again. The **Background Runs** section in the sidebar shows live status for every active job.

The daemon runs as long as the Flowo window is open. Closing Flowo stops all background runs. For workflows that need to keep running after the app closes — or 24/7 on a server — see [Server Deployment](server-deploy.md).

---

## Start a background run

Your workflow must have a **Schedule** or **Webhook** trigger as its first node. Workflows with only a Manual Trigger can't run in the background.

1. Confirm the first node is a Schedule or Webhook node, configured as you want it.
2. Save the workflow (`Ctrl+S`).
3. Click the **Run in background** button in the toolbar (the play button with a clock icon).

Within a few seconds the job appears in the **Background Runs** sidebar section with a pulsing green dot.

---

## Schedule trigger

The Schedule node has three modes:

### Interval

Runs every N seconds.

```
mode: interval
interval_secs: 3600    # every hour
```

Minimum interval is 10 seconds.

### Cron

Runs on a cron schedule. Uses standard 5-field cron syntax: `minute hour day-of-month month day-of-week`.

```
mode: cron
cron_expr: 0 9 * * 1-5    # 9 AM Monday–Friday
```

Common expressions:

| Expression | When it runs |
|---|---|
| `* * * * *` | Every minute |
| `0 * * * *` | Every hour, on the hour |
| `0 9 * * *` | Daily at 9 AM |
| `0 9 * * 1-5` | Weekdays at 9 AM |
| `0 9 1 * *` | Monthly on the 1st at 9 AM |
| `*/15 * * * *` | Every 15 minutes |

### Once

Runs at a specific date and time, then stops.

```
mode: once
run_at: 2025-03-01T09:00:00Z
```

The timestamp must be in ISO 8601 format with a UTC offset (`Z` for UTC, or `+00:00`, etc.).

---

## Webhook trigger

The Webhook node listens for an incoming HTTP request, then runs the workflow using that request's data as input.

```
port: 3456        # must be ≥ 1024
path: /webhook
method: POST
secret: my-secret # optional but recommended
timeout_secs: 60
```

When a background run is active, the webhook listener stays open. Any matching request triggers a run.

**Important:** in the desktop app, the webhook binds to `127.0.0.1` — your own machine only. External services like GitHub, Stripe, or Twilio cannot reach it over the internet. To receive webhooks from the internet, you have two options:

- **For testing:** use a tunnel tool like cloudflared or ngrok to expose your local port. See [Exposing Webhooks to the Internet](webhooks-public.md).
- **For production:** deploy to a server. See [Server Deployment](server-deploy.md).

### Secrets

If you set a `secret`, incoming requests must include an `X-Flowo-Secret` header matching that value. The check is timing-safe (it can't be bypassed by measuring response time). Always set a secret for any webhook that triggers sensitive actions — without it, anyone who discovers the port can trigger your workflow.

Always set `timeout_secs` too. Without it, a sender that connects but never sends data can block the executor indefinitely.

### Test a webhook locally

With the workflow running in the background, fire it from a terminal on the same machine:

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

Replace `3456` and `/webhook` with your configured port and path. The run should appear in the Background Runs sidebar and in the output drawer shortly after.

---

## Manage background runs

The **Background Runs** sidebar section lists all active and recently completed jobs. Each entry shows:

- Status dot: green (running or waiting), grey (stopped), red (failed)
- Workflow name
- Number of completed runs
- Next scheduled run time (interval and cron workflows)

**Stop a job:** click the stop button next to the job, or right-click → **Stop**.

**Always On:** right-click a job → **Always On**. The daemon automatically restarts the workflow if it errors or if Flowo restarts. Use this for workflows you want running indefinitely without any manual intervention.

**View run history:** click the job name to open the output drawer on the History tab. Click any past run to see its full Summary, Results, and Logs.

---

## Run history and the "interrupted" status

Every run that's recorded in the History tab has a status: **running**, **success**, **failed**, or **interrupted**.

- **running** — the run is in progress right now. Flowo writes this entry the moment a run starts, before any node has executed, so the History tab shows it immediately rather than only after the workflow finishes.
- **success** / **failed** — the run finished. This is what you'd expect to see for almost every entry.
- **interrupted** — the run never got to finish, and not because the workflow itself failed. This happens if Flowo (or `flowo-server`) was closed, crashed, or had its power cut while the run was still in progress. When Flowo starts back up, it checks for any run still marked "running" from before — since nothing actually crashed *during* this new session, that old entry obviously didn't complete normally, so it gets relabeled "interrupted" rather than left looking like it's still running forever.

If you see "interrupted" in the history, it means: that specific run was cut short by the app closing, not by an error in your workflow logic. The Logs tab for that entry will show whatever happened up to the point of interruption, but won't show a final result — there isn't one. If this happens regularly for a workflow you need running continuously, see [Server Deployment](server-deploy.md) for 24/7 execution with proper restart handling, and the [graceful shutdown](execution-flow.md#10-graceful-shutdown) behavior of `flowo-server`, which avoids interruptions during normal restarts.

---

## Retry behavior

If a node fails and its `max_attempts` is greater than 1, the executor retries that node. This matters for nodes with side effects:

- An email node with `max_attempts: 3` may send up to 3 emails if the first two fail transiently.
- A Stripe node may create multiple payment intents.

The default `max_attempts` is `1` (no retry). Only raise it on operations that are safe to run multiple times — HTTP GET requests, database reads, AI prompts. For nodes with side effects, handle failures explicitly using the `on_error` port instead.

The `on_error` port is a secondary output port on every node. Hover the node on the canvas to see it appear on the right edge. Drag from it to a downstream node — a Stop node with a reason, a Slack alert, a Send Email notification — to route failure gracefully rather than stopping the whole workflow.

Full retry reference: [Nodes Reference — Per-node retry and error routing](nodes.md#per-node-retry-and-error-routing)

---

## Port conflicts

Two webhook workflows can't listen on the same port. If a second workflow tries to start on a port already in use, it fails with a port conflict error that names the workflow holding that port. Stop that workflow first, or change the port in one of them.

---

## Importing workflows from n8n

Flowo can import n8n workflow JSON files directly. If you've been using n8n and want to move your automations over, this is the fastest path.

### How to import

1. In n8n, open the workflow and click **Download** (or copy the workflow JSON from the editor).
2. In Flowo, press `Ctrl+N` to create a new workflow (so you're not accidentally overwriting an existing one).
3. Go to **File → Import**, or drag-and-drop the `.json` file onto the canvas.
4. Flowo shows a preview with the workflow name, node count, and a compatibility check. Each node type is marked green (supported) or amber (not recognized).
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

**Check amber nodes.** Any node marked amber was imported as an "unsupported" placeholder. It appears on the canvas but won't run — you'll need to replace it with a different approach.

**Re-add credentials.** n8n credentials don't transfer over. Open each node that needs a key and assign one from the Connections panel.

**Test step by step.** Right-click each node and choose **Run from here** to test it in isolation before running the full workflow. This catches problems early without triggering downstream side effects.

---

## Troubleshooting

**The background run stopped and I don't know why.**
Click the job in the sidebar to open the run history. The most recent entry shows whether it succeeded or failed, and the Logs tab shows exactly what happened.

**The scheduler ran at the wrong time.**
Cron expressions in Flowo are evaluated in UTC. If you expected 9 AM in your local timezone but the run fired at a different time, adjust your cron expression to account for the UTC offset. For example, if you're UTC-5, use `0 14 * * *` for 9 AM local time.

**The webhook fires but the workflow doesn't seem to run.**
Check that the method configured in the Webhook node (`GET`, `POST`, etc.) matches the method your sender is using. Also verify the secret header matches exactly, including case. Check the Background Runs sidebar — if the job shows a red dot, open it to see the error.

**"Previous run still in progress" appears in the logs.**
Flowo only runs one instance of a workflow at a time. If a run takes longer than your schedule interval, the next scheduled run is skipped with this log message. Either reduce the interval, increase the timeout on slow nodes, or investigate why the workflow is running slowly.
