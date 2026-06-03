# Flowo Server — REST API Reference

Base URL: `http://<host>:<port>` (default port 4242)

All request/response bodies are JSON unless otherwise noted.
Authentication: Bearer token via `Authorization: Bearer <token>` header (configure with `--api-key`).

---

## Workflows

### `GET /workflows`
List all stored workflows.

**Response 200**
```json
[
  {
    "id": "wf_abc123",
    "name": "My Workflow",
    "schema_version": "1.0",
    "description": "",
    "nodes": [...],
    "edges": [...],
    "metadata": {}
  }
]
```

---

### `GET /workflows/:id`
Fetch a single workflow by ID.

**Response 200** — workflow object (same shape as above)  
**Response 404** — `{"error": "workflow not found"}`

---

### `POST /workflows`
Create or fully replace a workflow. The body must be a complete workflow JSON object.

**Request body** — full workflow object  
**Response 201** — saved workflow object

---

### `PUT /workflows/:id`
Replace a workflow by ID.

**Request body** — full workflow object  
**Response 200** — updated workflow object  
**Response 404** — not found

---

### `DELETE /workflows/:id`
Delete a workflow and its scheduler entry.

**Response 204** — no body  
**Response 404** — not found

---

## Runs

### `POST /workflows/:id/run`
Trigger a single ad-hoc run of a workflow.

**Request body** (optional)
```json
{ "variables": { "key": "value" } }
```

**Response 200**
```json
{
  "execution_id": "exec_xyz",
  "workflow_id":  "wf_abc123",
  "success":      true,
  "started_at":   "2025-01-01T12:00:00Z",
  "duration_ms":  1234,
  "node_results": { ... }
}
```

**Response 429** — server is at maximum concurrent workflow capacity  
**Response 503** — workflow currently locked (another run in progress)

---

### `GET /workflows/:id/runs`
Return the most recent run records for a workflow (default: last 50).

**Query params**
| Param  | Type | Default | Description          |
|--------|------|---------|----------------------|
| `limit`| int  | 50      | Max records returned |

**Response 200** — array of run record objects

---

## Scheduler

### `POST /workflows/:id/schedule`
Enable or update the scheduler for a workflow.

**Request body**
```json
{
  "trigger": {
    "type": "interval",
    "secs": 3600
  }
}
```
Supported trigger types: `interval` (`secs`), `cron` (`expr`), `once` (`run_at` ISO-8601), `webhook` (`port`, `path`, `method`, `secret`).

**Response 200** — `{"status": "scheduled"}`

---

### `DELETE /workflows/:id/schedule`
Disarm the scheduler for a workflow.

**Response 200** — `{"status": "disarmed"}`

---

## Health

### `GET /health`
Server liveness check. No authentication required.

**Response 200** — `{"status": "ok", "version": "0.1.0"}`

---

## Error format

All error responses share this shape:
```json
{ "error": "human-readable message" }
```

---

## Rate limits

The server enforces a global concurrent-run ceiling (`--max-concurrent-runs`, default 16). Requests beyond the ceiling receive HTTP 429. Retry with exponential back-off.
