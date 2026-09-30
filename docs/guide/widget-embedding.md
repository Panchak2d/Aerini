# Widget Embedding

The embeddable chat widget drops a small floating chat window onto any web page, backed by one of your Aerini workflows. This page covers what the workflow needs to look like, how to add the widget to a page, how a message actually travels from the visitor's browser to your workflow and back, and the tradeoffs worth knowing before you put this on a page other people can reach. It assumes you've read [Credentials](credentials.md) and [Security](security.md); most of what makes the widget safe or risky to expose is the token and secret model those two pages already cover, applied here to one specific feature.

## Before you start

The widget only works with a workflow shaped a specific way, already running in the background:

- The workflow's trigger has to be a **Webhook** node, not Schedule, Manual, or a plugin trigger. See [Nodes Reference §Webhook](nodes.md#webhook) for its fields.
- The workflow needs an **Output** node, so the widget has something to show back to the visitor. See [Nodes Reference §Output](nodes.md#output).
- The workflow has to already be running as a background job: either **Schedule Run** in the desktop app, or started through `aerini-server`'s `api` mode (`POST /api/scheduler/:id/start`). A workflow that's saved but not started shows an error the moment someone opens the widget; see [What each error means](#what-each-error-means) below.

The widget itself is only served by `aerini-server` running in **`api` mode**. `aerini-server serve`, the single-workflow mode covered in [Background Runs §Beyond the desktop app](background-runs.md#beyond-the-desktop-app), doesn't have it. Setting up `api` mode is covered in [Server Deployment](../operations/server-deploy.md).

## Embedding the script

Add one script tag to the page you want the widget on:

```html
<script
  src="https://your-server.example.com/aerini-widget.js"
  data-server="https://your-server.example.com"
  data-workflow-id="wf_abc123"
  data-secret="a short-lived token from /api/widget/wf_abc123/mint-token — see below"
  data-token="a read-scoped token, ACL'd to this one workflow"
></script>
```

That's the whole setup. The script injects a floating chat bubble and its window on its own; there's no separate element to add or function to call. `data-secret` can also be the Webhook node's raw secret directly — see [How a message reaches the workflow](#how-a-message-reaches-the-workflow) for the tradeoff between the two before picking one.

| Attribute | Required | What it does |
|---|---|---|
| `data-server` | Yes | Base URL of your `aerini-server` instance. |
| `data-workflow-id` | Yes | The workflow's ID. |
| `data-secret` | Yes | Either the Webhook node's configured `secret` field, or a short-lived signed token minted from it. See [How a message reaches the workflow](#how-a-message-reaches-the-workflow) — read that before deciding which one to put here. |
| `data-token` | No, but replies won't arrive without one | A token used only to read this workflow's reply. See [Getting the reply back](#getting-the-reply-back). |
| `data-session-id` | No | A stable ID grouping one visitor's messages, and used server-side to keep one visitor's replies from reaching another visitor's browser. See [Getting the reply back](#getting-the-reply-back). Generated with the browser's Web Crypto API and kept in local storage if you leave it out; a browser without Web Crypto can't generate one, so the widget doesn't start there unless you set this. |
| `data-allow-images` | No | `"true"` by default. Set `"false"` to always render replies as text, even when the Output node's value is image data. |
| `data-max-length` | No | Character limit on a single message, enforced before it's sent. Default 2000. |
| `data-theme` | No | `"light"` (default) or `"dark"`. |

Leave out any of the three required attributes and the widget doesn't appear at all. It logs an error to the browser console and stops before adding anything to the page.

## How a message reaches the workflow

A widget embedded on a page hosted somewhere else can't call the Webhook node's listener directly. That listener always binds to `127.0.0.1`, the server's own loopback address, the same restriction [Nodes Reference §Webhook](nodes.md#webhook) and [Background Runs](background-runs.md#how-webhook-behaves-in-the-background) already describe: nothing outside the machine running Aerini can reach it without something in front of it.

The widget's relay endpoint, `POST /api/widget/:workflow_id/trigger`, is that something. It runs on the same server as the rest of the API, the one the browser already reached to load the widget script in the first place, and forwards the request over that server's own loopback interface to the port the workflow's Webhook trigger is actually bound to right now.

This relay endpoint doesn't require a bearer token the way every other API route does. A widget sitting in a third-party page's source can't hold a real admin or write token without handing it to every visitor, so it's authenticated a different way: whatever's in `data-secret`, sent as the `x-webhook-secret` header, the same header a webhook provider like Stripe or GitHub would send. The relay doesn't check this itself; it forwards the value verbatim, and the Webhook listener validates it exactly the way it validates any other incoming request — accepting either form below.

**Two ways to fill in `data-secret`, with a real security difference between them:**

- **The raw secret, directly.** Simplest to set up — just paste the Webhook node's `secret` field into the script tag — but it's visible to anyone who views the page source, forever, until you rotate it. Fine for something low-stakes or internal. Not recommended for a public production page.
- **A short-lived signed token, recommended for production.** Your own backend — which already has the real secret, since it's the same one you'd otherwise paste directly — exchanges it for a token that expires on its own, server-side, immediately before rendering the page:

  ```bash
  curl -X POST https://your-server.example.com/api/widget/wf_abc123/mint-token \
    -H "Content-Type: application/json" \
    -d '{"secret": "<the Webhook node secret>", "ttl_secs": 300}'
  ```

  This returns `{"token": "awh1.<expiry>.<signature>", "expires_at": "...", "ttl_secs": 300}`. Put that `token` value in `data-secret` instead of the raw secret — the widget doesn't need to know the difference, it just forwards whatever's there. **Call `mint-token` from your server when rendering the page, never from the visitor's browser** (the server enforces this for browsers: it never grants any origin, `--allow-origin` included, cross-origin access to `mint-token`) — the raw secret has to stay server-side for this to mean anything; if the browser calls it directly you've just moved the same standing exposure to a different endpoint. A captured token still lets its holder trigger the workflow, exactly like the raw secret does, but only until `expires_at` — after that it's dead regardless of who has it, which the raw secret never is on its own. `ttl_secs` is clamped to a 24-hour ceiling server-side no matter what you request; pick the shortest value that comfortably outlives one page view.

Neither form cryptographically signs the message body, so even a token in active use doesn't stop a captured request from being replayed verbatim until it expires. If you need that too, see [Nodes Reference §Webhook](nodes.md#webhook)'s note on verifying a platform HMAC header downstream — the same mechanism applies here.

`mint-token`, like `trigger`, is rate-limited per caller IP: it's a secret-guessing oracle in the same way the trigger endpoint is (a wrong secret gets a 401, a right one a 200), so treat 429s from it the same way — back off, don't retry in a tight loop.

## Getting the reply back

The trigger call's own response is not the workflow's answer, only an acknowledgement that the request reached the Webhook listener. The actual reply arrives separately, as an event on the server's live event stream, Server-Sent Events (SSE): a plain HTTP connection the server keeps open and pushes updates through. The widget opens that connection at `GET /api/events` on page load and waits for a `scheduler-status` event carrying this workflow's result once the run finishes.

Reading that stream needs a bearer token, same as any other authenticated route, which is what `data-token` is for. Without one, the widget can still send messages, but every one of them times out on the browser side after 30 seconds with no way to show a reply, and a banner in the chat window tells the visitor replies aren't available.

Create that token from an account with `admin` scope, scoped to this one workflow in the same call; see [Security §Hardening a server deployment](security.md#hardening-a-server-deployment) for what scope means here:

```bash
curl -X POST https://your-server.example.com/api/tokens \
  -H "Authorization: Bearer <your admin token>" \
  -H "Content-Type: application/json" \
  -d '{"label": "widget-reader", "scopes": ["read"], "workflow_ids": ["wf_abc123"]}'
```

Save the token from that response, it's only shown once. `workflow_ids` restricts it to exactly the workflows listed, granted atomically in this same call — leave it out (or empty) and a `read`-scoped token can see every workflow's events on the server, not just the one you're embedding. There's no reason to leave it out for a widget token.

`data-token` is embedded in page HTML the same as `data-secret` is, so every visitor of the page shares that one token. On its own that would mean one visitor's browser receives every other visitor's replies too — the SSE stream is per-workflow, not per-visitor. `data-session-id` is what prevents that: the widget includes it on every connection, and the server only forwards a reply to the connection whose session matches the one that sent it. This only filters the widget's own rendering path — it doesn't change what the token itself is scoped to, so treat `data-token` as "can read this workflow's results," not "can read this visitor's results," when deciding how it's stored and how often it's rotated.

## Allowing your origin through CORS

Local origins, `http://localhost` and `http://127.0.0.1` on any port, can always call the server. Anything else, meaning the actual domain your embedding page is served from, needs to be added explicitly with `--allow-origin`, one of `aerini-server api`'s startup flags:

```bash
aerini-server api --token <your token> --allow-origin https://your-site.example.com
```

Cross-Origin Resource Sharing (CORS) is the browser's own check on whether a page is allowed to call a server it wasn't loaded from. It's enforced by the browser, not by Aerini, but the server still has to explicitly say yes for the browser to allow it through. Skip this flag and the widget's trigger call fails before it ever reaches Aerini's server code: the browser blocks the request itself, and the widget falls back to showing whatever generic message the browser's own failed network call carried, something like "Failed to fetch," worded differently depending on the browser. See [Server Deployment](../operations/server-deploy.md) for the rest of `api` mode's startup flags.

## What each error means

Everything below assumes CORS already allowed the request through and Aerini's server actually received it. A request the browser blocks for CORS never reaches that far; see the note above for what the widget shows instead in that case.

| What the widget shows | What actually happened |
|---|---|
| "workflow is not scheduled: start it first" | No background job exists yet for this workflow ID at all. Start it with Schedule Run in the desktop app, or `POST /api/scheduler/:id/start` in `api` mode. |
| "workflow is not running (status: stopped)" (or `paused`, `done`, `error`) | A job exists but isn't currently active. Same fix: start or restart it. |
| "workflow's trigger is '...', not Webhook: the embeddable widget only works with a Webhook-triggered workflow" | The workflow's trigger is something other than Webhook. The message names which one (Interval, Cron, Once, Manual, or Plugin); only Webhook works with the widget. |
| "workflow definition not found" | The scheduled job exists but its saved workflow definition doesn't — most often a workflow deleted without stopping its job first. Stop the job, or point the widget at a workflow ID that still exists. |
| "this workflow contains a Shell Command, Code, or Database node — the public widget relay refuses to trigger it, regardless of the server's --allow-shell/--allow-code/--allow-database flags" | The target workflow contains one of those node types. The relay refuses unconditionally, before it ever reaches the workflow's listener — see [Security tradeoffs](#security-tradeoffs-to-know-before-you-embed-this). |
| "could not reach the workflow's webhook listener, it may have just stopped" | The job looked active a moment ago, but the loopback call to it failed anyway, most often because it stopped in between. Try again. |
| "the secret did not match the Webhook node's configured secret" | `data-secret` doesn't match the Webhook node's `secret` field, or a signed token in `data-secret` has expired or was minted for a different workflow's secret. |
| "the request path or HTTP method did not match the Webhook node's configuration" | The Webhook node's `path` doesn't match what the relay is calling, or its `method` isn't `POST` or `ANY`. The widget only ever sends POST. |
| "message too large" | The message body exceeded the Webhook listener's own 1 MB cap, far above `data-max-length`'s default, but reachable if you raised that default a lot. |
| "the webhook listener rejected the request" | A catch-all for anything else the listener refused: a malformed request, or one that took too long partway through. Usually clears on retry. |
| 429 Too Many Requests, from `trigger` or `mint-token` | The per-IP or per-workflow rate limit on that endpoint was hit. See [Security tradeoffs](#security-tradeoffs-to-know-before-you-embed-this). Back off; don't retry immediately. |

The first six come straight from the relay endpoint itself; the rest are the relay translating whatever the Webhook listener told it. Either way, the widget shows the message as an error bubble in the chat window, not a silent failure.

## Security tradeoffs to know before you embed this

`data-secret` and `data-token` both sit in plain HTML on whatever page you embed the widget on. Anyone who views that page's source has both. There's no way to keep a value truly secret once it ships to a browser — that's inherent to any credential embedded in client-side code, not particular to Aerini. What that means in practice, and what's done about each:

- **Direct triggering / replay.** Anyone with the page source can call `trigger` directly, without going through the widget UI, and can replay a captured request exactly as sent — neither the raw secret nor a signed token cryptographically binds to the request body. Two things bound this rather than eliminate it: use a signed token (see [How a message reaches the workflow](#how-a-message-reaches-the-workflow)) so a captured credential expires instead of working forever, and rely on the rate limits below to cap how much damage a captured credential — expired or not — can do in the meantime. `trigger` is limited to 20 requests/minute per caller IP and 60/minute per workflow (whichever is hit first applies); `mint-token` is limited to 10/minute per caller IP, tighter because a wrong guess there costs the caller nothing but a 401, the same shape of risk as a login form. Both return `429 Too Many Requests` once hit. If your workflow does something that costs real money or resources per run (an LLM call, a paid API, a database write), the rate limit is a ceiling on that cost from this endpoint, not a guarantee — size it against what you'd tolerate at the limit, not against expected normal traffic.
- **Reply token scope.** Scope the reply token to this one workflow — `workflow_ids` in the `POST /api/tokens` call above, done in the same request as creation now, not a separate step to remember. An unrestricted token in page source can read every workflow's run results on the server, not just this one.
- **Cross-visitor reply leakage.** Fixed as of this doc: `data-session-id`, described above, keeps one visitor's SSE connection from receiving another visitor's reply. Set it explicitly (rather than relying on the auto-generated one) only if you have a reason to — e.g. syncing it with your own site's session — since the auto-generated one already does the job.
- **Dangerous nodes.** Shell Command, Code, and Database nodes are disabled server-wide by default — `aerini-server api` refuses to execute any of them unless started with `--allow-shell`, `--allow-code`, or `--allow-database` respectively (see [Server Deployment](../operations/server-deploy.md)). Those flags are server-wide, not per-workflow: enabling one for an unrelated internal workflow would otherwise also unlock that node type for anything else the same server process runs. The widget relay closes that gap for itself specifically: `POST /api/widget/:workflow_id/trigger` checks the target workflow's own nodes before relaying anything, and refuses with `403` if it contains a Shell Command, Code, or Database node — regardless of which `--allow-*` flags happen to be set, and before the request ever reaches the workflow's listener. This check is scoped to the public, unauthenticated widget relay; a dangerous-node workflow triggered through an authenticated Bearer-token route is still governed only by the `--allow-*` flags, same as ever. See [Security §Dangerous nodes](security.md#dangerous-nodes) for what the confirmation dialog looks like for an interactive run — it never appears for a background-triggered one, widget included, the same as any other scheduled or webhook fire; the `--allow-*` flags (and, for this one route, the check above) are the actual enforcement, the dialog is a separate, interactive-only nicety.

Treat a publicly embedded widget the same way you'd treat any other public form: rate-limited and time-boxed by the above, but still something an anonymous visitor can set off. Fine for something you'd be comfortable with anyone triggering; the mitigations above narrow the blast radius, they don't remove the fact that it's public.

## See also

- [Chat Panel](chat-panel.md), for the desktop-only equivalent of this feature — same workflow shape, no secret or token needed since it never leaves the machine
- [Nodes Reference](nodes.md#webhook), for the Webhook node's full field table and the Output node's fields
- [Background Runs](background-runs.md#how-webhook-behaves-in-the-background), for what it means for a Webhook-triggered workflow to be running in the background
- [Credentials](credentials.md), for the encrypted credential store, a separate mechanism from the secret and token this page covers
- [Security](security.md#hardening-a-server-deployment), for scoped tokens, `--allow-origin`, and the rest of hardening an `aerini-server api` deployment
- [Server Deployment](../operations/server-deploy.md), for the rest of running `aerini-server api` itself
