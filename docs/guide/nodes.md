# Nodes Reference

Every built-in **[node](../glossary.md#node)** Aerini ships with: 40 in total, one entry each. Each entry lists what the node does, its config fields (with the exact label you'll see in the app), its ports, and anything worth knowing before you rely on it.

This page assumes you already know what a node and a connection are. If "workflow," "node," or "trigger" aren't familiar yet, read [Concepts](../getting-started/concepts.md) first.

Nodes are grouped here the same way they're grouped in the app's node picker: Triggers, Core Actions, Files & Storage, Integrations, AI, Logic, and Utility.

A few things that apply across every node on this page, so they're not repeated 40 times:

- **Fields marked "resolved from Connections"** don't show a plain text box. They open the saved-credential picker instead, so the actual secret lives in Aerini's encrypted credential store, not typed into the node. See [Credentials](credentials.md).
- **Any text field can take a `{{...}}` expression** to pull in a value from an earlier node, a workflow variable, or the current run. Number and dropdown (enum) fields can't, since there's nowhere in a number box to type `{{`. See [Expressions](expressions.md) for the full syntax.
- **"Ports"** lists what a node connects to on each side. Unless noted otherwise, a node has one input ("In") and one output ("Out"), the same shape you'll see for most boxes on the canvas.

## Triggers

Every workflow needs exactly one of these to start it. A trigger has no input port; it's always the first node in the chain.

### Manual Trigger

Run a workflow yourself, by pressing Run in the app or calling the server API. No config required to use it.

| Field | Label | Notes |
|---|---|---|
| `mock_payload` | Mock Payload | Optional. A JSON string injected as the trigger's output when you run manually, useful for testing a workflow that expects webhook-shaped data without needing a real webhook call. |

**Ports:** no input. Output: Start.

### Webhook

Starts a workflow when an HTTP request hits a local address. Outputs the request body, headers, method, and path.

| Field | Label | Notes |
|---|---|---|
| `port` | Port | Port to listen on. Default 3456. In server/API mode, outside traffic can't reach `127.0.0.1` directly, so you'll need a reverse proxy in front of it. |
| `path` | Path | URL path to listen on. Default `/webhook`. |
| `method` | Method | `GET`, `POST`, `PUT`, or `ANY`. |
| `secret` | Secret | Optional shared secret, checked against an `x-webhook-secret` header. This confirms the caller knows the secret, but it does not sign the body, so a captured request can be replayed as-is. For providers that send their own body signature (Stripe, GitHub), verify their native signature header in a Code node right after this one instead of relying on this field alone. |
| `timeout_secs` | Timeout (seconds) | How long to wait for the workflow to finish before responding. Default 60. |
| `dedup_window_secs` | Dedup Window Secs | Above 0, a request with a body already seen in this many seconds won't re-run the workflow (the caller still gets a 200 OK). Catches providers like Stripe or GitHub that resend the same event on a timeout or non-2xx response. Only applies to requests with a body: GET requests and empty POSTs are never deduped. Only takes effect for a workflow set to run in the background; an ad-hoc Run press always runs once. Default 0 (off). |
| `validate_timestamp` | Validate Timestamp | When on, requires an `x-webhook-timestamp` header and rejects anything older than 5 minutes. This narrows the replay window but doesn't close it, since the timestamp isn't cryptographically tied to the body. Not compatible with GitHub, Stripe, or any provider that doesn't send that header. Default off. |

**Ports:** no input. Outputs: Triggered, Error.

### Schedule

Runs a workflow automatically: on a fixed interval, a cron expression, or once at a specific time.

| Field | Label | Notes |
|---|---|---|
| `mode` | Mode | `interval`, `cron`, or `once`. |
| `interval_secs` | Interval (seconds) | Interval mode. The app enforces a 10-second minimum; anything lower is reset to 10. |
| `cron_expr` | Cron Expression | Cron mode. Five-part format: `minute hour day month weekday`, e.g. `0 9 * * 1-5` for weekdays at 9 AM. The app offers a few common presets as quick-fill buttons. |
| `run_at` | Run At (ISO timestamp) | Once mode. Picked in your local time in the app, stored and sent to the engine as UTC. |

**Ports:** no input. Output: Triggered.

## Core Actions

### HTTP Request

Make an HTTP request to any URL. Supports GET, POST, PUT, PATCH, DELETE, custom headers, and a body.

| Field | Label | Notes |
|---|---|---|
| `url` | URL | Required. |
| `method` | Method | Required. `GET`, `POST`, `PUT`, `PATCH`, or `DELETE`. |
| `headers` | Headers | Request headers, as an object. Edited as JSON in **Advanced: full config**. |
| `body` | Body | Request body, for POST/PUT/PATCH. Plain text or an expression edits normally; a JSON object/array value is edited in **Advanced: full config** instead. |
| `api_key` | API Key | Resolved from Connections. |

**Ports:** In → Success, Error.

**Output:** `status`, `body`, `headers`.

### Shell Command

Run a shell command on the machine Aerini is on, and capture its stdout, stderr, and exit code.

| Field | Label | Notes |
|---|---|---|
| `command` | Command | Required. |
| `cwd` | Cwd | Working directory. |
| `env` | Env | Environment variables, as an object. Edited as JSON in **Advanced: full config**. |
| `timeout_secs` | Timeout (seconds) | Default 30. |

**Ports:** default (In → Out).

**Output:** `stdout`, `stderr`, `exit_code`, `success`.

**Gotcha:** this runs a real command on the host machine. Aerini treats Shell Command, Code (JS), and Database as "dangerous" nodes: any workflow containing one shows a one-time confirmation dialog before its first run in a session ("This workflow contains nodes that execute code on your computer... Only run workflows from sources you trust"), and the node gets a small warning badge on the canvas.

### Code (JS)

Run a JavaScript snippet with a full Node.js runtime and return its result.

| Field | Label | Notes |
|---|---|---|
| `code` | Code | Required. Call `output(value)` to return a result. Has access to `input` and `context`. |
| `timeout_secs` | Timeout (seconds) | Max execution time. Default 10, max 60. |

**Ports:** In → Out, Error.

**Output:** `result` (whatever you passed to `output()`), `stdout`, `duration_ms`, and `_sandbox_partial` (present and `true` on macOS when the code sandbox is active: ESM import restrictions are enforced there, but the CPU/memory limits are Linux-only and don't apply on macOS).

**Gotcha:** counted among the "dangerous" nodes (see Shell Command above); triggers the same confirmation dialog and canvas badge.

### Send Email

Send an email over SMTP, plain text or HTML, to one or more recipients.

| Field | Label | Notes |
|---|---|---|
| `smtp_host` | SMTP Host | Required, e.g. `smtp.gmail.com`. |
| `smtp_port` | SMTP Port | 587 (STARTTLS, recommended) or 465 (SSL). Default 587. |
| `from` | From | Required. Sender address. |
| `to` | To | Required. Comma-separated; 50 recipients max. |
| `subject` | Subject | Required. |
| `body` | Body | Required. |
| `html` | HTML | Send as HTML instead of plain text. Default off. |
| `username` | Username | SMTP username, usually your email address. |
| `password` | Password | Resolved from Connections. |

**Ports:** default (In → Out).

**Output:** `sent`.

### Desktop Notification

Show a desktop notification on the machine running the workflow, while it runs.

| Field | Label | Notes |
|---|---|---|
| `title` | Title | Required. |
| `body` | Body | Notification body text. |
| `urgency` | Urgency | `low`, `normal`, or `critical`. Linux only. Default `normal`. |

**Ports:** default (In → Out).

**Output:** `sent`.

### Database

Run SQL against SQLite, PostgreSQL, or MySQL, or key-value operations against Redis.

| Field | Label | Notes |
|---|---|---|
| `db_type` | Db Type | `sqlite`, `postgres`, `mysql`, or `redis`. Default `sqlite`. |
| `db_path` | Database Path | SQLite only. Absolute path ending in `.db`, `.sqlite`, or `.sqlite3`. |
| `connection_url` | Connection URL | Postgres/MySQL/Redis. Full connection string including credentials, e.g. `postgres://user:pass@host/db`. |
| `query` | Query | SQL query, using `?` for parameters. Not used for Redis. |
| `params` | Params | JSON array of values for the query's `?` placeholders, e.g. `[42, "Alice"]`. |
| `operation` | Operation | SQL: `query` (SELECT) or `execute` (INSERT/UPDATE/DELETE). Redis: `get`, `set`, `del`, `lpush`, `rpush`, `lpop`, `rpop`, `hget`, `hset`. |
| `key` | Key | Redis: key to operate on. |
| `value` | Value | Redis: value for `set`, `lpush`, `rpush`, `hset`. |
| `field` | Field | Redis: hash field name for `hget`/`hset`. |
| `expire` | Expire | Redis `set`: optional TTL in seconds. |
| `allow_local` | Allow Local | Postgres/MySQL/Redis only. Default off: connections to localhost, `127.0.0.1`, and private network ranges are blocked. Only turn this on for a database you're intentionally running yourself on this machine or LAN. |

**Ports:** In → Out, Error.

**Output (SQL):** `rows`, `rows_affected`, `last_insert_id` (SQLite and MySQL populate this; Postgres returns 0, use `RETURNING` instead), `columns`.

**Output (Redis):** `value`, `ok`, `deleted`, `list_length`.

**Gotcha:** the `query` field flatly rejects `{{...}}` expressions and fails the node instead of running. Values are inserted directly into the SQL string, so a numeric injection (`1 OR 1=1`) produces no quotes and can't be sanitized by pattern-matching. Use `?` placeholders in `query` and put the dynamic values in `params` instead, e.g. `query: "SELECT * FROM t WHERE id = ?"` with `params: ["{{HTTP Request.output.id}}"]`.

**Gotcha:** counted among the "dangerous" nodes (see Shell Command above); triggers the same confirmation dialog and canvas badge.

## Files & Storage

### File

Read, write, or delete a file at a path on the host machine.

| Field | Label | Notes |
|---|---|---|
| `operation` | Operation | Required. `read`, `write`, `append`, `delete`, or `exists`. |
| `path` | Path | Required. Absolute or relative. |
| `content` | Content | Content to write, for `write`/`append`. |
| `encoding` | Encoding | `utf8` or `base64`. Default `utf8`. |

**Ports:** default (In → Out).

**Output:** `content`, `exists`, `bytes`, `path`.

### Save to Folder

Save one or more files to a folder on disk, optionally split across subfolders.

| Field | Label | Notes |
|---|---|---|
| `folder_path` | Destination Folder | Set via a folder-picker button in the config panel, not a plain text field. |
| `overwrite` | Overwrite existing files | Checkbox. When off, a file with the same name is kept and the incoming one is skipped. Default on. |
| `filename_prefix` | Filename Prefix | Optional, prepended to every saved filename. Capped at 50 characters; a longer literal value is truncated in the field itself, and a longer `{{...}}`-resolved value is truncated at save time since the cap can't be enforced until the expression resolves. |
| Subfolders | Subfolders | Not a config field so much as a port builder: each subfolder you add in the config panel becomes its own input port, labeled with that subfolder's name. Connect a node that outputs a files array (Image Generation, Collect Files, S3 Storage) to route its files into that specific subfolder. |

**Ports:** dynamic. One input port per configured subfolder (each accepting files), or a single generic "In" port if no subfolders are configured. Output: Out.

**Output:** `saved`, `count`, `folder`, `skipped` (files not written because `overwrite` was off and a same-named file existed), `errors`.

### S3 Storage

Upload, download, list, or delete objects in an S3-compatible object store: AWS S3, Cloudflare R2, or MinIO.

| Field | Label | Notes |
|---|---|---|
| `operation` | Operation | `upload`, `download`, `list`, `delete`, or `presign_url`. |
| `provider` | Provider | `aws`, `r2`, or `minio`. Default `aws`. |
| `access_key_id` | Access Key ID | |
| `secret_access_key` | Secret Access Key | |
| `region` | Region | AWS region, e.g. `us-east-1` (default). For R2, use `auto`. Ignored for MinIO, use Endpoint instead. |
| `endpoint` | Endpoint | Custom endpoint URL. Required for R2 (`https://ACCOUNT_ID.r2.cloudflarestorage.com`) and MinIO (`http://host:9000`). Unused for AWS. |
| `bucket` | Bucket | Required for most operations. |
| `key` | Key | Object key/path. Required for `upload`, `download`, `delete`, `presign_url`. |
| `content` | Content | Content to upload, as text or base64 depending on Content Encoding. |
| `content_encoding` | Content Encoding | `text` or `base64`. Default `text`. |
| `content_type` | Content Type | MIME type for upload. Default `application/octet-stream`. |
| `prefix` | Prefix | Key prefix filter for `list`. Empty string lists everything. |
| `expiry_secs` | Expiry Secs | Presigned URL lifetime. Default 3600, maximum 604800 (7 days). |

**Ports:** In → Out, Error.

**Output:** `key`, `size`, `content_type` (upload only), `content` (download only, base64-encoded), `content_encoding` (always `base64` for a download), `objects` (list only, array of `{key, size, last_modified}`), `prefix`, `count` (list only), `deleted`, `url` (presign_url only), `expires_in`.

### Social Upload

Upload video or image content to YouTube, Instagram, or TikTok.

| Field | Label | Notes |
|---|---|---|
| `files` | Files | Required. Media files array, typically wired from an upstream node. |
| `platform` | Platform | Required. `youtube`, `instagram`, or `tiktok`. |
| `title` | Title | Required. |
| `description` | Description | Optional. |
| `tags` | Tags | Comma-separated. YouTube only. |
| `privacy` | Privacy | YouTube: `public`, `private`, or `unlisted`. TikTok: `public_to_everyone`, `mutual_follow_friends`, or `self_only`. |
| `client_id` | Client ID | OAuth client ID. Resolved from Connections. |
| `client_secret` | Client Secret | OAuth client secret. Resolved from Connections. |

**Ports:** default (In → Out).

**Output:** `uploaded`, `count`, `platform`, `errors`.

## Integrations

### Slack

Post a message to a Slack channel via `chat.postMessage`, using a bot token.

| Field | Label | Notes |
|---|---|---|
| `channel` | Channel | Required. Channel ID or name, e.g. `C1234567890` or `#general`. |
| `text` | Text | Required. |
| `api_key` | API Key | Slack Bot Token (`xoxb-...`). Resolved from Connections. |

**Ports:** default (In → Out).

**Output:** `ok`, `ts` (message timestamp), `channel`.

### Discord

Send a text message to a Discord channel via a webhook URL.

| Field | Label | Notes |
|---|---|---|
| `webhook_url` | Webhook URL | Required. `https://discord.com/api/webhooks/...`. |
| `content` | Content | Required. Maximum 2000 characters. |
| `username` | Username | Optional, overrides the webhook's display name for this message only. |

**Ports:** default (In → Out).

**Output:** `sent`.

### GitHub

Create an issue or add a comment via the GitHub API.

| Field | Label | Notes |
|---|---|---|
| `action` | Action | Required. `create_issue` or `add_comment`. |
| `owner` | Owner | Required. Repository owner, user or org. |
| `repo` | Repo | Required. |
| `title` | Title | Required for `create_issue`. |
| `body` | Body | Issue body or comment text. |
| `issue_number` | Issue Number | Required for `add_comment`. |
| `api_key` | API Key | GitHub personal access token. Resolved from Connections. |

**Ports:** default (In → Out).

**Output:** `number`, `html_url`, `id`, `state`.

**Gotcha:** only `create_issue` and `add_comment` are implemented. Pull requests and repository file edits aren't available through this node yet, even though they're a natural fit for "interact with GitHub."

### Google Sheets

Read from or write to a spreadsheet via the Sheets API.

| Field | Label | Notes |
|---|---|---|
| `action` | Action | Required. `append_row` or `get_values`. |
| `spreadsheet_id` | Spreadsheet ID | Required. From the sheet's URL. |
| `range` | Range | Required. A1 notation, e.g. `Sheet1!A1:D1`. |
| `values` | Values | Row data for `append_row`, an array of arrays, e.g. `[["a","b"]]`. |
| `client_id` | Client ID | Google OAuth client ID. Enables automatic token refresh; recommended. Entered directly on this node; not offered as a saved-credential picker field. |
| `client_secret` | Client Secret | Google OAuth client secret. Entered directly on this node; not offered as a saved-credential picker field. |
| `api_key` | API Key | A pasted OAuth 2.0 access token instead of client ID/secret. Does not auto-refresh and expires after about an hour. Only used when Client ID/Secret aren't set. |

**Ports:** default (In → Out).

**Output:** `values` (from `get_values`), `updatedRange`, `updatedRows`, `updatedColumns`, `updatedCells`.

### Notion

Create or update pages and database entries.

| Field | Label | Notes |
|---|---|---|
| `action` | Action | Required. `create_page` or `update_page`. |
| `database_id` | Database ID | Required for `create_page`. |
| `page_id` | Page ID | Required for `update_page`. |
| `title` | Title | Page title, used in the title property. |
| `properties` | Properties | Additional Notion page properties, as a JSON object. |
| `api_key` | API Key | Notion integration token (`secret_...`). Resolved from Connections. |

**Ports:** default (In → Out).

**Output:** `id`, `url`, `object`, `archived`.

### Telegram

Send a text message to a Telegram chat or channel via a bot.

| Field | Label | Notes |
|---|---|---|
| `chat_id` | Chat ID | Required. Numeric chat ID or `@channelname`. |
| `text` | Text | Required. |
| `parse_mode` | Parse Mode | Optional formatting: none, `Markdown`, or `HTML`. |
| `api_key` | API Key | Bot token from @BotFather. Resolved from Connections. |

**Ports:** default (In → Out).

**Output:** `message_id`, `chat`, `text`, `date`.

### SendGrid

Send a plain-text transactional email via the SendGrid API.

| Field | Label | Notes |
|---|---|---|
| `to_email` | To Email | Required. |
| `from_email` | From Email | Required. Must be a verified sender in SendGrid. |
| `subject` | Subject | Required. |
| `body` | Body | Required. Plain text. |
| `api_key` | API Key | SendGrid API key (`SG....`). Resolved from Connections. |

**Ports:** default (In → Out).

**Output:** `sent`, `message_id`.

### Stripe

Create a Stripe PaymentIntent.

| Field | Label | Notes |
|---|---|---|
| `action` | Action | Required. Only `create_payment_intent` is supported right now. |
| `amount` | Amount | In the smallest currency unit, e.g. `1000` for $10.00 USD. |
| `currency` | Currency | Three-letter lowercase ISO code, e.g. `usd`. |
| `description` | Description | Optional. |
| `api_key` | API Key | Stripe secret key (`sk_live_...` or `sk_test_...`). Resolved from Connections. |
| `idempotency_key` | Idempotency Key | Optional. Identifies this specific charge (e.g. an order ID) so a retry or re-run doesn't create a duplicate PaymentIntent. If left blank, one is derived automatically from this run and the resolved amount, currency, and description. |

**Ports:** default (In → Out).

**Output:** `id` (`pi_...`), `client_secret`, `status`, `amount`, `currency`.

## AI

### AI Prompt

Send a prompt to an AI model, Claude, GPT, Gemini, or a local Ollama model, and get a reply back.

| Field | Label | Notes |
|---|---|---|
| `prompt` | Prompt | Required. |
| `system` | System | System/persona instructions. |
| `model` | Model | Model name, e.g. `gpt-5.6`, `claude-sonnet-5`, `gemini-3.6-flash`, `llama3`. A **Fetch Models** button queries the provider's `/models` endpoint and offers a dropdown; typing a name directly always still works if the fetch fails or the provider doesn't support it. |
| `provider` | Provider | `auto`, `openai`, `anthropic`, `gemini`, or `local`. `auto` detects from Base URL, including local servers such as Ollama. |
| `base_url` | Base URL | API base URL. Leave blank for OpenAI. Loopback and private-network addresses are allowed here specifically, for local models like Ollama. See [Local Models](local-models.md) for how to point this at one. |
| `api_key` | API Key | Resolved from Connections. |
| `temperature` | Temperature | 0.0 (precise) to 2.0 (creative). Default 0.7. |
| `max_tokens` | Max Tokens | Default 2048. The real ceiling is whatever the provider/model you picked allows, not this field. |
| `rate_limit_rpm` | Rate Limit (RPM) | Max requests per minute. 0 = unlimited. |

**Ports:** In → Success, Error. A second, "Files" input port accepts a files array wired from an upstream node (Image Generation, Collect Files, Text to File), merged with any static attachments configured on the node itself.

**Output:** `content`, `model`, `provider`, `input_tokens`, `output_tokens`.

### AI Agent

Give an AI model a set of tools it can call to work toward a goal across multiple steps, instead of answering in one shot.

| Field | Label | Notes |
|---|---|---|
| `goal` | Goal | Required. What the agent should accomplish. Be specific and state constraints. |
| `system` | System | System instructions for the agent's persona and behavior. |
| `tools` | Tools | JSON array of tool definitions, each `{name, description, parameters}`. Used with OpenAI-compatible and Gemini providers. |
| `context` | Context | Extra context to hand the agent, e.g. from earlier nodes or variables. |
| `provider` | Provider | Required. `openai`, `anthropic`, `gemini`, `local`, or `auto`. OpenAI, Gemini, and `local` run a full tool-calling loop; Anthropic runs a single reasoning pass with no tool loop. |
| `model` | Model | E.g. `gpt-5.6` (OpenAI), `claude-sonnet-5` (Anthropic), `gemini-3.6-flash` (Gemini). A **Fetch Models** button queries the provider's `/models` endpoint and offers a dropdown; typing a name directly always still works if the fetch fails or the provider doesn't support it. |
| `base_url` | Base URL | API base URL, leave blank for the provider's default. Loopback and private-network addresses are allowed here too, for local models like Ollama; see [Local Models](local-models.md). |
| `api_key` | API Key | Resolved from Connections. |
| `max_iterations` | Max Iterations | Max think-act-observe cycles. Default 5, max 20. Ignored for Anthropic, which always runs 1. |
| `max_tokens` | Max Tokens | Per response. Default 2048. Total spend scales with Max Iterations. |
| `temperature` | Temperature | 0.0 to 1.0. Agents tend to work best around 0.2 to 0.5. |

**Ports:** In → Done, Error. A second, "Files" input port works the same way as AI Prompt's.

**Output:** `result`, `iterations`, `tool_calls`, `reasoning`, `finished` (false if it hit Max Iterations or the response was cut off), `truncated` (true if the provider cut the response off mid-generation; raise Max Tokens).

### AI Memory

Read or write persistent AI conversation history, scoped per session, so an AI node can remember earlier turns.

| Field | Label | Notes |
|---|---|---|
| `operation` | Operation | Required. `read` (get history as a messages array), `write` (replace history), `append` (add one message), or `clear` (delete all). |
| `session_id` | Session ID | Required. Unique ID for the conversation thread, e.g. `user_123`. |
| `role` | Role | `user`, `assistant`, or `system`. Required for write/append. |
| `content` | Content | Message content. Required for write/append. |
| `max_messages` | Max Messages | Max messages returned on read, newest first. Default 20. |
| `max_stored` | Max Stored | Max messages retained per session; oldest are pruned after an append past this. Default 1000, minimum 1. |

**Ports:** default (In → Out).

**Output:** `messages` (array of `{role, content}`), `count`, `session_id`.

The [Chat Panel](chat-panel.md)'s Clear button and session delete call this node's `clear` operation directly, using the Chat Panel's own session ID — if a workflow uses that same ID for its AI Memory calls, clearing a chat session clears its memory too.

### Text Splitter

Split a long piece of text into smaller chunks, by character count, word count, sentence count, or paragraph boundaries.

| Field | Label | Notes |
|---|---|---|
| `text` | Text | Required. |
| `mode` | Mode | `chars`, `words`, `sentences`, or `paragraphs`. Default `chars`. |
| `chunk_size` | Chunk Size | Max size of each chunk, in the unit `mode` sets. Default 1000 chars / 200 words / 5 sentences. |
| `overlap` | Overlap | How much each chunk overlaps the previous one, same unit as Chunk Size. Helps an AI model keep context across chunks. Default 100. |
| `source_field` | Source Field | Optional dot-path to pull the text from the input data instead of typing it directly, e.g. `body`. |

**Ports:** In → Chunks.

**Output:** `chunks`, `total_chunks`, `total_chars`, `mode`, `chunk_size`, `overlap`.

### Image Generation

Generate images from a text prompt. Cloud providers: GPT Image 1/2 (OpenAI), Nano Banana/Gemini (Google), Flux Pro/2 Pro (BFL). Local: Automatic1111, ComfyUI.

| Field | Label | Notes |
|---|---|---|
| `prompt` | Prompt | Required. |
| `provider` | Provider | Required. `gpt_image_1`, `gpt_image_2`, `flux_pro`, `flux_2_pro`, `imagen4`, `nano_banana`, `dalle3`, `a1111`, `comfyui`. `gpt_image_1` is the recommended cloud default; `a1111`/`comfyui` are for local inference. `imagen4`/`nano_banana` both route to NanoBanana, and `dalle3` is a legacy alias that routes to `gpt_image_1`. |
| `n` | N | Images to generate, 1 to 4. Default 1. GPT Image sends all of them in one call; NanoBanana and Flux loop one call per image. |
| `size` | Size | GPT Image only. `gpt_image_1`: `1024x1024`, `1536x1024`, `1024x1536`, or `auto`. `gpt_image_2`: any width x height divisible by 16, or the same presets. Default `1024x1024`. |
| `quality` | Quality | GPT Image only. `auto`, `low`, `medium`, `high`. Default `auto`. |
| `aspect_ratio` | Aspect Ratio | NanoBanana/Gemini only. One of `1:1 1:4 1:8 2:3 3:2 3:4 4:1 4:3 4:5 5:4 8:1 9:16 16:9 21:9`. Default `1:1`. |
| `width` | Width | Flux only. `flux_pro`: 256 to 1440, rounded to the nearest 32. `flux_2_pro`: minimum 64. Default 1024. |
| `height` | Height | Flux only. Same rules as Width. Default 768 (`flux_pro`) or 1024 (`flux_2_pro`). |
| `api_key` | API Key | Resolved from Connections. OpenAI key for GPT Image, Google AI key for NanoBanana, BFL key for Flux. Not used for a1111 or comfyui. |
| `base_url` | Base URL | Local provider URL, e.g. `http://127.0.0.1:7860` for a1111, `http://127.0.0.1:8188` for comfyui. Can embed credentials: `http://user:pass@host:port`. |
| `negative_prompt` | Negative Prompt | a1111 only. Things to exclude from the image. |
| `steps` | Steps | a1111 only. Sampling steps, 1 to 150. Default 20. |
| `cfg_scale` | Cfg Scale | a1111 only. Classifier-free guidance scale. Default 7.0. |
| `username` | Username | a1111 only. Basic auth username, overrides any username already in Base URL. |
| `password` | Password | a1111 only. Basic auth password, overrides any password already in Base URL. |
| `timeout_seconds` | Timeout Seconds | a1111 only. HTTP timeout. Raise for large batches or high step counts on slower hardware. Default 300, max 1800 (30 minutes). |
| `workflow` | Workflow | comfyui only. A workflow exported in ComfyUI API format (Settings → Enable Dev Mode → Save API Format). |

**Ports:** In, plus a "Reference Images" input port for image-to-image providers that accept them. Output: Out.

**Output:** `files` (generated images as media objects), `count`, `source` (which provider actually generated them).

## Logic

### If / Condition

Branch the workflow on a condition. True paths go to one output, false to another.

| Field | Label | Notes |
|---|---|---|
| `condition` | Condition | E.g. `{{temperature_2m}} > 20`, `{{status}} == ok`, `{{count}} >= 5`. See [Expressions](expressions.md) for exactly how this gets evaluated, it's a smaller comparison language, not the full expression function set. |

**Ports:** In → True, False.

**Gotcha:** an empty or blank condition doesn't error, it evaluates to false and logs "No condition specified, defaulting to false."

### Switch

Route the workflow to one of several branches based on a value match, like a switch statement.

| Field | Label | Notes |
|---|---|---|
| `field` | Field | Required. Dot-path to switch on, e.g. `status` or `response.code`. |
| `source_node` | Source Node | Required. Which upstream node's output to read Field from. |
| `cases` | Cases | Required. A JSON array of cases, e.g. `[{"match": "ok", "port": "case_1"}]`. |
| `default_port` | Default Port | Port to route to when nothing matches. Default `default`. |

**Ports:** In → Case 1 through Case 8 (eight fixed case ports), plus Default. Case labels on the canvas are generic; the app reads your actual case config to show the label you gave each one alongside the port ID.

**Gotcha:** a case with no `port` set gets auto-assigned to its position (`case_1`, `case_2`, ...); explicit ports are validated against the real port set, so a typo or an out-of-range case number fails the node loudly at config time instead of silently misrouting at run time. Only 8 case ports exist; a 9th case with no explicit port set fails for the same reason.

### Loop (For Each)

Run a downstream branch once for each item in a list, then continue with the collected results.

| Field | Label | Notes |
|---|---|---|
| `array_field` | Array Field | Required. Dot-path to the array to iterate, e.g. `items` or `response.results`. |
| `source_node` | Source Node | Required. Which upstream node's output to read Array Field from. |
| `item_var` | Item Variable | Variable name for the current item. Default `item`. |
| `index_var` | Index Variable | Variable name for the current index. Default `index`. |
| `max_iterations` | Max Iterations | Stop after this many items instead of the full array. Blank or 0 means no limit. Can only lower the effective bound, a value larger than the array (or larger than the hard 10,000-item cap) has no extra effect. |

**Ports:** In → Each Item, Done.

**Output:** `items`, `total`, `item`, `index`, `all_results`, `done`.

**Gotcha:** 10,000 items is a hard cap regardless of Max Iterations. Item Variable and Index Variable can't be set to one of the node's own reserved output keys (`total`, `item`, `index`, `all_results`, `done`, and a couple of internal loop-control keys), and the two can't be set to the same name as each other, both fail the node at config time rather than silently corrupting the loop.

### Stop

End this branch of the workflow. Downstream nodes on this path don't run; other independent branches are unaffected.

| Field | Label | Notes |
|---|---|---|
| `reason` | Reason | Optional. Logged when the branch stops. |

**Ports:** In only, no output. This is the one node on this page with no output port at all, since its whole job is to end a branch.

### Merge

Wait for all incoming parallel branches to finish, then continue as one execution path.

| Field | Label | Notes |
|---|---|---|
| `mode` | Mode | `object` (combine inputs keyed by node ID) or `array` (values only, no keys). |

**Ports:** default (In → Out). In practice this node has multiple incoming connections, one per branch it's merging.

**Output:** all upstream node outputs merged into one value, shaped per Mode.

### Collect Files

Gather files from multiple upstream sources into a single list for downstream processing.

| Field | Label | Notes |
|---|---|---|
| Sources | Sources | Not a plain config field, each source you add in the config panel becomes its own input port. Connect an upstream node to each one. |

**Ports:** dynamic. One input port per configured source, or a single generic "In" port if none are configured yet. Output: Out.

**Output:** `files` (merged from every source), `count`, `source`.

**Gotcha:** despite dealing entirely in files, this node's category is Logic, not Files & Storage, in the node picker. That's a backend classification quirk, not a hint about what it does.

## Utility

### Delay

Pause the workflow for a fixed duration before continuing to the next node.

| Field | Label | Notes |
|---|---|---|
| `duration` | Duration | How long to wait. Default 1. |
| `unit` | Unit | `ms`, `s`, `m`, or `h`. Default `s`. |

**Ports:** default (In → Out).

### Wait

Pause the workflow for a fixed duration, or poll a field until it matches an expected value.

| Field | Label | Notes |
|---|---|---|
| `mode` | Mode | Required. `duration` (wait a fixed time) or `condition` (poll until a value matches). |
| `duration_secs` | Duration (seconds) | Duration mode. Default 5. |
| `field` | Field | Condition mode. Dot-path to check, e.g. `node_http.status`. |
| `expected` | Expected | Condition mode. The value Field must equal to stop waiting. |
| `poll_interval_secs` | Poll Interval (seconds) | How often to recheck Field. Default 2. |
| `timeout_secs` | Timeout (seconds) | Max time to wait before routing to Timed Out instead. Default 60. |

**Ports:** In → Done, Timed out.

**Output:** `waited_ms`, `timed_out`, `mode`.

**Gotcha:** this is a separate node from Delay, with a separate purpose. Delay is a plain fixed pause with one output port. Wait adds condition-polling and a dedicated Timed Out branch for when the condition never matches in time.

### Transform Data

Reshape or extract data from the previous node's output using a template or expression.

| Field | Label | Notes |
|---|---|---|
| `source_node` | Source Node | Required. Which node to pull output from. |
| `mappings` | Mappings | Required. Array of `{from, to}` pairs: `from` is a JSON pointer into the source (e.g. `/user/name`), `to` is the key it lands on in this node's output. Edited as JSON in **Advanced: full config**. |

**Ports:** default (In → Out).

**Output:** the mapped fields, one key per `mappings` entry.

### JSON

Parse a JSON string into an object, or serialize an object into a JSON string.

| Field | Label | Notes |
|---|---|---|
| `operation` | Operation | Required. `parse`, `stringify`, `extract`, `merge`, or `array_get`. |
| `input_text` | Input Text | The JSON string to parse. |
| `pointer` | Pointer | JSON pointer for `extract`, e.g. `/user/name` or `/0/temperature`. |
| `index` | Index | Array index for `array_get`. Default 0. |

**Ports:** default (In → Out).

**Output:** `result`.

### Set Variable

Store a value in a named workflow variable, so any node downstream in the same run can read it back, either through the Get Variable node or an expression.

| Field | Label | Notes |
|---|---|---|
| `key` | Key | Required. Variable name. |
| `value` | Value | Value to store, any JSON type. |
| `persist` | Persist | When on, also saves the value to disk so it survives between runs, instead of only lasting for this one. Default off. In server mode running a single workflow with no database attached, this has no effect: the value still works for the rest of the current run, and the workflow logs a warning that nothing was saved to disk. |

**Ports:** default (In → Out).

**Output:** `key`, `value`.

**Gotcha:** a variable set here is readable by name from the moment this node succeeds onward, using `{{$vars.key}}` in any downstream node's field, or from a Get Variable node placed later in the same branch. See [Expressions](expressions.md) for the `$vars` syntax.

### Get Variable

Read a named workflow variable set earlier in the run by a Set Variable node.

| Field | Label | Notes |
|---|---|---|
| `key` | Key | Required. Variable name to retrieve. |
| `persist` | Persist | When on, falls back to the on-disk value if the variable wasn't set earlier in this run. Default off. |
| `default` | Default | Value to return if the variable isn't found anywhere. |

**Ports:** default (In → Out).

**Output:** `key`, `value`, `found`.

**Gotcha:** `{{$vars.key}}` does the same lookup inline, without a dedicated node. Reach for Get Variable when you want the missing-variable case to be an explicit, visible step in the workflow (with its own Default field); reach for `$vars` when you just need the value slotted into another field's text.

### Text to File

Convert a text string into a file object. Commonly used between AI Prompt and Save to Folder, to write an AI model's reply to disk. Sets the filename, extension, and MIME type together through a combined picker.

The config panel replaces the raw Filename/MIME Type/Format fields with a combined picker: pick a format and the filename's extension and MIME type update together; type your own filename and, if it ends in a recognized extension, format and MIME type sync back the other way.

| Field | Label | Notes |
|---|---|---|
| `filename` | File Name | E.g. `report.pdf`. |
| `format` | Format | Set by the Format picker: Plain Text (`.txt`), Markdown (`.md`), HTML (`.html`), CSV (`.csv`), JSON (`.json`), PDF (`.pdf`), or Word (`.docx`). |
| `mime_type` | Mime Type | Auto-set by the Format picker. Only override this directly if you actually need a non-standard MIME type for the format you picked. |

**Ports:** default (In → Out).

### Output

Mark the final output of the workflow. Required for the Chat Panel and the [embeddable widget](widget-embedding.md).

| Field | Label | Notes |
|---|---|---|
| `label` | Label | Shown above the result. Default `Result`. |
| `source_node` | Source Node | Which node's output to display. Leave blank to use whatever data reaches this node directly. |
| `field` | Field | A specific field to pull out, e.g. `content` or `body.text`. Leave blank to show everything. |

**Ports:** default (In → Out).

**Output:** `value`, `label`, `type` (detected type: string, number, boolean, object, array, or null), `output_type` (set to `media_batch` when Value is a media payload).

## What's next

- [Expressions](expressions.md): the `{{...}}` syntax referenced throughout this page, full syntax and available context.
- [Concepts](../getting-started/concepts.md): the mental model this page assumes.
- [Glossary](../glossary.md): quick lookup for any term on this page.
