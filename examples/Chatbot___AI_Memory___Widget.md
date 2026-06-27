# Chatbot with AI Memory and Widget

A four-node workflow that runs a persistent chatbot: receives a message from the embedded chat widget, retrieves the conversation history from AI Memory, sends history + message to an AI model, stores the updated history, and returns the response to the widget.

---

## Workflow structure

```
Manual Trigger
     │
     ▼
AI Memory (read)
     │
     ▼
AI Prompt
     │
     ▼
AI Memory (write)
     │
     ▼
Output
```

Use two **AI Memory** nodes: the first reads the stored history before the AI Prompt runs; the second writes the updated history after the AI Prompt responds. Connect them in sequence — Trigger → Memory (read) → AI Prompt → Memory (write) → Output.

---

## Node configuration

### 1 · Manual Trigger

| Setting | Value |
|---|---|
| **Trigger name** | `chat` (or any name — this becomes the webhook path) |
| **Method** | `POST` |

The widget posts JSON `{ "message": "user text" }` to this endpoint. Reference the incoming message in downstream nodes as `{{ManualTrigger.output.body.message}}`.

### 2 · AI Memory (read)

| Setting | Value |
|---|---|
| **Mode** | `read` |
| **Session ID** | `{{ManualTrigger.output.body.session_id}}` — or a fixed string (e.g. `default`) for a single shared conversation |
| **Max messages** | `20` (keeps context from growing unboundedly) |

Output: `messages` array, ordered oldest-first, ready to pass directly to AI Prompt's `context` field.

### 3 · AI Prompt

| Setting | Value |
|---|---|
| **Provider** | Your chosen provider (OpenAI, Anthropic, Gemini, Ollama, …) |
| **Model** | e.g. `gpt-4o`, `claude-sonnet-4-5`, `gemini-2.0-flash` |
| **System** | Your persona, e.g. `You are a helpful assistant for Acme Corp. Be concise and friendly.` |
| **Prompt** | `{{ManualTrigger.output.body.message}}` |
| **Context** | `{{AIMemory.output.messages}}` — passes the retrieved history so the model has conversation continuity |

Output: `response` (the model's reply text).

### 4 · AI Memory (write)

| Setting | Value |
|---|---|
| **Mode** | `write` |
| **Session ID** | Same value as the read node — `{{ManualTrigger.output.body.session_id}}` or your fixed string |
| **User message** | `{{ManualTrigger.output.body.message}}` |
| **Assistant message** | `{{AIPrompt.output.response}}` |

This appends the new exchange (user turn + assistant turn) to the session's stored history. The next invocation will include this exchange when reading.

### 5 · Output

| Setting | Value |
|---|---|
| **Response** | `{{AIPrompt.output.response}}` |

Returns the AI's reply as the HTTP response body. The widget reads this and renders it in the chat thread.

---

## Embedding the chat widget

Add the widget script to any HTML page:

```html
<script
  src="https://your-aerini-host/aerini-widget.js"
  data-endpoint="https://your-aerini-host/webhook/chat"
  data-token="your-run-secret"
  data-title="Chat with us"
  data-placeholder="Type a message…"
></script>
```

| Attribute | Required | Description |
|---|---|---|
| `src` | Yes | Widget script served by your Aerini instance |
| `data-endpoint` | Yes | Webhook URL of the Manual Trigger — replace `chat` with your trigger name |
| `data-token` | Yes | The `run_secret` for your workflow (see security note below) |
| `data-title` | No | Text shown in the widget header |
| `data-placeholder` | No | Input placeholder text |

The widget posts `{ "message": "<user text>", "session_id": "<browser-generated UUID>" }` and renders the response. Session IDs are generated in the browser and persisted in `localStorage` — each browser tab maintains its own conversation thread.

For full widget configuration and attribute reference, see [docs/widget-embedding.md](../docs/widget-embedding.md).

---

## Security notes

**Token visibility.** The `data-token` attribute is visible in page source and browser dev tools. Treat it as a rate-limiting key, not a secret. Anyone who can view the page source can send requests to your webhook.

**Recommended mitigations:**
- Rate-limit the webhook endpoint at the reverse proxy (e.g. nginx `limit_req`).
- Set `timeout_secs` on the Manual Trigger node — an open connection that never sends a body will hold the executor.
- Use session IDs from your own auth system instead of browser-generated UUIDs if you need per-user isolation. Pass the authenticated session ID from your backend rather than letting the widget generate one.

**Prompt injection.** User messages flow directly into the AI Prompt's `prompt` field. A user who can send messages can attempt to redirect the model's behavior via prompt injection. Add a Code node before AI Prompt to strip or sanitize known injection patterns if you're operating in an adversarial environment.

---

## How to import

Workflow templates in Markdown format document the intended structure — use them as a reference while building the workflow by hand in the canvas.

1. Open Aerini and create a new workflow.
2. Add nodes in the order listed above.
3. Configure each node using the settings tables.
4. Wire them left to right: Trigger → Memory (read) → AI Prompt → Memory (write) → Output.
5. Set a `run_secret` on the workflow before exposing the endpoint publicly.
6. Copy the embed snippet from the Output node's panel and paste it into your HTML page.

---

## Variations

**Single-turn (no memory).** Remove both AI Memory nodes. The AI Prompt receives only the current message — no history. Suitable for one-shot Q&A.

**Multi-session.** If different users should have separate conversation threads, ensure each request includes a distinct `session_id`. The widget generates one automatically; if you use your own auth, override it with a user-specific identifier.

**Custom persona per session.** Add a Switch node before AI Prompt to route different session IDs to differently-configured AI Prompt nodes (different system prompts, models, or temperature settings).

**Streaming responses.** The widget does not stream — it displays the full response when the workflow completes. For a streaming experience, use Server-Sent Events via a custom relay in front of the webhook endpoint.
