# Nodes Reference

All 34 built-in nodes, organized by category. Every node's parameters, outputs, and any constraints worth knowing upfront.

The `type_id` shown under each node name is the internal identifier used in workflow JSON and the server API. If you're using Flowo's visual interface, you don't need it — just search for the node by its display name. If you're building workflows programmatically or importing via the API, use the `type_id`.

Parameter types: `string`, `number`, `boolean`, `object`, `array`. Required parameters are marked **required**.

---

## Per-node retry and error routing

Every node has two optional settings that control what happens when it fails:

### Retry config

| Field | Type | Default | Notes |
|---|---|---|---|
| `max_attempts` | number | `1` | Total attempts including the first. `1` = no retry. Max `10`. |
| `backoff_ms` | number | `500` | Milliseconds to wait between retry attempts. |

`max_attempts: 1` is the default — nodes do **not** retry automatically unless you change this. Set retries only on idempotent operations (HTTP GET, database reads, AI prompts). Nodes with side effects (Send Email, Stripe, Slack) should stay at `1` — a retry fires the side effect again.

You configure these in the node's config panel under **Retry**.

### on_error port

Every node has a hidden `on_error` output port. If the node fails on its final attempt, execution routes through `on_error` instead of stopping the workflow.

To wire it: hover the node on the canvas. A secondary output port labelled `on_error` appears on the right edge. Drag from it to a downstream node (typically a Stop node, a Send Email, or a Slack notification to alert you of the failure).

If `on_error` is not wired and the node fails, the workflow stops and the failure is recorded in the run result. If `on_error` is wired, the workflow continues through that branch — the failed node's output is not available to downstream nodes on that path.

---

## Triggers

Trigger nodes have no input port. They're always the first node in a workflow.

### Manual Trigger

`type_id: manual_trigger`

Runs when you press the Run button or call `/api/workflows/:id/run`. No parameters. Output: `{ triggered_at: string }`.

Use this during development. It can't be scheduled — exporting a workflow with only a Manual Trigger for server deployment is blocked.

### Schedule

`type_id: schedule`

| Parameter | Type | Notes |
|---|---|---|
| `mode` | string | **required.** `interval`, `cron`, or `once` |
| `interval_secs` | number | Seconds between runs. Used when `mode` is `interval`. |
| `cron_expr` | string | Cron expression e.g. `0 9 * * 1-5`. Used when `mode` is `cron`. |
| `run_at` | string | ISO 8601 timestamp. Used when `mode` is `once`. |

Output: `{ triggered_at: string, mode: string }`

The Schedule node doesn't sleep inside its `execute()` — the scheduler daemon owns the timing. Don't put a Schedule node in the middle of a workflow; it only makes sense as the first node.

### Webhook

`type_id: webhook`

| Parameter | Type | Notes |
|---|---|---|
| `port` | number | Port to listen on. Default `3456`. Must be ≥ 1024. |
| `path` | string | URL path. Default `/webhook`. |
| `method` | string | `GET`, `POST`, `PUT`, or `ANY`. |
| `secret` | string | Optional. If set, the incoming request must include an `X-Flowo-Secret` header matching this value. Comparison is timing-safe. |
| `timeout_secs` | number | How long to wait for an incoming request. Default `60`. |

Output: `{ body: any, headers: object, method: string, path: string }`

The Webhook node binds to `127.0.0.1` only. In the desktop app this means localhost access only; in server mode, use a reverse proxy to expose it externally. A sender that connects but never completes its request is automatically disconnected after at most 30 seconds. `timeout_secs` controls how long the node waits for a valid request to arrive before giving up entirely.

---

## Logic

### If / Condition

`type_id: if_condition`

Evaluates a condition and routes to one of two output ports.

| Parameter | Type | Notes |
|---|---|---|
| `condition` | string | **required.** Expression that evaluates to truthy/falsy. Supports `==`, `!=`, `>`, `<`, `>=`, `<=`, and `contains`. |

Ports: `true` output and `false` output. Connect downstream nodes to whichever branch applies.

### Switch

`type_id: switch`

Routes to one of N output ports based on a value.

| Parameter | Type | Notes |
|---|---|---|
| `value` | string | **required.** The value to test (usually an expression). |
| `cases` | array | Array of `{ match: string, port: string }` objects. |
| `default_port` | string | Port to use if no case matches. |

### Loop (For Each)

`type_id: loop_node`

Iterates over an array, running downstream nodes once per item.

| Parameter | Type | Notes |
|---|---|---|
| `items` | any | **required.** The array to iterate over. |
| `item_key` | string | Key name injected into each iteration's context. Default `item`. |

Each iteration runs the downstream subgraph with the current item available as `{{loop.output.item}}` (or whatever `item_key` is set to).

### Merge

`type_id: merge`

Waits for multiple upstream branches to complete, then passes all their outputs downstream as a merged object.

No parameters. Output: combined object of all upstream node outputs.

### Stop

`type_id: stop`

Immediately terminates the workflow execution at this point. Any downstream nodes are not run. Useful for early exits in conditional branches.

No parameters.

---

## Flow Control

### Delay

`type_id: delay`

| Parameter | Type | Notes |
|---|---|---|
| `delay_ms` | number | **required.** Milliseconds to wait before continuing. |

Output: `{ delayed_ms: number }`

### Wait

`type_id: wait_node`

Pauses execution until an external signal. Used in workflows that need to wait for a human or external process before continuing.

| Parameter | Type | Notes |
|---|---|---|
| `timeout_secs` | number | Maximum wait time. Default `300`. |

---

## AI

### AI Prompt

`type_id: ai_prompt`

Single-turn LLM call. Sends a prompt and returns the response.

| Parameter | Type | Notes |
|---|---|---|
| `prompt` | string | **required.** The user message. |
| `system` | string | System/persona instructions. |
| `model` | string | Model name, e.g. `gpt-4o`, `claude-sonnet-4-6`, `gemini-2.0-flash`, `llama3`. |
| `provider` | string | `auto`, `openai`, `anthropic`, or `gemini`. `auto` detects from `base_url`. |
| `base_url` | string | API base URL. Leave blank for OpenAI. Ollama: `http://localhost:11434/v1` |
| `api_key` | string | API key — use a Credential. |
| `temperature` | number | 0.0–2.0. Default `0.7`. |
| `max_tokens` | number | Maximum response tokens. Default `2048`. |
| `rate_limit_rpm` | number | Max requests per minute. `0` = unlimited. |

Output: `{ content: string, model: string, provider: string, input_tokens: number, output_tokens: number }`

The AI client uses a 120-second timeout per request. Long-running models (large `max_tokens`, slow providers) may hit this — increase with a custom `base_url` pointing to a proxy if needed.

### AI Agent

`type_id: ai_agent`

Autonomous ReAct loop (Reason → Act → Observe). The agent reasons about a goal, calls tools, observes results, and repeats until done or until `max_iterations` is reached.

| Parameter | Type | Notes |
|---|---|---|
| `goal` | string | **required.** What the agent should accomplish. Be specific. |
| `system` | string | Persona and behavioral instructions. |
| `provider` | string | **required.** `openai`, `anthropic`, or `gemini`. |
| `model` | string | Model name. |
| `api_key` | string | API key — use a Credential. |
| `tools` | string | JSON array of tool definitions the agent can call. |
| `max_iterations` | number | Safety limit on the ReAct loop. Default `10`. |

Output: `{ result: string, iterations: number, tool_calls: array }`

**Provider behavior differs:** OpenAI and OpenAI-compatible providers run the full ReAct loop with tool calls across multiple iterations. Anthropic runs a single reasoning pass — no tool loop. Connect the Anthropic agent's output to downstream action nodes to take action on its result.

### AI Memory

`type_id: ai_memory`

Stores and retrieves conversation history in a local SQLite database. Enables multi-turn conversations where context is preserved across workflow runs.

| Parameter | Type | Notes |
|---|---|---|
| `action` | string | **required.** `read`, `write`, or `clear`. |
| `session_id` | string | **required.** Identifies the conversation thread. |
| `role` | string | `user` or `assistant`. Required for `write`. |
| `content` | string | Message content. Required for `write`. |
| `max_messages` | number | How many past messages to return on `read`. Default `20`. |

Output: `{ messages: array, session_id: string }`

### Text Splitter

`type_id: text_splitter`

Splits a long text into chunks for feeding into AI nodes that have context limits.

| Parameter | Type | Notes |
|---|---|---|
| `text` | string | **required.** The text to split. |
| `chunk_size` | number | Target chunk size in characters. Default `1000`. |
| `overlap` | number | Characters of overlap between consecutive chunks. Default `100`. |

Output: `{ chunks: string[], count: number }`

---

## Actions

### HTTP Request

`type_id: http_request`

| Parameter | Type | Notes |
|---|---|---|
| `method` | string | **required.** `GET`, `POST`, `PUT`, `PATCH`, `DELETE`. |
| `url` | string | **required.** Must be `http` or `https`. |
| `headers` | object | Key-value pairs added to the request. |
| `body` | any | Request body. Serialized as JSON if an object. |
| `timeout_secs` | number | Default `30`. |

Output: `{ status: number, headers: object, body: any }`

The HTTP node validates URLs against a blocklist of private/internal IP ranges to prevent SSRF attacks. Redirects are disabled — if a URL redirects, you'll get a 3xx response, not the redirect target. The response body is capped at 10 MB.

The blocklist checks the URL at request time but doesn't pre-resolve DNS. A domain that resolves to a private address at connection time (DNS rebinding) would bypass the check. Don't use the HTTP node with attacker-controlled URLs in server mode.

### Shell Command

`type_id: shell_exec`

| Parameter | Type | Notes |
|---|---|---|
| `command` | string | **required.** Shell command to run. |
| `cwd` | string | Working directory. |
| `env` | object | Additional environment variables for the subprocess. |
| `timeout_secs` | number | Default `30`. |

Output: `{ stdout: string, stderr: string, exit_code: number, success: boolean }`

On Windows, commands run via `cmd /C`. On Unix, via `sh -c`. In the desktop app, you're prompted to confirm before running any workflow containing this node. In server mode, no prompt — treat workflows with shell nodes as fully trusted code.

### Code (JS)

`type_id: code`

Runs a JavaScript snippet using the system `node` binary. The snippet has access to `input` (the incoming data) and `context` (all node outputs in the run). Return a value by calling `output()`.

| Parameter | Type | Notes |
|---|---|---|
| `code` | string | **required.** JavaScript to execute. |
| `timeout_secs` | number | Max execution time. Default `10`, max `60`. |

Output: `{ result: any, stdout: string, duration_ms: number }`

Node.js must be installed and on PATH. The code runs as a subprocess — it has no access to Flowo internals beyond what's passed via `input` and `context`.

```javascript
// Example: convert temperature units
const temp = input.current?.temperature_2m ?? 0;
output({ celsius: temp, fahrenheit: temp * 9/5 + 32 });
```

### Send Email

`type_id: email_send`

Sends via SMTP. Works with any SMTP provider: Gmail, Outlook, Fastmail, Mailgun SMTP, etc.

| Parameter | Type | Notes |
|---|---|---|
| `smtp_host` | string | **required.** e.g. `smtp.gmail.com` |
| `smtp_port` | number | `587` (STARTTLS, default) or `465` (SSL). |
| `from` | string | Sender address. |
| `to` | string | **required.** Recipient(s), comma-separated. |
| `subject` | string | **required.** |
| `body` | string | **required.** |
| `html` | boolean | Send as HTML. Default `false`. |
| `username` | string | SMTP username (usually your email address). |
| `password` | string | SMTP password — use a Credential. |

Output: `{ sent: boolean, to: string, subject: string }`

For Gmail: use an App Password, not your account password. Two-factor accounts require it.

### SendGrid

`type_id: sendgrid`

| Parameter | Type | Notes |
|---|---|---|
| `to_email` | string | **required.** |
| `from_email` | string | **required.** Must be a verified sender in SendGrid. |
| `subject` | string | **required.** |
| `body` | string | **required.** Plain text only. |
| `api_key` | string | SendGrid API key (`SG....`) — use a Credential. |

Output: `{ sent: boolean, message_id: string }`

### File

`type_id: file`

Read or write files on the local filesystem.

| Parameter | Type | Notes |
|---|---|---|
| `action` | string | **required.** `read` or `write`. |
| `path` | string | **required.** Absolute path to the file. |
| `content` | string | Content to write. Required for `write`. |
| `encoding` | string | `utf8` (default) or `base64`. |

Output: `{ content: string, path: string, size: number }`

The File node can read and write any file the current OS user can access. In the desktop app you're prompted before any workflow with a File node runs. In server/API mode there is no prompt. Use `--file-sandbox-dir` to restrict File nodes to a specific directory tree — without it, File nodes can reach any path the server process can, including the data directory.

### Desktop Notification

`type_id: notification`

Shows an OS desktop notification.

| Parameter | Type | Notes |
|---|---|---|
| `title` | string | **required.** Notification title. |
| `body` | string | Notification body. |

This node logs an error and continues (it doesn't fail the workflow) when run in server mode where there's no desktop. Expected behavior — don't configure retries on it.

### Database

`type_id: database`

Query SQLite, PostgreSQL, MySQL, or Redis. Select the backend with `db_type`.

#### SQLite

| Parameter | Type | Notes |
|---|---|---|
| `db_type` | string | `sqlite` (default). |
| `db_path` | string | **required.** Absolute path ending in `.db`, `.sqlite`, or `.sqlite3`. No `..` path traversal. |
| `operation` | string | `query` (SELECT, default) or `execute` (INSERT / UPDATE / DELETE). |
| `query` | string | **required.** SQL query. Use `?` for placeholders. |
| `params` | string | JSON array of positional parameters. Example: `[42, "Alice"]` |


> **SQL injection warning:** Always use `?` placeholders and the `params` array for any value that comes from workflow data, trigger payloads, or external input. Never put workflow expressions directly inside the query string.
>
> **Safe:**
> ```
> query: "SELECT * FROM users WHERE id = ?"
> params: ["{{trigger.output.id}}"]
> ```
>
> **Unsafe (SQL injection risk):**
> ```
> query: "SELECT * FROM users WHERE id = '{{trigger.output.id}}'"
> ```
>
> When the server detects single-quoted literals in a resolved query, it appends a warning to the node's log output. This is a heuristic — it does not guarantee safety. Use parameterized queries unconditionally for any external data.

Output: `{ rows: [], rows_affected: 0, last_insert_id: 0, columns: [] }`

- `last_insert_id` — populated for SQLite `execute` operations. Always `0` for Postgres and MySQL (see notes below).
- Path must be absolute; relative paths and `..` traversal are rejected.
- Connection pooling is per path — multiple nodes querying the same file reuse the same pool.

#### PostgreSQL / MySQL

| Parameter | Type | Notes |
|---|---|---|
| `db_type` | string | `postgres` or `mysql`. |
| `connection_url` | string | **required.** Full connection URL including credentials. Use a Credential to avoid storing it in the workflow. Examples: `postgres://user:pass@host/dbname`  `mysql://user:pass@host/dbname` |
| `operation` | string | `query` (SELECT, default) or `execute` (INSERT / UPDATE / DELETE). |
| `query` | string | **required.** SQL query. Use `?` for placeholders (works for both Postgres and MySQL via the Any driver). |
| `params` | string | JSON array of positional parameters. Example: `[42, "Alice"]` |

Output: `{ rows: [], rows_affected: 0, last_insert_id: 0, columns: [] }`

**Notes:**
- Postgres does not expose `last_insert_id` via the Any driver. Use a `RETURNING id` clause in your INSERT and pass `operation: query` to retrieve the generated ID.
- MySQL `last_insert_id` is also `0` via the Any driver. Use `SELECT LAST_INSERT_ID()` in a follow-up Database node inside a transaction if you need it.
- Connection pools are lazily created per URL and reused across executions.

#### Redis

| Parameter | Type | Notes |
|---|---|---|
| `db_type` | string | `redis`. |
| `connection_url` | string | **required.** Redis URL. Example: `redis://host:6379` — use a Credential. |
| `operation` | string | **required.** One of: `get`, `set`, `del`, `lpush`, `rpush`, `lpop`, `rpop`, `hget`, `hset`. |
| `key` | string | **required.** Redis key. |
| `value` | string | Required for `set`, `lpush`, `rpush`, `hset`. |
| `field` | string | Required for `hget`, `hset`. Hash field name. |
| `expire` | number | Optional. TTL in seconds for `set`. Omit for no expiry. |

Output varies by operation:

| Operation | Output |
|---|---|
| `get`, `hget`, `lpop`, `rpop` | `{ value: string \| null }` |
| `set`, `hset` | `{ ok: true }` |
| `del` | `{ deleted: number }` |
| `lpush`, `rpush` | `{ ok: true, list_length: number }` |

Connections use a multiplexed async connection — multiple concurrent workflows share one connection to the same Redis instance.

---

### S3 Storage

`type_id: s3_storage`

Upload, download, list, delete, and presign objects in any S3-compatible object store — AWS S3, Cloudflare R2, or MinIO.

| Parameter | Type | Notes |
|---|---|---|
| `operation` | string | **required.** `upload`, `download`, `list`, `delete`, or `presign_url`. |
| `provider` | string | `aws` (default), `r2`, or `minio`. |
| `access_key_id` | string | **required.** Access key ID. |
| `secret_access_key` | string | **required.** Secret access key. |
| `bucket` | string | **required.** Bucket name. |
| `region` | string | AWS region (e.g. `us-east-1`). Default: `us-east-1`. For R2, use `auto`. Ignored for MinIO. |
| `endpoint` | string | Custom endpoint URL. **Required for `r2` and `minio`.** Examples: `https://ACCOUNT_ID.r2.cloudflarestorage.com`, `http://host:9000`. |
| `key` | string | Object key (path). Required for `upload`, `download`, `delete`, `presign_url`. |
| `content` | string | Content to upload. Text or base64 bytes. |
| `content_encoding` | string | `text` (default) or `base64`. |
| `content_type` | string | MIME type for upload. Default: `application/octet-stream`. |
| `prefix` | string | Key prefix filter for `list`. Empty string lists all objects. |
| `expiry_secs` | number | Presigned URL TTL in seconds. Default: `3600`. |

**Outputs by operation:**

| Operation | Output fields |
|---|---|
| `upload` | `{ key, size, content_type }` |
| `download` | `{ key, content (base64), content_encoding: "base64", size }` |
| `list` | `{ objects: [{ key, size, last_modified }], prefix, count }` |
| `delete` | `{ key, deleted: true }` |
| `presign_url` | `{ url, key, expires_in }` |

**Provider setup:**

- **AWS S3** — set `region`. No `endpoint` needed.
- **Cloudflare R2** — set `provider: r2`, `endpoint: https://ACCOUNT_ID.r2.cloudflarestorage.com`, `region: auto`.
- **MinIO** — set `provider: minio`, `endpoint: http://host:9000`, `region: us-east-1` (or any value).

Store credentials using the Credentials panel — reference them in `access_key_id` and `secret_access_key` fields with `{{credentials.my_s3_key}}`.

---

## Integrations

### Slack

`type_id: slack`

| Parameter | Type | Notes |
|---|---|---|
| `channel` | string | **required.** Channel ID (`C1234567890`) or name (`#general`). |
| `text` | string | **required.** Message text. |
| `api_key` | string | Slack Bot Token (`xoxb-...`) — use a Credential. |

Output: `{ ok: boolean, ts: string, channel: string }`

The bot must be added to the channel before posting. If `ok` is `false`, check the Logs tab — Slack's error messages are descriptive.

### Discord

`type_id: discord`

| Parameter | Type | Notes |
|---|---|---|
| `webhook_url` | string | **required.** Discord webhook URL (`https://discord.com/api/webhooks/...`). |
| `content` | string | **required.** Message text. Max 2000 characters. |
| `username` | string | Override the webhook's display name. |

Output: `{ sent: boolean }`

Uses Discord webhook URLs, not a bot token. Create a webhook in your server's channel settings.

### GitHub

`type_id: github`

| Parameter | Type | Notes |
|---|---|---|
| `action` | string | **required.** `create_issue` or `add_comment`. |
| `owner` | string | **required.** Repository owner (user or org). |
| `repo` | string | **required.** Repository name. |
| `title` | string | Issue title. Required for `create_issue`. |
| `body` | string | Issue body or comment text. |
| `issue_number` | number | Issue number. Required for `add_comment`. |
| `api_key` | string | GitHub personal access token — use a Credential. |

Output: `{ number: number, html_url: string, id: number, state: string }`

The token needs `repo` scope for private repositories, `public_repo` for public.

### Google Sheets

`type_id: google_sheets`

| Parameter | Type | Notes |
|---|---|---|
| `action` | string | **required.** `append_row` or `get_values`. |
| `spreadsheet_id` | string | **required.** The ID from the spreadsheet URL. |
| `range` | string | **required.** A1 notation, e.g. `Sheet1!A1:D1`. |
| `values` | array | Row data for `append_row`. Format: `[["col1", "col2"]]`. |
| `api_key` | string | Google OAuth 2.0 access token — use a Credential. |

Output: `{ values: array, updatedRange: string, updatedRows: number, updatedColumns: number, updatedCells: number }`

The `api_key` field takes a Google OAuth 2.0 access token — not a plain API key. Getting one requires setting up a Google Cloud service account. See [Credentials — Google Sheets](credentials.md#google-sheets) for the full setup steps. Once you have the service account, share the target spreadsheet with the service account's email address (`...@your-project.iam.gserviceaccount.com`) or it will return a 403 error.

### Notion

`type_id: notion`

| Parameter | Type | Notes |
|---|---|---|
| `action` | string | **required.** `create_page` or `update_page`. |
| `database_id` | string | Notion database ID. Required for `create_page`. |
| `page_id` | string | Notion page ID. Required for `update_page`. |
| `title` | string | Page title (used for the Name/title property). |
| `properties` | object | Additional Notion page properties as JSON. |
| `api_key` | string | Notion integration token (`secret_...`) — use a Credential. |

Output: `{ id: string, url: string, object: string, archived: boolean }`

Create an internal integration at [notion.so/my-integrations](https://www.notion.so/my-integrations) and share each database with it.

### Telegram

`type_id: telegram`

| Parameter | Type | Notes |
|---|---|---|
| `chat_id` | string | **required.** Chat ID or `@channelname`. |
| `text` | string | **required.** Message text. |
| `parse_mode` | string | `Markdown`, `HTML`, or empty. |
| `api_key` | string | Bot Token from [@BotFather](https://t.me/BotFather) — use a Credential. |

Output: `{ message_id: number, chat: object, text: string, date: number }`

### Stripe

`type_id: stripe`

| Parameter | Type | Notes |
|---|---|---|
| `action` | string | **required.** `create_payment_intent` (only action currently supported). |
| `amount` | number | **required.** Amount in the smallest currency unit. $10.00 USD = `1000`. |
| `currency` | string | Three-letter ISO code, lowercase. e.g. `usd`, `eur`. |
| `description` | string | Optional description. |
| `api_key` | string | Stripe secret key (`sk_live_...` or `sk_test_...`) — use a Credential. |

Output: `{ id: string, client_secret: string, status: string, amount: number, currency: string }`

Only `create_payment_intent` is supported. More Stripe operations are planned. If `max_attempts` > 1 is configured, a transient failure may create multiple payment intents — keep retries at 1 for payment nodes.

---

## Data & Utility

### Transform Data

`type_id: transform`

Extracts and remaps fields from a node's output into a new object.

| Parameter | Type | Notes |
|---|---|---|
| `source_node` | string | Node ID to pull output from. Uses the current input if omitted. |
| `mappings` | array | **required.** Array of `{ from: string, to: string }` objects. `from` is a JSON Pointer (`/field/subfield`). |

Output: object with the mapped keys.

```json
{
  "mappings": [
    { "from": "/body/user/name", "to": "username" },
    { "from": "/body/user/email", "to": "email" }
  ]
}
```

### JSON

`type_id: json_node`

Parse a JSON string or serialize a value to JSON.

| Parameter | Type | Notes |
|---|---|---|
| `action` | string | **required.** `parse` or `stringify`. |
| `value` | any | **required.** Value to parse or stringify. |

Output: `{ result: any }`

### Set Variable

`type_id: set_variable`

Store a value for use later in the same workflow run, or persist it across runs.

| Parameter | Type | Notes |
|---|---|---|
| `key` | string | **required.** Variable name. |
| `value` | any | Value to store (any JSON type). |
| `persist` | boolean | If `true`, write to the database so the value survives between runs. Default `false`. |

In server serve mode (single-workflow daemon), `persist: true` is a no-op with a warning logged. Persistence requires a `WorkflowDb` connection, which serve mode doesn't have.

### Get Variable

`type_id: get_variable`

Retrieve a variable set by a previous Set Variable node.

| Parameter | Type | Notes |
|---|---|---|
| `key` | string | **required.** Variable name to retrieve. |

Output: `{ key: string, value: any, found: boolean }`

### Output

`type_id: output_node`

Marks data as the workflow's final output. Useful when you want to explicitly label what a workflow produces.

| Parameter | Type | Notes |
|---|---|---|
| `value` | any | The value to emit as output. |

---

## Running a single node

Right-click any node on the canvas and choose **Run from here**. Flowo builds a subgraph containing that node and all its ancestors, then runs only that portion. The rest of the workflow is untouched.

This is the fastest way to test a single node's config during development without triggering downstream side effects (sending emails, posting to Slack, etc.).
