# Aerini Server: REST API Reference

This is the reference for `aerini-server` in API mode. Use it to manage workflows, trigger runs, control the scheduler, and stream live events — all programmatically.

**Base URL:** `http://<host>:<port>` (default port 7700)

**Authentication:** all endpoints (except `/api/health`) require a bearer token:
```
Authorization: Bearer <your-token>
```

Set the token with `--token` or the `AERINI_TOKEN` environment variable when starting the server.

**Content type:** all request and response bodies are JSON.

---

## Workflows

### `GET /api/workflows`

List all stored workflows.

**Query parameters**

| Param | Type | Default | Description |
|---|---|---|---|
| `limit` | int | 100 | Max records returned (capped at 500) |
| `offset` | int | 0 | Records to skip (for pagination) |

**Response 200**
```json
{
  "items": [
    {
      "id": "wf_abc123",
      "name": "My Workflow",
      "schema_version": "1.0",
      "description": "",
      "nodes": [...],
      "edges": [...],
      "metadata": {}
    }
  ],
  "total": 1,
  "limit": 100,
  "offset": 0
}
```

---

### `GET /api/workflows/:id`

Fetch a single workflow by ID.

**Response 200:** the workflow object  
**Response 404:** `{"error": "Not found"}`

---

### `POST /api/workflows`

Create or replace a workflow. Upsert by ID: if a workflow with the same ID exists, it is fully replaced.

**Request body**
```json
{ "workflow_json": "<workflow JSON string>" }
```

**Response 200:** `{"ok": true}`  
**Response 400:** `{"error": "..."}`

---

### `DELETE /api/workflows/:id`

Delete a workflow and remove its scheduler entry.

**Response 200:** `{"ok": true}`  
**Response 404:** `{"error": "Not found"}`

---

## Runs

### `POST /api/workflows/:id/run`

Trigger a single run of a workflow immediately.

**Request body** (optional)
```json
{ "initial_variables": { "key": "value" } }
```

Initial variables are injected into the execution context and available in expressions via `{{$var.key}}`.

**Response 200**
```json
{
  "execution_id": "exec_xyz",
  "workflow_id": "wf_abc123",
  "success": true,
  "node_outputs": { "Node Name": { ... } },
  "logs": [
    { "timestamp": "2025-01-01T12:00:00Z", "node_id": "n1", "level": "INFO", "message": "..." }
  ],
  "error": null,
  "validation_errors": []
}
```

**Response 429:** the workflow is currently locked (a run is already in progress)  
**Response 503:** server is at capacity. Includes a `Retry-After` header (in seconds). See [Rate limits](#rate-limits) below.

---

## Scheduler

### `GET /api/scheduler`

List all scheduled jobs and their current status.

**Response 200:** `{"items": [...], "total": N}`

---

### `POST /api/scheduler/:id/start`

Start scheduling a workflow.

**Request body**
```json
{ "always_on": false }
```

Set `always_on: true` to have the scheduler automatically restart the workflow if it errors or if the server restarts.

**Response 200:** `{"status": "started"}`

---

### `POST /api/scheduler/:id/stop`

Stop a scheduled workflow.

**Response 200:** `{"status": "stopped"}`

---

## Credentials

### `GET /api/credentials`

List all stored credential IDs and names. Credential values are never returned through the API.

**Response 200:** array of `{ "id": "...", "name": "..." }` objects

---

### `POST /api/credentials`

Store a new credential.

**Request body**
```json
{ "id": "openai-prod", "name": "OpenAI API Key", "value": "sk-..." }
```

**Response 200:** `{"ok": true}`

---

### `DELETE /api/credentials/:id`

Delete a credential.

**Response 200:** `{"ok": true}`  
**Response 404:** `{"error": "Not found"}`

---

## Tokens

Token management endpoints require **admin scope**. The initial token set via `--token` / `AERINI_TOKEN` has admin scope automatically.

### `GET /api/tokens`

List all tokens. Token values are never returned.

**Requires:** admin scope

**Response 200**
```json
[
  {
    "token_id": "tok_abc123",
    "label": "CI deploy",
    "scopes": ["read", "write"],
    "created_at": "2025-01-01T12:00:00Z",
    "revoked_at": null,
    "expires_at": null
  }
]
```

---

### `POST /api/tokens`

Create a new token with specific scopes and an optional expiry.

**Requires:** admin scope

**Request body**
```json
{
  "label": "CI deploy",
  "scopes": ["read", "write"],
  "expires_in_secs": 86400
}
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `label` | string | yes | 1–256 characters |
| `scopes` | array | no | Any of `read`, `write`, `admin`. Defaults to `["read", "write"]` |
| `expires_in_secs` | int | no | Token lifetime in seconds. Omit for a non-expiring token |

**Response 201**
```json
{
  "token": "aerini_...",
  "label": "CI deploy",
  "scopes": ["read", "write"],
  "expires_in_secs": 86400,
  "note": "Save this token — it will not be shown again."
}
```

The token value is shown only once. Copy it immediately.

**Response 400:** invalid label or unrecognised scope value

---

### `DELETE /api/tokens/:id`

Revoke a token by its `token_id`. You cannot revoke the token you're currently authenticated with.

**Requires:** admin scope

**Response 200:** `{"ok": true}`  
**Response 400:** `{"error": "Cannot revoke the token you are currently using"}`

---

### `GET /api/tokens/:id/workflows`

List workflow IDs the token is restricted to for SSE event delivery. An empty list means the token can see all workflow events.

**Requires:** admin scope

**Response 200**
```json
{
  "token_id": "tok_abc123",
  "workflow_ids": ["wf_abc123"],
  "note": "Token is restricted to these workflow IDs only."
}
```

---

### `POST /api/tokens/:id/workflows/:wf_id`

Restrict a token's SSE event access to a specific workflow. Once any grant exists on a token, it can only see events for its granted workflows.

**Requires:** admin scope

**Response 201:** `{"ok": true, "token_id": "...", "workflow_id": "..."}`

---

### `DELETE /api/tokens/:id/workflows/:wf_id`

Remove a workflow grant from a token.

**Requires:** admin scope

**Response 200:** `{"ok": true}`

---

## Events (SSE)

### `GET /api/events`

Subscribe to a live stream of workflow and scheduler events using [Server-Sent Events](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events).

**Requires:** read scope

**Query parameters**

| Param | Type | Description |
|---|---|---|
| `workflow_id` | string | Optional. Filter to events from one workflow. Returns 403 if the token's ACL doesn't include that workflow. |

**Response:** `text/event-stream`. Each message's `data` field is a JSON object:

```json
{ "event": "<event-type>", "payload": { ... } }
```

**Event types**

| Event | Payload fields | When it fires |
|---|---|---|
| `node-status` | `workflow_id`, `node_id`, `status` | A node starts, completes, or fails during a run |
| `scheduler-status` | `workflow_id`, `workflow_name`, `status`, `run_count`, `last_run_at`, `next_run_at`, `last_error`, `last_result` | A scheduler job's state changes. `status` is one of `waiting`, `running`, `done`, `error`, `stopped`. `last_result` is included only on `done` or `error`. |
| `scheduler-error` | `workflow_id`, `message` | A scheduler run encountered an error |
| `scheduler-warning` | `workflow_id`, `message` | A non-fatal scheduler warning |
| `scheduler-skip` | `workflow_id`, `reason` | A scheduled run was skipped |
| `scheduler-ready` | `job_count`, `timestamp` | The scheduler finished loading and is ready |

**Connection limits:** maximum 64 concurrent SSE connections per server instance. Requests beyond that receive `429 Too Many Requests`.

**Keep-alive:** the server sends a `ping` comment every 30 seconds to prevent proxy timeout disconnections.

**Response 403:** the token's ACL doesn't allow the requested `workflow_id`  
**Response 429:** `{"error": "too many active SSE connections"}`

---

## Health

### `GET /api/health`

Server liveness check. No authentication required.

**Response 200:** `{"status": "ok", "version": "<semver>"}`

Use this in load balancers, uptime monitors, and deployment health checks.

---

## Error format

All error responses use this shape:

```json
{ "error": "human-readable message" }
```

---

## Rate limits

The server enforces a ceiling on simultaneous workflow runs via `--max-concurrent-runs` (default: 10). When all slots are occupied, incoming run requests queue — they don't fail immediately. If a slot doesn't open within `--max-queue-wait-secs` (default: 30 seconds), the server returns `503 Service Unavailable` with a `Retry-After` header indicating how many seconds to wait before retrying.

To disable queuing and get an immediate `503` when at capacity, set `--max-queue-wait-secs 0`.
