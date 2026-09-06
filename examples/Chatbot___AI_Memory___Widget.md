# Chatbot with AI Memory and Widget

A six-node workflow that runs a persistent chatbot: receives a message from the embedded chat widget, retrieves the conversation history from AI Memory, sends history + message to an AI model, stores both sides of the new exchange, and returns the response to the widget.

---

## Workflow structure

```
Webhook
   │
   ▼
AI Memory (read)
   │
   ▼
AI Prompt
   │
   ├───────────────────────┐
   ▼                        ▼
AI Memory (append user)   Output
   │
   ▼
AI Memory (append assistant)
```

**Why two AI Memory writes instead of one:** AI Memory's `append` operation stores exactly one `role`/`content` pair per call — there's no single call that writes both a user turn and an assistant turn at once, and `write` (as opposed to `append`) replaces the session's entire history rather than adding to it. So one exchange needs two sequential `append` calls: user message first, then the assistant's reply, chained one after the other so they can't race each other's sequence numbers. This branch runs after AI Prompt but doesn't need to finish before the reply is shown, so it runs in parallel with the **Output** branch — both wired directly from AI Prompt's Success output.

The embeddable widget requires the workflow's trigger to be a **Webhook** node specifically — Schedule, Manual Trigger, and plugin triggers aren't supported by the widget relay. See [Widget Embedding §Before you start](../docs/guide/widget-embedding.md#before-you-start).

---

## Node configuration

### 1 · Webhook

| Setting | Value |
|---|---|
| **Path** | `/chat` (or any path — this is what the widget relay forwards to internally) |
| **Method** | `POST` |
| **Secret** | A shared secret you choose — required for the widget to authenticate (see the Widget Embedding section below) |

The widget posts JSON `{ "message": "user text", "session_id": "..." }` to this node. Reference the incoming message in downstream nodes as `{{Webhook.output.body.message}}`.

### 2 · AI Memory (read)

| Setting | Value |
|---|---|
| **Operation** | `read` |
| **Session ID** | `{{Webhook.output.body.session_id}}` — or a fixed string (e.g. `default`) for a single shared conversation |
| **Max Messages** | `20` (keeps context from growing unboundedly) |

Output: `messages`, an array of `{role, content}` objects, newest first.

### 3 · AI Prompt

| Setting | Value |
|---|---|
| **Provider** | Your chosen provider (OpenAI, Anthropic, Gemini, Ollama, …) |
| **Model** | e.g. `gpt-5.6`, `claude-sonnet-5`, `gemini-3.6-flash` |
| **System** | Your persona, e.g. `You are a helpful assistant for Acme Corp. Be concise and friendly.` |
| **Prompt** | `Conversation so far (newest first): {{AI Memory (read).output.messages}}\n\nUser: {{Webhook.output.body.message}}` |

AI Prompt has no dedicated "context" or "history" field — only `prompt` and `system` accept text. The retrieved history is interpolated directly into the prompt as a JSON array; the model reads it as text, not as a structured field.

Output: `content` (the model's reply text) — not `text` and not `response`.

### 4 · AI Memory (append user)

| Setting | Value |
|---|---|
| **Operation** | `append` |
| **Session ID** | Same value as the read node — `{{Webhook.output.body.session_id}}` or your fixed string |
| **Role** | `user` |
| **Content** | `{{Webhook.output.body.message}}` |

### 5 · AI Memory (append assistant)

| Setting | Value |
|---|---|
| **Operation** | `append` |
| **Session ID** | Same value as above |
| **Role** | `assistant` |
| **Content** | `{{AI Prompt.output.content}}` |

Wire this node after "AI Memory (append user)" — not in parallel with it — so the user's turn is always stored before the assistant's, and the two calls can't race each other's row ordering.

### 6 · Output

| Setting | Value |
|---|---|
| **Source Node** | `AI Prompt` |
| **Field** | `content` |

Wired directly from AI Prompt's Success output (in parallel with the append branch above). Set **Source Node** explicitly to `AI Prompt` rather than leaving it blank: a blank Source Node falls back to whichever node in the whole workflow *completed most recently*, not necessarily whichever one is wired directly into Output — with a parallel append branch also running off AI Prompt, that fallback could just as easily land on "AI Memory (append assistant)" and show its `{messages, count, session_id}` output instead of the reply text. An explicit Source Node has no such ambiguity: it looks up that one named node's output directly.

---

## Embedding the chat widget

The widget only works with a workflow that's already running as a background job — either **Schedule Run** in the desktop app, or `POST /api/scheduler/:id/start` in `aerini-server api` mode. A saved-but-not-started workflow shows an error the moment someone opens the widget. The widget script itself is only served by `aerini-server api` mode — `aerini-server serve` doesn't have it. See [Widget Embedding](../docs/guide/widget-embedding.md) for the full setup.

Add the script to any HTML page:

```html
<script
  src="https://your-server.example.com/aerini-widget.js"
  data-server="https://your-server.example.com"
  data-workflow-id="wf_abc123"
  data-secret="the Webhook node's Secret field, or a short-lived signed token minted from it"
  data-token="a read-scoped API token, restricted to this one workflow"
></script>
```

| Attribute | Required | Description |
|---|---|---|
| `data-server` | Yes | Base URL of your `aerini-server` instance |
| `data-workflow-id` | Yes | This workflow's ID |
| `data-secret` | Yes | Either the Webhook node's `Secret` field directly, or a short-lived token minted from it via `POST /api/widget/:workflow_id/mint-token` (recommended for a public page) |
| `data-token` | No, but replies won't arrive without one | A `read`-scoped API token, created via `POST /api/tokens` and restricted to this workflow's ID. Used only to receive the reply — it's a separate credential from `data-secret` |
| `data-session-id` | No | Groups one visitor's messages. Auto-generated and stored in the browser's local storage if omitted |

`data-secret` and `data-token` are two different credentials with two different jobs — `data-secret` authorizes *sending* a message (the same secret configured on the Webhook node above), `data-token` authorizes *reading* the reply back. Both are visible in the page's HTML source to anyone who views it.

For the full attribute reference, the message/reply flow, and CORS setup, see [Widget Embedding](../docs/guide/widget-embedding.md).

---

## Security notes

**Credential visibility.** Both `data-secret` and `data-token` sit in plain HTML on any page you embed the widget on — anyone who views the page source has both, permanently, until you rotate them. Prefer a short-lived minted token over the raw Webhook secret for `data-secret` on a public page (see [Widget Embedding §How a message reaches the workflow](../docs/guide/widget-embedding.md#how-a-message-reaches-the-workflow)), and scope `data-token` to this one workflow's ID rather than leaving it unrestricted.

**Recommended mitigations:**
- Rely on the relay's built-in rate limits (20 requests/minute per caller IP on the trigger endpoint) rather than assuming a captured credential is harmless — see [Widget Embedding §Security tradeoffs](../docs/guide/widget-embedding.md#security-tradeoffs-to-know-before-you-embed-this).
- Use session IDs from your own auth system instead of the auto-generated one if you need per-user isolation — pass the authenticated session ID from your backend rather than letting the widget generate one.

**Prompt injection.** User messages flow directly into the AI Prompt's `Prompt` field. A user who can send messages can attempt to redirect the model's behavior via prompt injection. Add a Code node before AI Prompt to strip or sanitize known injection patterns if you're operating in an adversarial environment.

---

## How to import

Workflow templates in Markdown format document the intended structure — use them as a reference while building the workflow by hand in the canvas.

1. Open Aerini and create a new workflow.
2. Add the six nodes above, named exactly as shown (renaming matters — see the naming note below).
3. Configure each node using the settings tables.
4. Wire them as shown in the workflow diagram: Webhook → AI Memory (read) → AI Prompt, then AI Prompt branching to both AI Memory (append user) → AI Memory (append assistant), and separately to Output.
5. Start the workflow as a background job (Schedule Run, or the `api`-mode start endpoint) — the widget won't connect to a workflow that isn't running.
6. Copy the embed snippet above into your HTML page, filled in with your server URL, workflow ID, secret, and token.

**On node names:** expressions like `{{AI Memory (read).output.messages}}` match a node by its exact name. Two nodes left with the same default name (e.g. both called "AI Memory") are ambiguous — any expression referencing that name silently resolves to whichever one appears first in the workflow. Rename each node exactly as shown in the section headers above before wiring expressions to it.

---

## Variations

**Single-turn (no memory).** Remove both AI Memory nodes and the append branch. The AI Prompt receives only the current message — no history. Suitable for one-shot Q&A.

**Multi-session.** If different users should have separate conversation threads, ensure each request includes a distinct `session_id`. The widget generates one automatically; if you use your own auth, override it with a user-specific identifier.

**Custom persona per session.** Add a Switch node before AI Prompt to route different session IDs to differently-configured AI Prompt nodes (different system prompts, models, or temperature settings).

**Streaming responses.** The reply is delivered as a single event once the workflow finishes, not as incremental tokens — there's no partial-response display while the model is still generating.
