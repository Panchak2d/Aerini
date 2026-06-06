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
