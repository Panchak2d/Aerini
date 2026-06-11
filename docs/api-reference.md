# Flowo Server: REST API Reference

Base URL: `http://<host>:<port>` (default port 7700)

All request and response bodies are JSON unless otherwise noted.
Authentication: `Authorization: Bearer <token>` header. Configure with `--token` or `FLOWO_TOKEN` env var.

---

## Workflows

### `GET /api/workflows`

List all stored workflows. Paginated.

**Query params**

| Param    | Type | Default | Description          |
|----------|------|---------|----------------------|
| `limit`  | int  | 100     | Max records returned (cap 500) |
| `offset` | int  | 0       | Records to skip      |

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

**Response 200:** workflow object  
**Response 404:** `{"error": "Not found"}`

---

### `POST /api/workflows`

Create or replace a workflow. Upsert by ID: if a workflow with the same ID already exists, it is fully replaced.

**Request body**
```json
{ "workflow_json": "<workflow JSON string>" }
```

**Response 200:** `{"ok": true}`  
**Response 400:** `{"error": "..."}`

---

### `DELETE /api/workflows/:id`

Delete a workflow and its scheduler entry.

**Response 200:** `{"ok": true}`  
**Response 404:** `{"error": "Not found"}`

---

## Runs

### `POST /api/workflows/:id/run`

Trigger a single ad-hoc run of a workflow.

**Request body** (optional)
```json
{ "initial_variables": { "key": "value" } }
```

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

**Response 429:** workflow currently locked (another run in progress)  
**Response 503:** server is at capacity and no slot became available within the queue timeout. Includes a `Retry-After` header (seconds).

---

## Scheduler

### `GET /api/scheduler`

List all scheduled jobs.

**Response 200:** `{"items": [...], "total": N}`

---

### `POST /api/scheduler/:id/start`

Start scheduling a workflow.

**Request body**
```json
{ "always_on": false }
```

**Response 200:** `{"status": "started"}`

---

### `POST /api/scheduler/:id/stop`

Stop a scheduled workflow.

**Response 200:** `{"status": "stopped"}`

---

## Credentials

### `GET /api/credentials`

List all stored credential IDs and names (values are never returned).

**Response 200:** array of credential objects

---

### `POST /api/credentials`

Store a credential.

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

All token routes require **admin scope**. The initial token (set via `--token` / `FLOWO_TOKEN`) has admin scope by default.

### `GET /api/tokens`

List all tokens (values are never returned).

**Auth:** admin scope required

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

Create a new token.

**Auth:** admin scope required

**Request body**
```json
{
  "label": "CI deploy",
  "scopes": ["read", "write"],
  "expires_in_secs": 86400
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `label` | string | yes | 1–256 characters |
| `scopes` | array | no | Any of `read`, `write`, `admin`. Defaults to `["read", "write"]` |
| `expires_in_secs` | int | no | Token lifetime in seconds. Omit for non-expiring |

**Response 201**
```json
{
  "token": "flowo_...",
  "label": "CI deploy",
  "scopes": ["read", "write"],
  "expires_in_secs": 86400,
  "note": "Save this token — it will not be shown again."
}
```

**Response 400:** invalid label or unrecognised scope value

---

### `DELETE /api/tokens/:id`

Revoke a token by its `token_id`. You cannot revoke the token you are currently using.

**Auth:** admin scope required

**Response 200:** `{"ok": true}`  
**Response 400:** `{"error": "Cannot revoke the token you are currently using"}`  
**Response 500:** internal error

---

### `GET /api/tokens/:id/workflows`

List workflow IDs the token is restricted to for SSE event delivery. An empty list means the token sees all workflow events.

**Auth:** admin scope required

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

Grant a token access to a specific workflow's SSE events. Once any grant exists, the token is restricted to only those workflows.

**Auth:** admin scope required

**Response 201**
```json
{
  "ok": true,
  "token_id": "tok_abc123",
  "workflow_id": "wf_abc123",
  "note": "Token is now restricted to SSE events for its granted workflow(s)."
}
```

---

### `DELETE /api/tokens/:id/workflows/:wf_id`

Revoke a token's access to a specific workflow's SSE events.

**Auth:** admin scope required

**Response 200:** `{"ok": true}`

---

## Events

### `GET /api/events`

Subscribe to a live Server-Sent Events (SSE) stream of workflow and scheduler events.

**Auth:** read scope required

**Query params**

| Param | Type | Description |
|-------|------|-------------|
| `workflow_id` | string | Optional. Filter events to a single workflow. Subject to token ACL — returns 403 if the token's ACL does not include that workflow. |

**Response:** `text/event-stream`. Each SSE `data` field is a JSON object:

```json
{ "event": "<event-type>", "payload": { ... } }
```

**Event types**

| Event | Payload fields | Description |
|-------|---------------|-------------|
| `node-status` | `workflow_id`, `node_id`, `status` | Node started, completed, or failed during a run |
| `scheduler-status` | `workflow_id`, `workflow_name`, `status`, `run_count`, `last_run_at`, `next_run_at`, `last_error`, `last_result` | Scheduler job state changed. `status` is one of `waiting`, `running`, `done`, `error`, `stopped`. `last_result` (full `WorkflowResult`) is only present on `done` or `error`. |
| `scheduler-error` | `workflow_id`, `message` | Scheduler encountered a run error |
| `scheduler-warning` | `workflow_id`, `message` | Non-fatal scheduler warning |
| `scheduler-skip` | `workflow_id`, `reason` | Scheduled run was skipped |
| `scheduler-ready` | `job_count`, `timestamp` | Scheduler finished loading and is ready |

**Connection limits:** at most 64 concurrent SSE connections per server instance. Excess connections receive `429 Too Many Requests`.

**Keep-alive:** the server sends a `ping` comment every 30 seconds to prevent proxy timeouts.

**Response 403:** token ACL does not permit the requested `workflow_id`  
**Response 429:** `{"error": "too many active SSE connections"}`

---

## Health

### `GET /api/health`

Server liveness check. No authentication required.

**Response 200:** `{"status": "ok", "version": "0.2.0"}`

---

## Error format

All error responses use this shape:
```json
{ "error": "human-readable message" }
```

---

## Rate limits

The server enforces a global concurrent-run ceiling via `--max-concurrent-runs` (default 10). When all slots are occupied, incoming run requests queue rather than being rejected immediately. If a slot does not open within `--max-queue-wait-secs` (default 30), the server returns 503 with a `Retry-After` header. Pass `--max-queue-wait-secs 0` to get immediate 503 on full capacity instead.
