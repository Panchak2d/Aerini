# Embedding the chat widget

`aerini-server` ships a single-file, zero-dependency JS widget
(`aerini-widget.js`) that drops a chat bubble onto any web page and talks to
one of your Webhook-triggered workflows. This guide covers the script tag,
its data attributes, how the relay works, and the security tradeoffs you're
accepting by using it.

If you're looking for the Rust embedding API (running `aerini-engine` inside
your own application) see [Embedding aerini-engine](embedding.md) instead —
this doc is about the browser-facing chat widget only.

> **Check your license before deploying this on a commercial site.** Serving
> the widget means running `aerini-server` as a service for your site's
> visitors — the scenario [embedding.md's licensing section](embedding.md#licensing)
> covers. This isn't legal advice either way; it's a pointer to go read that
> section and the [pricing page](https://panchak2d.github.io/aerini/pricing)
> before you decide.

---

## What you need first

The widget targets a **Webhook-triggered workflow** that's currently running
on `aerini-server` (status `active` in the scheduler). A typical shape is:

```
Webhook (trigger) → AI Memory → AI Prompt → Output
```

The Webhook node needs a **Secret** configured — the widget can't work
without one (see [Security note](#security-note-both-secrets-are-visible-in-page-source)
below for why that matters).

---

## The script tag

```html
<script
  src="https://YOUR_SERVER/aerini-widget.js"
  data-server="https://YOUR_SERVER"
  data-workflow-id="wf_abc123"
  data-secret="the Webhook node's configured secret"
  data-token="a read-scoped, single-workflow-ACL'd API token"
  data-session-id="optional — generated and persisted in localStorage if omitted"
  data-allow-images="true"
  data-max-length="2000"
  data-theme="light"
></script>
```

Drop this anywhere in the page's HTML — the widget renders its own floating
bubble and chat window, scoped under a prefixed CSS namespace so it can't
collide with your page's styles.

### Data attributes

| Attribute | Required | Default | What it does |
|---|---|---|---|
| `data-server` | Yes | — | Base URL of your `aerini-server` instance (no trailing slash needed) |
| `data-workflow-id` | Yes | — | The workflow's ID. Find it in the desktop app or via `GET /api/workflows` |
| `data-secret` | Yes | — | The target workflow's Webhook node secret. Forwarded to the relay on every message |
| `data-token` | No | — | A read-scoped API token, ACL'd to this one workflow. Without it the widget can send messages but can't display replies (see below) |
| `data-session-id` | No | random UUID, persisted in `localStorage` | Lets you control session continuity yourself (e.g. tie it to your own logged-in user ID) |
| `data-allow-images` | No | `true` | Whether to render image replies from `media_batch` outputs inline |
| `data-max-length` | No | `2000` | Max characters per outgoing message |
| `data-theme` | No | `light` | `light` or `dark` |

If `data-server`, `data-workflow-id`, or `data-secret` is missing, the widget
logs an error to the console and doesn't render at all.

---

## How a message actually gets there: the relay endpoint

A browser can't POST straight at the Webhook node's listener port — it has
no CORS headers and no `OPTIONS` handling, so any cross-origin request gets
blocked before it leaves the browser. The widget instead POSTs to a relay on
the main API server:

```
POST /api/widget/:workflow_id/trigger
Content-Type: application/json

{
  "secret": "...",
  "body": { "message": "...", "session_id": "..." }
}
```

This route is intentionally registered **without** Bearer-token auth — a
widget embedded on a public page can't hold a server admin/write token
without exposing it to every visitor. Instead, the relay forwards your
`secret` verbatim to the workflow's own Webhook listener over loopback, and
the Webhook node's existing constant-time secret check is the only thing
that decides whether the request is accepted. The relay itself never
inspects or compares the secret.

The relay's HTTP response only confirms the message was accepted — it does
**not** wait for or return the workflow's actual output (the Webhook node
always acks immediately, before the workflow body runs). The reply arrives
separately, as described next.

---

## How the reply comes back: SSE + the scoped token

The widget opens a Server-Sent Events connection to:

```
GET /api/events?workflow_id=wf_abc123
Authorization: Bearer <data-token>
```

and waits for a `scheduler-status` event carrying the workflow's output. This
is why `data-token` is separate from `data-secret`: the trigger relay needs
no auth (see above), but *reading* run output back over `/api/events`
does — otherwise anyone could subscribe to every workflow's event stream on
your server.

**Create a token scoped to exactly this workflow:**

```bash
# 1. Create a read-only token (requires an admin-scoped token to call this)
curl -X POST https://YOUR_SERVER/api/tokens \
  -H "Authorization: Bearer <your-admin-token>" \
  -H "Content-Type: application/json" \
  -d '{"label": "chat-widget-wf_abc123", "scopes": ["read"]}'
# → { "token": "...", "note": "Save this token — it will not be shown again." }

# 2. Restrict it to this one workflow's events
curl -X POST https://YOUR_SERVER/api/tokens/<token_id>/workflows/wf_abc123 \
  -H "Authorization: Bearer <your-admin-token>"
```

Without step 2, a `read`-scoped token still sees **every** workflow's SSE
events on your server, not just this one. Granting at least one workflow ACL
entry restricts it to only those granted workflows. Use the token from step
1's response (not the admin token) as `data-token`.

If you omit `data-token` entirely, the widget still sends messages
successfully — it just shows a banner saying replies are unavailable, since
it has no way to authenticate the SSE subscription.

---

## Security note: both secrets are visible in page source

`data-secret` and `data-token` are both plain HTML attributes — anyone who
opens your browser's dev tools, or views page source, can read them. This is
true by construction for any pure client-side embed; there's no script-tag
mechanism that hides an attribute from the page that loaded it.

What limits the damage if someone copies these values:

- **The secret** only lets someone trigger the same workflow your widget
  already lets any visitor trigger via the chat box. It does not grant
  access to other workflows, credentials, or the server's admin API.
- **The token** is `read`-scoped and ACL'd to one workflow's SSE events
  only (if you followed the steps above) — it can't create, edit, or delete
  anything, and can't read other workflows' events.

What this setup does **not** protect against:

- Someone scripting requests directly to `/api/widget/:workflow_id/trigger`
  to run the workflow outside your page, bypassing your UI, rate limits, or
  any client-side validation you've added.
- Someone using the leaked token to read that workflow's run history over
  SSE indefinitely, until you revoke it (`DELETE /api/tokens/:id`).

If the workflow this widget triggers does anything sensitive (sends real
emails, hits paid APIs, writes to a database), put your own rate limiting or
abuse detection in front of `/api/widget/:workflow_id/trigger` at the reverse
proxy — the widget and relay don't provide any on their own. Treat the
secret and token the same way you'd treat any value you're knowingly putting
in front of the public.
