# Server API Reference

`aerini-server api` exposes a REST API for every workflow it manages: CRUD, on-demand runs, the scheduler, live memory and performance data, credentials, plugins, and token administration, plus a Server-Sent Events stream for run status. This page is an exhaustive route reference. For how to get the server running in the first place, see [Server Deployment](server-deploy.md); for the equivalent `aerini-server` CLI flags and control commands, see [Server CLI Reference](server-cli-reference.md); for the security reasoning behind scopes, ACLs, and the SSRF warning, see [Security](../guide/security.md).

## Authentication and scopes

Every route below requires an `Authorization: Bearer <token>` header, except `GET /api/health`, `GET /aerini-widget.js`, `POST /api/widget/:workflow_id/trigger`, and `POST /api/widget/:workflow_id/mint-token` (see [Widget trigger](#widget-trigger)). A missing or invalid token returns `401 Unauthorized` with `{"error": "Invalid or missing token"}`.

Each token carries one or more scopes: `read`, `write`, `admin`. A route's required scope is checked against the token's scope list; `admin` satisfies any check, so an admin token never needs `read` or `write` listed separately. Failing a scope check returns `403 Forbidden` with `{"error": "<scope> scope required"}` (token management routes use `{"error": "admin scope required for token management"}` instead).

## Per-token workflow ACL

Beyond scope, a token can be restricted to a specific set of workflow IDs. `GET /api/tokens/:id/workflows` lists a token's current grants; `POST` and `DELETE` on `/api/tokens/:id/workflows/:workflow_id` add or remove one. A token with zero ACL grants is unrestricted, not blocked: the ACL only starts filtering once at least one workflow is explicitly granted.

The ACL is enforced by four route families: scheduler (`GET /api/scheduler`, `POST /api/scheduler/:id/start`, `POST /api/scheduler/:id/stop`), `GET /api/memory`, performance (`GET /api/performance/live`, `GET|DELETE /api/performance/reports`, `GET|DELETE /api/performance/reports/:run_id`), and `GET /api/events`. Two enforcement shapes exist: a pre-check that returns `403` with `{"error": "token ACL does not permit access to this workflow"}` before doing any work (scheduler start/stop, performance list/clear, SSE), and a fetch-then-check on the two `:run_id` performance routes that returns a plain `404` instead of `403` for a row the caller's ACL excludes, so a restricted token can't distinguish "doesn't exist" from "exists, not yours."

The ACL does **not** apply to `GET /api/workflows`, `GET|DELETE /api/workflows/:id`, or `POST /api/workflows/:id/run`: those four routes check only the base `read`/`write` scope. A write-scoped token restricted by ACL to one workflow can still list, read, delete, or run any workflow on the server through these routes. The token management endpoints' own response text and the source comments above them describe this ACL as gating "SSE events," which was true when it was introduced but has not matched its actual scope (scheduler, memory, and performance too) since the ACL was extended to those route families; the label is stale, the enforcement described above is what actually runs.

## Conventions

**Errors.** Failures return a JSON body of the form `{"error": "<message>"}` (some also add extra fields, noted per route). Success on a mutation is usually `{"ok": true}` plus any fields specific to that route.

**Pagination.** `GET /api/workflows` and `GET /api/scheduler` share one shape: `limit` (default `100`, capped at `500`) and `offset` (default `0`) as query parameters, and a response body of `{"items": [...], "total": <n>, "limit": <n>, "offset": <n>}`. `GET /api/performance/reports` uses a similar but distinct shape: `limit` (default `100`, clamped to `1..=1000` server-side) and `offset`, with a response of `{"items": [...], "limit": <n>, "offset": <n>, "has_more": <bool>}` (no `total`).

**Concurrency and limits.** Requests are capped at 300 per client IP per 60-second window; over that returns `429 Too Many Requests` with no body. The client IP is the socket's own address unless `--trusted-proxy-count` is set, in which case that many hops are trusted out of `X-Forwarded-For`. Request bodies are capped at 5 MiB. `GET /api/events` connections are capped at 64 concurrent, server-wide; a connection past that limit gets `429` with `{"error": "too many active SSE connections"}`. `POST /api/workflows/:id/run` draws from a global run semaphore sized by `--max-concurrent-runs` (default `10`); a request that can't get a slot within `--max-queue-wait-secs` (default `30`) gets `503 Service Unavailable` with a `Retry-After` header, and a request for a workflow that's already running waits up to 5 seconds for that workflow's own lock before returning `429` with `{"error": "workflow already running"}`.

**CORS.** Cross-origin requests are allowed from `localhost`/`127.0.0.1` on any port plus any origin passed via `--allow-origin`, for `GET`, `POST`, and `DELETE`, with `Authorization`, `Content-Type`, and `If-Match` as allowed request headers and `ETag` exposed to the browser.

**Response headers.** Every response carries `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: strict-origin-when-cross-origin`, `Permissions-Policy: camera=(), microphone=(), geolocation=()`, and `Strict-Transport-Security`. JSON responses also get `Content-Security-Policy: default-src 'none'` and a no-store `Cache-Control`; `GET /aerini-widget.js` gets a 5-minute `Cache-Control` instead, since it's a static asset with no content hash to bust on deploy.

## Health

| Method | Path | Scope |
|---|---|---|
| `GET` | `/api/health` | none |

Unauthenticated. Response: `{"status": "ok", "version": "<engine version>", "node_bundled": {"status": "ok", "version": "<node version>"}}`, or `node_bundled: {"status": "error", "message": "<detail>"}` if the bundled Node.js runtime failed its own check. The outer `status` is always `"ok"` if the server is up enough to answer at all.

## Widget trigger

| Method | Path | Scope |
|---|---|---|
| `GET` | `/aerini-widget.js` | none |
| `POST` | `/api/widget/:workflow_id/trigger` | none |
| `POST` | `/api/widget/:workflow_id/mint-token` | none |

None of the three routes take a Bearer token. `GET /aerini-widget.js` serves the embeddable chat widget's JS as a static asset. `POST /api/widget/:workflow_id/trigger` is a public gateway that relays a browser POST to that workflow's already-running Webhook listener; see [Widget Embedding](../guide/widget-embedding.md) for how a page embeds it, and [Per-token workflow ACL](#per-token-workflow-acl) is not relevant here since there is no token at all on this route. Body: `{"secret": "<webhook secret>", "body": <any JSON, forwarded verbatim>}`. The `secret` is forwarded as `x-webhook-secret` to the workflow's Webhook node and never compared by this route itself; the node's own constant-time check is what accepts or rejects it.

On success: `200` with `{"ok": true, "output_node_id": "<node id or null>"}`. The actual workflow output is not returned here (the Webhook listener only acks receipt); it arrives later as a `scheduler-status` event over [`GET /api/events`](#events-sse). Failure cases: `404` if the workflow isn't scheduled, `409` if it's scheduled but not currently active, `400` if its trigger isn't a Webhook trigger, `502` if the listener can't be reached, and the upstream listener's own status code (with a translated, non-revealing hint) if it rejects the request, for `401` (secret mismatch), `404`/`405` (path or method mismatch, the widget always sends `POST`), and `413` (body too large).

`POST /api/widget/:workflow_id/mint-token` exchanges the Webhook node's raw secret for a short-lived signed token, meant to be called from the embedding page's own backend at render time so the raw secret never ships to a browser; see [Widget Embedding](../guide/widget-embedding.md) for the full pattern. Body: `{"secret": "<webhook secret>", "ttl_secs": <optional, default 300>}`. `secret` must be the Webhook node's raw configured secret, not a previously-minted token, and is checked with the same constant-time comparison as `trigger`'s. `ttl_secs` is clamped server-side to `[1, 86400]` (24 hours) regardless of what's requested.

On success: `200` with `{"token": "<signed token>", "expires_at": "<RFC 3339 timestamp>", "ttl_secs": <clamped value>}`. Failure cases: the same `404`/`409`/`400` as `trigger` for an unscheduled, inactive, or non-Webhook workflow; `400` if the Webhook node has no secret configured to mint from; `401` if `secret` doesn't match. This route sits behind the same rate limiter as `trigger` (see [Security tradeoffs](../guide/widget-embedding.md#security-tradeoffs-to-know-before-you-embed-this) for the actual limits), since a wrong guess here is exactly as much of a secret-guessing oracle as `trigger` is.

## Workflows

| Method | Path | Scope |
|---|---|---|
| `GET` | `/api/workflows` | read |
| `POST` | `/api/workflows` | write |
| `GET` | `/api/workflows/:id` | read |
| `DELETE` | `/api/workflows/:id` | write |
| `POST` | `/api/workflows/:id/run` | write |

None of these five enforce the per-token workflow ACL; see [Per-token workflow ACL](#per-token-workflow-acl).

**`GET /api/workflows`** takes the shared pagination params (see [Conventions](#conventions)). Each item is `{"id", "name", "updated_at", "tags": [...], "collection_id"}` (a summary, not the full workflow).

**`POST /api/workflows`** body: `{"workflow_json": "<the workflow as a JSON string>"}`. An optional `If-Match: "<row_version>"` request header enables optimistic-concurrency protection: omit it for an unconditional last-write-wins save; send back the `row_version` from a prior `ETag` to get `409 Conflict` (`{"error": "workflow was modified since you last loaded it", "current_row_version": <n>}`) on a stale write, or `412 Precondition Failed` if the workflow no longer exists at all. A malformed `If-Match` value (not a bare or quoted integer) is `400`. On success, `200 OK` either way, whether the workflow was newly created or updated. Response and `ETag` both carry the new `row_version`: `{"ok": true, "row_version": <n>}`.

**`GET /api/workflows/:id`** returns `200` with an `ETag: "<row_version>"` header and a body that is the workflow's pretty-printed JSON encoded as a JSON string, not a JSON object: the body is `"{\n  \"id\": ...\n}"`, so a client needs to parse it twice (once for the HTTP body, once for the string it contains) to get a usable object. `404` if the ID doesn't exist.

**`DELETE /api/workflows/:id`** stops any active scheduled job for the workflow first (best effort), then deletes its run history, scheduled-job row, and the workflow itself. `200` with `{"ok": true}`.

**`POST /api/workflows/:id/run`** body: `{"initial_variables": {...}}` (optional, defaults to empty). Runs synchronously and returns the full result once execution finishes: `200` with `{"execution_id", "workflow_id", "success", "node_outputs": {...}, "logs": [...], "error": "<string or null>", "validation_errors": [...]}`. A workflow that fails at runtime still returns `200` with `success: false` and `error` set; `500` is reserved for the request itself failing (a DB error, an executor panic), not for the workflow's own logic failing. `404` if the workflow doesn't exist. Note that a caller with the `admin` scope who runs a workflow this way passes that admin privilege into the run, so any Database node in that workflow gets `allow_raw_sql` for that execution.

## Scheduler

| Method | Path | Scope |
|---|---|---|
| `GET` | `/api/scheduler` | read |
| `POST` | `/api/scheduler/:id/start` | write |
| `POST` | `/api/scheduler/:id/stop` | write |

`start` and `stop` also enforce the [per-token workflow ACL](#per-token-workflow-acl); a write-scoped token restricted to other workflows gets `403` on a workflow outside its grants. This is the REST-level equivalent of the CLI's `start`/`stop`/`restart` commands (see [Server CLI Reference](server-cli-reference.md)); a CLI call needing both `read` (to resolve a workflow name via `list`) and `write` (for the actual start/stop call) is two REST calls, each individually scoped as shown here, not one route requiring both scopes at once.

**`GET /api/scheduler`** takes the shared pagination params. For an ACL-restricted token, filtering happens before pagination, so `total` and the returned page only ever reflect workflows that token can see. Each item is a `ScheduledJobRow`: `{"workflow_id", "workflow_name", "trigger_kind": "<JSON-encoded trigger, itself a string>", "status": "active"|"paused"|"done"|"error"|"stopped", "always_on", "run_count", "last_run_at", "next_run_at", "last_error", "created_at"}`. `trigger_kind` is JSON-encoded twice the same way a workflow body is on `GET /api/workflows/:id`; a Webhook trigger's `secret` field inside it is always replaced with the literal string `<redacted>`, never the real value.

**`POST /api/scheduler/:id/start`** body: `{"always_on": false, "port_override": <u16 or omitted>}` (`always_on` defaults to `false`). `200` with `{"ok": true}` on success, `400` with a debug-formatted error (e.g. a port conflict, or a workflow with no schedulable trigger) if the scheduler rejects the start.

**`POST /api/scheduler/:id/stop`** no body. `200` with `{"ok": true}`.

## Memory

| Method | Path | Scope |
|---|---|---|
| `GET` | `/api/memory` | read |

Enforces the [per-token workflow ACL](#per-token-workflow-acl). No pagination; returns every currently-live run as a flat JSON array (not wrapped in `items`), each entry a `RunBreakdown`: `{"workflow_id", "started_at_ms", "live_bytes": <i64, executor overhead only, excludes node bytes>, "nodes": [{"node_id", "node_type_id", "started_at_ms", "live_bytes"}, ...]}`. Empty array if nothing is currently running (or nothing the caller's ACL permits).

## Performance

| Method | Path | Scope |
|---|---|---|
| `GET` | `/api/performance/live` | read |
| `GET` | `/api/performance/reports` | read |
| `DELETE` | `/api/performance/reports` | write |
| `GET` | `/api/performance/reports/:run_id` | read |
| `DELETE` | `/api/performance/reports/:run_id` | write |

`GET /api/performance/live` enforces the [ACL](#per-token-workflow-acl) the same way as `/api/memory`: a flat array, no pagination, filtered before returning. Each entry is a `PerformanceReport`: `{"workflow_id", "status": "running"|"success"|"failed", "started_at_ms", "finished_at_ms", "duration_ms", "sampling_interval_ms", "baseline_bytes", "final_bytes", "peak_bytes", "peak_at_ms", "minimum_bytes", "average_bytes", "delta_bytes": <i64, can be negative>, "sample_count", "history": [{"at_ms", "bytes"}, ...]}`. For a still-running workflow, `finished_at_ms` is the timestamp this particular reading was taken, not a real completion time.

`GET|DELETE /api/performance/reports` (list and clear all reports for one workflow) both require a `?workflow_id=` query parameter; a request missing it is rejected with `400` before the handler runs. Both enforce the ACL pre-check (`403` on a workflow outside the caller's grants). `GET` also takes `limit` (default `100`, clamped `1..=1000`) and `offset`; response is `{"items": [<PerformanceReportRecord>, ...], "limit", "offset", "has_more"}`. Each item is a `PerformanceReport` (fields as above) flattened together with an `"id"` field (the run ID), not nested under a `report` key despite the underlying Rust type's name. `DELETE` clears every stored report for that workflow: `200` with `{"ok": true}`.

`GET|DELETE /api/performance/reports/:run_id` (a single report by run ID) check scope first, then fetch the row and apply the ACL as a fetch-then-check: a row that exists but belongs to a workflow outside the caller's grants returns a plain `404`, identical to a row that doesn't exist at all, never `403`. `GET` returns the same flattened `PerformanceReportRecord` shape as the list route. `DELETE` returns `200` with `{"ok": true}`; deleting an already-deleted `run_id` (a race with another caller) is treated as success, not an error.

## Credentials

| Method | Path | Scope |
|---|---|---|
| `GET` | `/api/credentials` | read |
| `POST` | `/api/credentials` | write |
| `DELETE` | `/api/credentials/:id` | write |

This is the `api`-mode credential store described in [Credentials](../guide/credentials.md), separate from the desktop app's own store.

**`GET /api/credentials`** returns a flat array of `{"id", "name", "cred_type"}`. Never includes the secret value, and never includes the optional `provider`/`model`/`base_url` metadata a credential can carry, since the desktop-side `list` response type this route reuses doesn't have those fields.

**`POST /api/credentials`** body: `{"id", "name", "value", "cred_type"}` (`cred_type` defaults to `"api_key"` if omitted). `provider`, `model`, and `base_url` are not accepted by this route: it always stores a credential with those three set to none, even though the underlying store supports them (the desktop app's own credential UI can set them; this REST route currently can't). `200` with `{"ok": true}` on success.

**`DELETE /api/credentials/:id`** `200` with `{"ok": true}`.

## Plugins

| Method | Path | Scope |
|---|---|---|
| `POST` | `/api/plugins/reload` | admin |
| `GET` | `/api/plugins/load-report` | admin |

Both admin-scoped: a plugin is code that runs for every caller on the server, not just the caller's own resources, so this is a higher trust bar than `write`.

**`POST /api/plugins/reload`** rebuilds the node registry from scratch (built-ins plus every `.wasm` in `--plugin-dir`) and swaps it in atomically; a run already in progress keeps executing against the registry snapshot it started with. `400` with `{"error": "no --plugin-dir configured for this server: nothing to reload"}` if the server wasn't started with one. On success, `200` with the resulting `PluginLoadReport`: `{"loaded": [...], "builtin_rejected": [...], "plugin_collisions": [...]}`, each a flat array of node `type_id` strings (loaded includes the winner of any collision; builtin_rejected lists ones that collided with a built-in node and lost outright; plugin_collisions lists ones two or more plugin files both claimed).

**`GET /api/plugins/load-report`** returns the same `PluginLoadReport` shape, from the most recent load (server startup, or the last `/api/plugins/reload` call), without triggering a new scan.

## Tokens

| Method | Path | Scope |
|---|---|---|
| `GET` | `/api/tokens` | admin |
| `POST` | `/api/tokens` | admin |
| `DELETE` | `/api/tokens/:id` | admin |
| `GET` | `/api/tokens/:id/workflows` | admin |
| `POST` | `/api/tokens/:id/workflows/:workflow_id` | admin |
| `DELETE` | `/api/tokens/:id/workflows/:workflow_id` | admin |

All six require `admin`; this is the REST equivalent of the CLI's `tokens` subcommands (see [Server CLI Reference](server-cli-reference.md)).

**`GET /api/tokens`** returns a flat array of `{"token_id", "label", "scopes": [...], "created_at", "revoked_at", "expires_at"}`. Never includes the token's own secret value; that only exists at creation time.

**`POST /api/tokens`** body: `{"label": "<1-256 chars>", "scopes": [...], "expires_in_secs": <u64 or omitted>}` (`scopes` defaults to `["read", "write"]` if omitted; each entry must be `read`, `write`, or `admin`, anything else is `400`). On success, `201 Created` with `{"token": "<raw token, shown once>", "label", "scopes", "expires_in_secs", "note": "Save this token, it will not be shown again."}`.

**`DELETE /api/tokens/:id`** `200` with `{"ok": true}`, except a token cannot revoke itself: revoking the same token ID present in the caller's own `Authorization` header returns `400` with `{"error": "Cannot revoke the token you are currently using"}`.

**`GET /api/tokens/:id/workflows`** lists the target token's ACL grants: `{"token_id", "workflow_ids": [...], "note": "<one of two fixed strings depending on whether the list is empty>"}`. An empty list means that token is unrestricted everywhere the ACL applies (see [Per-token workflow ACL](#per-token-workflow-acl)), not that it has no access.

**`POST /api/tokens/:id/workflows/:workflow_id`** grants the target token access to one workflow. `201` with `{"ok": true, "token_id", "workflow_id", "note": "..."}`.

**`DELETE /api/tokens/:id/workflows/:workflow_id`** revokes that grant. `200` with `{"ok": true}`.

## Events (SSE)

| Method | Path | Scope |
|---|---|---|
| `GET` | `/api/events` | read |

A long-lived `text/event-stream` connection, capped at 64 concurrent connections server-wide (see [Conventions](#conventions)). Enforces the [per-token workflow ACL](#per-token-workflow-acl); an optional `?workflow_id=` query parameter narrows the connection to one workflow's events, and is rejected with `403` if the caller's ACL doesn't include that workflow. Each message's `data` field is a JSON object of the form `{"event": "<type>", "payload": {...}}`. The server also sends a keep-alive `ping` every 30 seconds on an otherwise-idle connection.

Six event types are emitted:

| Event | Payload fields |
|---|---|
| `node-status` | `workflow_id`, `node_id`, `status` (`running`, `success`, `error`, or `skipped`) |
| `scheduler-status` | `workflow_id`, `workflow_name`, `status` (`running`, `waiting`, `done`, `error`, or `stopped`), `run_count`, `last_run_at`, `next_run_at`, `last_error`, `last_result` |
| `scheduler-error` | `workflow_id`, `message` |
| `scheduler-warning` | `workflow_id`, `message` |
| `scheduler-skip` | `workflow_id`, `reason` |
| `scheduler-ready` | `job_count`, `timestamp` (no `workflow_id`; sent once at scheduler startup and forwarded to every connection regardless of ACL, the one event type an ACL filter can't narrow) |

`scheduler-status` is what a widget or dashboard should watch for a workflow triggered through [`POST /api/widget/:workflow_id/trigger`](#widget-trigger), since that route's own response never carries the run's actual output.

## See also

- [Server Deployment](server-deploy.md), for getting `--api` mode running in the first place
- [Server CLI Reference](server-cli-reference.md), for the `aerini-server` flags and control commands that sit on top of these same routes
- [Widget Embedding](../guide/widget-embedding.md), for the browser-side half of the widget trigger route
- [Credentials](../guide/credentials.md), for how this store differs from the desktop app's own
- [Security](../guide/security.md), for the reasoning behind scopes, the ACL model, and the SSRF warning this server logs on startup
