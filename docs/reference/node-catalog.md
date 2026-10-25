# Node Catalog

Every built-in node, generated from the engine (40 nodes, engine 0.4.3, workflow schema version 1.0). The same data is in `docs/reference/node-schemas.json` as machine-readable JSON Schema. For what each node does in practice, gotchas and examples, see the [Nodes Reference](../guide/nodes.md).

This page is generated. Do not edit it by hand; see [Regenerating the reference files](../development/testing.md#regenerating-the-reference-files).

**Required** means the node's schema requires the field. Some nodes check more when they run, for example a field that is only needed for one setting of another field, so a config that fits the schema can still fail.

## Action nodes

### Code (JS)

`code` version 1.0.0

Execute a JavaScript snippet and return its result. Full Node.js runtime available.

Ports. In: `input` (In); Out: `output` (Out), `on_error` (Error).

| Field | Type | Required | Notes |
|---|---|---|---|
| `code` | string | Yes | JavaScript to run. Use output(value) to return a result. Has access to 'input' and 'context'. |
| `timeout_secs` | number | No | Max execution time in seconds (1 to 60, default 10) |

Output fields: `_sandbox_partial`, `duration_ms`, `result`, `stdout`, `truncated`.

### Database

`database` version 2.0.0

Run SQL queries against a SQLite, PostgreSQL, or MySQL database, or key-value operations against Redis, and return the results.

Ports. In: `input` (In); Out: `output` (Out), `on_error` (Error).

| Field | Type | Required | Notes |
|---|---|---|---|
| `allow_local` | boolean | No | Postgres / MySQL / Redis only. Default: false — connections to localhost, 127.0.0.1, and private-network addresses (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, etc.) are blocked (SSRF protection). Set true only when connection_url intentionally targets a database you run yourself on this machine or LAN. |
| `allow_raw_sql` | boolean | No | SQL only. Default: false. When false, a query containing single-quoted string literals (or double-quoted ones on MySQL) is refused so values go through '?' and 'params'. Only an admin caller (a server run with admin rights) can turn this on; the desktop app ignores it. |
| `connection_url` | string | No | Postgres / MySQL / Redis. Connection URL. Examples: postgres://user@host/db  mysql://user@host/db  redis://host:6379. Leave the password out and use Password below to keep it out of the workflow file; a password written in the URL still works. |
| `db_path` | string | No | SQLite only. Absolute path ending in .db, .sqlite, or .sqlite3. |
| `db_type` | string | No | Database backend. Default: sqlite. One of: `sqlite`, `postgres`, `mysql`, `redis`. |
| `expire` | number | No | Redis set: optional TTL in seconds. Omit for no expiry. |
| `field` | string | No | Redis: hash field name for hget and hset. |
| `key` | string | No | Redis: key to operate on. |
| `operation` | string | No | SQL: query (SELECT) or execute (INSERT/UPDATE/DELETE). Redis: get, set, del, lpush, rpush, lpop, rpop, hget, hset. |
| `params` | string | No | JSON array of positional parameters for ? placeholders. Example: [42, "Alice"] |
| `password` | string | No | Postgres / MySQL / Redis. Password for connection_url. Select a saved credential; it replaces any password written in the URL. |
| `query` | string | No | SQL query. Use ? for parameters. Not used for Redis — use operation + key/value/field. |
| `value` | string | No | Redis: value for set, lpush, rpush, hset. |

Output fields: `columns`, `deleted`, `last_insert_id`, `list_length`, `ok`, `rows`, `rows_affected`, `truncated`, `value`.

### Discord

`discord` version 1.0.0

Send a text message to a Discord channel via a webhook URL.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `content` | string | Yes | Message content (max 2000 characters) |
| `webhook_url` | string | Yes | Discord webhook URL (https://discord.com/api/webhooks/...) |
| `allow_everyone_mentions` | boolean | No | Let @everyone and @here in the content ping the channel (default false). The webhook's channel permissions can still block it. |
| `username` | string | No | Override the webhook's display name (optional) |

Output fields: `sent`.

### Send Email

`email_send` version 2.0.0

Send an email via SMTP with plain text or HTML content and multiple recipients.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `body` | string | Yes |  |
| `from` | string | Yes | Sender email address |
| `smtp_host` | string | Yes | SMTP server e.g. smtp.gmail.com |
| `subject` | string | Yes |  |
| `to` | string | Yes | Recipient(s): comma-separated, or an array of addresses from an upstream node. A comma inside quotes or &lt;angle brackets&gt;, as in "Doe, John" &lt;a@b.c&gt;, does not split. Maximum 50 recipients. |
| `html` | boolean | No | Send as HTML email (default: false) Default: `false`. |
| `password` | string | No |  |
| `smtp_port` | number | No | 587 (STARTTLS, recommended) or 465 (SSL) Default: `587`. |
| `username` | string | No | SMTP username (usually your email address) |

Output fields: `sent`.

### File

`file` version 1.0.0

Read, write, or delete a file at a given path on the host machine.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `operation` | string | Yes | One of: `read`, `write`, `append`, `delete`, `exists`. |
| `path` | string | Yes | Absolute or relative file path |
| `content` | string | No | Content to write (write/append only; required, use an empty string for an empty file) |
| `encoding` | string | No | Default: utf8. Reading a file that is not valid UTF-8 as utf8 fails; use base64 for binary files. One of: `utf8`, `base64`. |

Output fields: `bytes`, `content`, `exists`, `path`.

### GitHub

`github` version 1.0.0

Interact with the GitHub API: create issues, pull requests, comments, and repository files.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `action` | string | Yes | Operation to perform One of: `create_issue`, `add_comment`. |
| `owner` | string | Yes | Repository owner (user or org) |
| `repo` | string | Yes | Repository name |
| `api_key` | string | No | GitHub personal access token |
| `body` | string | No | Issue body or comment text |
| `issue_number` | number | No | Issue number (required for add_comment) |
| `title` | string | No | Issue title (required for create_issue) |

Output fields: `html_url`, `id`, `number`, `state`.

### Google Sheets

`google_sheets` version 1.0.0

Read from or write to a Google Sheets spreadsheet using the Sheets API.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `action` | string | Yes | Operation to perform One of: `append_row`, `get_values`. |
| `range` | string | Yes | A1 notation range (e.g. Sheet1!A1:D1) |
| `spreadsheet_id` | string | Yes | Google Sheets spreadsheet ID (from the URL); the full sheet address also works |
| `api_key` | string | No | Google OAuth 2.0 access token, pasted directly. Does not auto-refresh (expires after ~1 hour). Only used when client_id/client_secret are not set. |
| `client_id` | string | No | Google OAuth client ID — enables automatic token refresh (recommended). Entered directly here; not currently offered as a saved-credential picker field. |
| `client_secret` | string | No | Google OAuth client secret — enables automatic token refresh (recommended). Pick a saved credential or enter it directly. Takes a saved credential. |
| `value_input` | string | No | append_row only. RAW (default) stores values as typed. USER_ENTERED parses them as if typed in the sheet: formulas run, dates and numbers are converted. Do not use USER_ENTERED with data you do not control: a value starting with = becomes a formula. One of: `RAW`, `USER_ENTERED`. |
| `values` | any | No | Row data for append_row — rows of cells as JSON, e.g. [["a","b"]]. Stored as typed (RAW) unless value_input is USER_ENTERED. |

Output fields: `updatedCells`, `updatedColumns`, `updatedRange`, `updatedRows`, `values`.

### HTTP Request

`http_request` version 1.0.0

Make an HTTP request to any URL. Supports GET, HEAD, OPTIONS, POST, PUT, PATCH, DELETE, and custom headers and body.

Ports. In: `input` (In); Out: `output` (Success), `on_error` (Error).

| Field | Type | Required | Notes |
|---|---|---|---|
| `method` | string | Yes | One of: `GET`, `HEAD`, `OPTIONS`, `POST`, `PUT`, `PATCH`, `DELETE`. |
| `url` | string | Yes | Request URL |
| `api_key` | string | No | Resolved from credentials |
| `body` | any | No | Request body (for POST/PUT/PATCH) |
| `headers` | object | No | Request headers |

Output fields: `body`, `headers`, `status`.

### Manual Trigger

`manual_trigger` version 1.0.0

Run a workflow manually via the Run button in the app or the server API.

Ports. In: none; Out: `output` (Start).

| Field | Type | Required | Notes |
|---|---|---|---|
| `mock_payload` | string | No | Optional JSON payload to inject when running manually |

### Desktop Notification

`notification` version 1.0.0

Show a desktop notification on the host machine while the workflow is running.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `title` | string | Yes | Notification title |
| `body` | string | No | Notification message body |
| `urgency` | string | No | Urgency level (Linux only) One of: `low`, `normal`, `critical`. Default: `normal`. |

Output fields: `sent`.

### Notion

`notion` version 1.0.0

Create or update pages and database entries in Notion.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `action` | string | Yes | Operation to perform One of: `create_page`, `update_page`. |
| `api_key` | string | No | Notion integration token (secret_...) |
| `data_source_id` | string | No | Notion data source ID, for a database with several data sources. Use instead of database_id, not with it |
| `database_id` | string | No | Notion database ID (32 hex digits, with or without hyphens) or a link to the database — create_page needs this or data_source_id |
| `page_id` | string | No | Notion page ID — required for update_page |
| `properties` | any | No | Additional Notion page properties as a JSON object, e.g. {"Status": {"select": {"name": "Done"&#125;&#125;} |
| `title` | string | No | Page title, written to the title property (see Title Property) |
| `title_property` | string | No | Name of the database's title property (default Name). Set it when the database calls its title column something else |

Output fields: `archived`, `id`, `object`, `url`.

### S3 Storage

`s3_storage` version 1.0.0

Upload, download, list, or delete objects in an S3-compatible object store.

Ports. In: `input` (In); Out: `output` (Out), `on_error` (Error).

| Field | Type | Required | Notes |
|---|---|---|---|
| `access_key_id` | string | No | Access key ID. |
| `bucket` | string | No | Bucket name. |
| `content` | string | No | Content to upload. Text or base64 bytes depending on content_encoding. |
| `content_encoding` | string | No | Encoding of content field. Default: text. One of: `text`, `base64`. |
| `content_length` | number | No | presign_url with PUT: exact size in bytes the upload must have, up to 5 GiB. Signed into the URL; the uploader must send the same Content-Length. |
| `content_type` | string | No | MIME type for upload (default application/octet-stream). With presign_url and PUT, signed into the URL: the uploader must send the same Content-Type. Not allowed with GET. |
| `delimiter` | string | No | List only: group keys that share the text between the prefix and the first delimiter (usually '/') into common_prefixes, like folders. Empty lists every key. |
| `endpoint` | string | No | Custom endpoint URL. Required for r2 (https://ACCOUNT_ID.r2.cloudflarestorage.com) and minio (http://host:9000). Must be empty for aws. |
| `expiry_secs` | number | No | Presigned URL expiry in seconds. Default: 3600. Maximum: 604800 (7 days). |
| `key` | string | No | Object key (path). Required for upload, download, delete, presign_url. |
| `max_objects` | number | No | Most entries a list returns, 1 to 10000. Default: 10000. |
| `method` | string | No | HTTP method the presigned URL is valid for: GET downloads the object, PUT uploads it. Default: GET. One of: `GET`, `PUT`. |
| `operation` | string | No | Operation to perform. One of: `upload`, `download`, `list`, `delete`, `presign_url`. |
| `prefix` | string | No | Key prefix filter for list. Empty string lists all objects. |
| `provider` | string | No | Storage provider. Default: aws. One of: `aws`, `r2`, `minio`. |
| `region` | string | No | AWS region (e.g. us-east-1). Default: us-east-1. For R2, use 'auto'. Ignored for MinIO — use endpoint. |
| `secret_access_key` | string | No | Secret access key. Takes a saved credential. |

Output fields: `common_prefixes`, `content`, `content_encoding`, `content_type`, `count`, `deleted`, `expires_in`, `headers`, `key`, `max_objects`, `method`, `objects`, `prefix`, `size`, `truncated`, `url`.

### Save to Folder

`save_to_folder` version 1.0.0

Save one or more files to a folder on disk with optional subfolder organisation.

Ports. In: `input` (In); Out: `output` (Out). The ports depend on the config, so these are the defaults.

| Field | Type | Required | Notes |
|---|---|---|---|
| `filename_prefix` | string | No | Optional prefix prepended to every saved filename |

Output fields: `count`, `errors`, `folder`, `saved`, `skipped`.

### Schedule

`schedule` version 1.0.0

Run a workflow on a cron schedule, fixed interval, or one-time delay.

Ports. In: none; Out: `output` (Triggered).

| Field | Type | Required | Notes |
|---|---|---|---|
| `mode` | string | Yes | One of: `interval`, `cron`, `once`. |
| `cron_expr` | string | No | Cron expression e.g. 0 9 * * 1-5 |
| `interval_secs` | number | No | Seconds between runs (interval mode) |
| `run_at` | string | No | ISO timestamp (once mode) |

Output fields: `mode`, `triggered_at`.

### SendGrid

`sendgrid` version 1.0.0

Send a plain-text transactional email via the SendGrid API.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `body` | string | Yes | Email body (plain text) |
| `from_email` | string | Yes | Sender email address (must be verified in SendGrid). Write the bare address; use from_name for a display name. |
| `subject` | string | Yes | Email subject line |
| `to_email` | string | Yes | Recipient email address, or several separated by commas (up to 1000; each gets a separate copy) |
| `api_key` | string | No | SendGrid API key (SG....) |
| `from_name` | string | No | Sender display name (optional, max 200 characters) |

Output fields: `message_id`, `sent`.

### Shell Command

`shell_exec` version 1.0.0

Run a shell command on the host machine and capture its stdout, stderr, and exit code.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `command` | string | Yes | Shell command to run |
| `cwd` | string | No | Working directory |
| `env` | object | No | Environment variables |
| `timeout_secs` | number | No | Max run time in seconds (1 to 86400, default 30) Default: `30`. |

Output fields: `exit_code`, `stderr`, `stdout`, `success`, `truncated`.

### Slack

`slack` version 1.0.0

Post a message to a Slack channel via the Slack API (chat.postMessage) using a Bot Token.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `channel` | string | Yes | Channel ID or name (e.g. C1234567890 or #general) |
| `text` | string | Yes | Message text |
| `api_key` | string | No | Slack Bot Token (xoxb-...) |

Output fields: `channel`, `ok`, `ts`.

### Social Upload

`social_upload` version 1.0.0

Upload video or image content to YouTube, Instagram, or TikTok. Instagram takes a public URL as the file's data. Fails when no file was uploaded.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `files` | array | Yes | Media contract files array. For Instagram, each file's data is a public http(s) URL. |
| `platform` | string | Yes | One of: `youtube`, `instagram`, `tiktok`. |
| `title` | string | Yes | Post title. TikTok and Instagram use it as the start of the caption. |
| `client_id` | string | No | OAuth client ID / client key. Entered directly here; not currently offered as a saved-credential picker field. |
| `client_secret` | string | No | OAuth client secret. Pick a saved credential or enter it directly. Takes a saved credential. |
| `description` | string | No | Post description (optional). Added after the title in a TikTok or Instagram caption. |
| `privacy` | string | No | Privacy level (default private). YouTube: public\|private\|unlisted. TikTok: public_to_everyone\|mutual_follow_friends\|follower_of_creator\|self_only. Ignored by Instagram. |
| `tags` | string | No | Comma-separated tags (YouTube) |

Output fields: `count`, `errors`, `failed`, `platform`, `uploaded`.

### Stripe

`stripe` version 1.0.0

Create a Stripe PaymentIntent via the Stripe API.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `action` | string | Yes | Stripe operation to perform One of: `create_payment_intent`. |
| `amount` | number | No | Amount in the smallest currency unit (e.g. 1000 for $10.00 USD) |
| `api_key` | string | No | Stripe secret key (sk_live_... or sk_test_...) |
| `currency` | string | No | Three-letter ISO currency code, lowercase (e.g. 'usd') |
| `description` | string | No | Optional description for this payment intent |
| `idempotency_key` | string | No | Optional value identifying this specific charge (e.g. an order ID) — prevents Stripe from creating a duplicate PaymentIntent if this node is retried or re-run with the same value. If omitted, a key is derived automatically from this run and the resolved amount/currency/description. |

Output fields: `amount`, `client_secret`, `currency`, `id`, `status`.

### Telegram

`telegram` version 1.0.0

Send a text message to a Telegram chat or channel via a bot token.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `chat_id` | string | Yes | Telegram chat ID or username (e.g. 123456789 or @channelname) |
| `text` | string | Yes | Message text |
| `api_key` | string | No | Telegram Bot Token (from @BotFather) |
| `parse_mode` | string | No | Optional text formatting mode One of: ``, `Markdown`, `HTML`. |

Output fields: `chat`, `date`, `message_id`, `text`.

### Webhook

`webhook` version 1.0.0

Start a workflow when an HTTP request arrives. Outputs the request body, headers, method, and path, plus files when the body has an attachments array.

Ports. In: none; Out: `output` (Triggered), `on_error` (Error).

| Field | Type | Required | Notes |
|---|---|---|---|
| `dedup_window_secs` | number | No | When set above 0, a request whose body was already seen within this many seconds does not re-run the workflow (the caller still gets a 200 OK — this only suppresses the re-run). Providers that retry on timeout or a non-2xx response (Stripe, GitHub, ...) resend the same event body byte-for-byte, so this catches that case. Only applies to requests with a non-empty body — GET requests and empty-body POSTs are never deduped. Default: 0 (disabled). Only takes effect for a workflow set to run in the background (Active in the scheduler) — an ad-hoc 'Run' press always executes once per press. |
| `method` | string | No | One of: `GET`, `POST`, `PUT`, `ANY`. |
| `path` | string | No | URL path (default /webhook) |
| `port` | number | No | Port to listen on (default 3456). In server/API mode, external traffic cannot reach 127.0.0.1 directly — a reverse proxy forwarding to this port is required. |
| `secret` | string | No | Optional shared secret validated via x-webhook-secret header. This verifies the caller knows the secret but does NOT sign the request body — captured requests can be replayed verbatim. For integrations that send HMAC-SHA256 body signatures (Stripe, GitHub, etc.), verify the platform's native signature header (e.g. x-hub-signature-256) in a Code node immediately downstream rather than relying on this field alone. |
| `timeout_secs` | number | No | Wait timeout (default 60) |
| `validate_timestamp` | boolean | No | When true, requires callers to include an 'x-webhook-timestamp' header containing a Unix timestamp (seconds). Requests older than 5 minutes are rejected, which reduces but does not eliminate the replay window — the timestamp is not cryptographically bound to the request body, so an attacker can replay with a captured secret and a fresh timestamp. For body integrity, verify a platform HMAC header (e.g. Stripe-Signature, X-Hub-Signature-256) in a downstream Code node. Default: false. Set to true to require timestamp headers (not compatible with GitHub, Stripe, or other platforms that do not send x-webhook-timestamp). |

Output fields: `body`, `files`, `headers`, `method`, `path`.

## AI nodes

### AI Agent

`ai_agent` version 1.0.0

Give an AI model a goal and tool definitions. It makes one model call and returns the answer, or the tools the model asks for; it does not run them.

Ports. In: `input` (In), `attachments` (Files); Out: `output` (Done), `on_error` (Error).

| Field | Type | Required | Notes |
|---|---|---|---|
| `goal` | string | Yes | What the agent should accomplish. Be specific and include constraints. |
| `provider` | string | Yes | AI provider. Tool definitions are sent and tool requests returned for every provider. One of: `openai`, `anthropic`, `gemini`, `local`, `auto`. |
| `api_key` | string | No | API key — resolved from Connections |
| `base_url` | string | No | API base URL (leave blank for default) |
| `context` | string | No | Additional context to give the agent (from previous nodes, variables, etc.) |
| `effort` | string | No | How hard the model thinks. default: the provider's own default. Anthropic takes low, medium, high, xhigh or max; Gemini takes minimal, low, medium or high; OpenAI-compatible and local servers take none, minimal, low, medium, high, xhigh or max. A level the chosen model does not accept comes back as the provider's API_ERROR. Higher effort uses more of Max Tokens and costs more. One of: `default`, `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`. |
| `max_iterations` | number | No | Not used: the agent makes one call. Kept so saved workflows still load. |
| `max_tokens` | number | No | Maximum tokens per agent response (default: 2048). Higher values allow longer output but increase cost. |
| `messages` | string | No | Earlier conversation to continue, as JSON. Either a list of {role, content} turns where role is user or assistant (AI Memory's messages output fits), or the messages output of an earlier AI Agent run, passed in unchanged. Roles system and developer are refused: use System. Keep Provider, Model, System and Tools the same from turn to turn. At most 4 MiB. |
| `model` | string | No | Model name, for example gpt-5.6, claude-sonnet-5-5 or gemini-3.8-flash. |
| `response_schema` | string | No | JSON Schema the reply must follow, written as a JSON object whose top level has type object. The reply comes back parsed in structured and is checked against the schema in structured_valid. Sent to every provider. OpenAI and Anthropic need additionalProperties false on every object, and OpenAI also needs every property listed in required. A long reply needs a higher max_tokens. |
| `return_messages` | boolean | No | Return the conversation so far in the messages output so a later AI Agent node can continue it. Off by default: the transcript can hold the model's reasoning and any files sent, and it grows with every turn. |
| `single_tool_call` | boolean | No | Allow at most one tool request per reply. Off by default, so the model may ask for several. Not available with Gemini, which has no such setting. |
| `strict_tools` | boolean | No | Make the model's tool arguments follow each tool's parameters schema exactly. Off by default. Every object in a schema needs additionalProperties false; for OpenAI, every property must also be in required. Not available with Gemini. Anthropic allows 20 strict tools. |
| `system` | string | No | System instructions for the agent's persona and behavior. |
| `temperature` | number | No | Creativity (0.0-1.0). Agents work best at 0.2-0.5. |
| `timeout_secs` | number | No | How long to wait for the model's reply, in seconds. Default 3600 (1 hour), range 10-86400. A request that runs past it fails with TIMEOUT and is not retried. Raise it for slow local models. |
| `tool_choice` | string | No | Whether the model may, must or must not ask for a tool. auto: the model decides (default). required: the model must ask for at least one tool (not available with Anthropic, whose current models accept only auto and none). none: the model answers in text only. Needs Tools. One of: `auto`, `required`, `none`. |
| `tool_results` | string | No | Results for the tools the previous run asked for, as a JSON array of {id, tool, content, is_error}. Needs Messages from an earlier run that had Return Messages on. Every tool request must be answered exactly once, matched by id (by tool name for a provider that sends no ids). content may be text or any JSON; a result over 64 KiB is cut and marked. Goal and Context are not sent again. |
| `tools` | string | No | JSON array of tool definitions. Each tool: {name, description, parameters}. Sent to every provider; the node returns the model's tool requests and does not run them. Anthropic accepts names of letters, digits, underscores and hyphens, up to 64 characters. |
| `web_search` | boolean | No | Let the model search the web before it answers, using the provider's own search. Off by default. OpenAI needs a search model such as gpt-5-search-api (used when Model is blank); Anthropic and Gemini work with any model that supports search. Not available with local servers. Gemini cannot combine it with Tools, and Anthropic cannot combine it with Response Schema. The provider sees your prompt, charges per search, and the pages it finds are untrusted content. |
| `web_search_domains` | string | No | Search only these sites (Anthropic only). Separate with commas or new lines and write example.com, not https://example.com; subdomains are included. Blank allows any site. Needs Web Search. |
| `web_search_max_uses` | number | No | Most searches the model may run in one reply (Anthropic only, default 5). Needs Web Search. |

Output fields: `awaiting_tools`, `finished`, `input_tokens`, `iterations`, `messages`, `model`, `output_tokens`, `provider`, `reasoning`, `result`, `search_queries`, `search_suggestions`, `sources`, `structured`, `structured_error`, `structured_valid`, `tool_calls`, `tool_calls_valid`, `truncated`.

### AI Memory

`ai_memory` version 1.0.0

Read from or write to persistent AI memory scoped per session, so the AI can remember past conversations — and, optionally, the files shared in them.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `operation` | string | Yes | read: get history as messages array. write: replace history. append: add a message. clear: delete all messages and files. forget_files: delete the session's remembered files but keep its messages. One of: `read`, `write`, `append`, `clear`, `forget_files`. |
| `session_id` | string | Yes | Unique ID for this conversation thread, e.g. 'user_123' or 'support_chat' |
| `content` | string | No | Message content (required for write/append) |
| `files` | string | No | Files to remember with this message (write/append only). An expression that resolves to a files array, e.g. &#123;&#123;Webhook.output.files&#125;&#125;. Leave blank to store no files. Stored files stay on this device until the session is cleared, they are pruned, or you use forget_files. |
| `include_files` | boolean | No | Return the session's remembered files in the output's 'files' array (read/append/write). Wire it to an AI node's Files port to show the model those files again. Off by default. The AI provider receives them again on every run that uses them. |
| `max_files` | number | No | Most files returned when include_files is on, newest first (default: 5, maximum: 20). Only files attached to messages inside the max_messages window come back, up to 12 MiB in total. |
| `max_messages` | number | No | Maximum messages to return on read (default: 20, newest first) |
| `max_stored` | number | No | Maximum messages to retain in storage per session. Oldest rows are pruned after append if exceeded (default: 1000, minimum: 1). |
| `max_stored_files` | number | No | Maximum files to retain per session. Oldest files are pruned after append if exceeded (default: 20, minimum: 1). A session never keeps more than 32 MiB of files, and each file is limited to 3 MiB. |
| `role` | string | No | Message role (required for write/append) One of: `user`, `assistant`, `system`. |

Output fields: `count`, `files`, `files_omitted`, `files_removed`, `messages`, `session_id`.

### AI Prompt

`ai_prompt` version 1.0.0

Send a prompt to an AI model — Claude, GPT, Gemini, or a local Ollama model — and receive a reply.

Ports. In: `input` (In), `attachments` (Files); Out: `output` (Success), `on_error` (Error).

| Field | Type | Required | Notes |
|---|---|---|---|
| `prompt` | string | Yes | User prompt |
| `api_key` | string | No | API key — resolved from Connections |
| `base_url` | string | No | API base URL. Leave blank for OpenAI. Loopback and private-network addresses are allowed here, for local models such as Ollama. |
| `max_tokens` | number | No | Maximum response tokens. Default 2048. Higher values allow longer output but increase cost and latency. The ceiling that actually applies is set by the provider/model you select above, not by this node. |
| `model` | string | No | Model name — e.g. gpt-5.6, claude-sonnet-5-5, gemini-3.8-flash, llama3 |
| `provider` | string | No | API provider. 'auto' detects from base_url. One of: `auto`, `openai`, `anthropic`, `gemini`, `local`. |
| `rate_limit_rpm` | number | No | Max requests per minute. 0 = unlimited |
| `system` | string | No | System / persona instructions |
| `temperature` | number | No | Creativity: 0.0 (precise) to 2.0 (creative). Default 0.7 |
| `timeout_secs` | number | No | How long to wait for the model's reply, in seconds. Default 3600 (1 hour), range 10-86400. A request that runs past it fails with TIMEOUT and is not retried. Raise it for slow local models. |

Output fields: `content`, `input_tokens`, `model`, `output_tokens`, `provider`.

### Image Generation

`image_gen` version 1.0.0

Generate images from a text prompt. Cloud: GPT Image 1/2 (OpenAI), Nano Banana/Gemini (Google), Flux Pro/2 Pro (BFL). Local: Automatic1111, ComfyUI.

Ports. In: `input` (In), `reference_images` (Reference Images); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `provider` | string | Yes | Provider. gpt_image_1 recommended for cloud. a1111/comfyui for local inference. imagen4/nano_banana -&gt; NanoBanana; dalle3 -&gt; gpt-image-1 (legacy aliases). One of: `gpt_image_1`, `gpt_image_2`, `flux_pro`, `flux_2_pro`, `imagen4`, `nano_banana`, `dalle3`, `a1111`, `comfyui`. |
| `api_key` | string | No | API key from Connections. OpenAI key for GPT Image; Google AI key for NanoBanana; BFL key for Flux. Not used for a1111 or comfyui. |
| `aspect_ratio` | string | No | NanoBanana / Gemini only. Supported: 1:1 1:4 1:8 2:3 3:2 3:4 4:1 4:3 4:5 5:4 8:1 9:16 16:9 21:9. Default: 1:1 |
| `base_url` | string | No | Local provider base URL (a1111/comfyui). e.g. http://127.0.0.1:7860 for a1111, http://127.0.0.1:8188 for comfyui. May embed credentials: http://user:pass@host:port |
| `cfg_scale` | number | No | a1111 only. Classifier-free guidance scale (default 7.0). |
| `height` | number | No | flux_pro: 256-1440 rounded to nearest 32. flux_2_pro: min 64. a1111: 64-2048 rounded to nearest 8. Not used by other providers. Default: 768 (flux_pro) / 1024 (flux_2_pro) / 512 (a1111) |
| `model` | string | No | NanoBanana / Gemini only. Default gemini-2.5-flash-image, which Google shuts down on March 15 2027. gemini-nano-banana-2.1 and gemini-3.1-flash-lite-image (1K images only) are Google's replacements; Google's docs do not confirm they work through this node's request, so a refusal comes back as an API error with Google's message. One of: `gemini-2.5-flash-image`, `gemini-nano-banana-2.1`, `gemini-3.1-flash-lite-image`. |
| `n` | number | No | Images to generate (1-4, default 1). GPT Image, NanoBanana and Flux send one request per image; a1111 sends one batch. Not used by comfyui: the workflow decides. |
| `negative_prompt` | string | No | a1111 only. Things to exclude from the image. |
| `password` | string | No | a1111 only. Basic auth password. Overrides any password in base_url. |
| `prompt` | string | No | Image generation prompt. Required for every provider except comfyui, where the text lives in the workflow JSON. |
| `quality` | string | No | GPT Image only. Default: auto One of: `auto`, `low`, `medium`, `high`. |
| `size` | string | No | GPT Image only. gpt_image_1: 1024x1024\|1536x1024\|1024x1536\|auto. gpt_image_2: WxH divisible by 16, or same presets. Default: 1024x1024 |
| `steps` | number | No | a1111 only. Sampling steps (1-150, default 20). |
| `timeout_seconds` | number | No | a1111 only. HTTP request timeout in seconds. Raise for large batch_size or high step counts on slower hardware. Default 300, max 1800 (30 min). |
| `username` | string | No | a1111 only. Basic auth username. Overrides any username in base_url. |
| `width` | number | No | flux_pro: 256-1440 rounded to nearest 32. flux_2_pro: min 64. a1111: 64-2048 rounded to nearest 8. Not used by other providers. Default: 1024 (a1111: 512) |
| `workflow` | object | No | comfyui only. Workflow in ComfyUI API format (export via Settings → Enable Dev Mode → Save API Format). |

Output fields: `count`, `files`, `source`.

### Text Splitter

`text_splitter` version 1.0.0

Split a long text into smaller chunks by characters, words, sentences, or paragraphs.

Ports. In: `input` (In); Out: `output` (Chunks).

| Field | Type | Required | Notes |
|---|---|---|---|
| `text` | string | Yes | The text to split into chunks |
| `chunk_size` | number | No | Maximum size of each chunk (in the unit specified by mode), 1 or more. Default: 1000 chars / 200 words / 5 sentences / 3 paragraphs |
| `extra_abbreviations` | string | No | Sentences mode only. Extra abbreviations whose dot does not end a sentence, separated by commas or new lines, e.g. 'Kap., Anm., o.ä.'. Not case-sensitive; the trailing dot is optional. Up to 200 entries of up to 20 letters or dots each. |
| `mode` | string | No | How to split: chars (by character count), words, sentences, or paragraphs. Default: chars One of: `chars`, `words`, `sentences`, `paragraphs`. |
| `overlap` | number | No | How much each chunk overlaps with the previous one (same unit as chunk_size). Overlap helps AI maintain context. Default: 100 chars / 20 words / 1 sentence / 0 paragraphs |
| `source_field` | string | No | Optional: dot-path to a string field in the connected node's output, e.g. 'body' or 'content'. Fails if the field is not found. Overrides text. |

Output fields: `chunk_size`, `chunks`, `mode`, `overlap`, `total_chars`, `total_chunks`.

## Logic nodes

### Collect Files

`collect_files` version 1.0.0

Gather files from multiple upstream sources into a single list for downstream processing.

Ports. In: `input` (Source); Out: `output` (Out). The ports depend on the config, so these are the defaults.

| Field | Type | Required | Notes |
|---|---|---|---|
| `sources` | array | No | Source slots — each has id, name, source_expr (auto-populated by canvas wire) |

Output fields: `count`, `files`, `source`.

### If / Condition

`if_condition` version 1.0.0

Branch the workflow on a condition. True paths go to one output, false to another. Set Left Value, Operator and Right Value, or write one Condition string. Text comparisons (==, !=, contains) ignore case; numbers compare numerically.

Ports. In: `input` (In); Out: `on_true` (True), `on_false` (False).

| Field | Type | Required | Notes |
|---|---|---|---|
| `condition` | string | No | Condition as one string, used when Left Value and Right Value are both blank. Examples: '&#123;&#123;temperature_2m&#125;&#125; &gt; 20', '&#123;&#123;status&#125;&#125; == ok', '&#123;&#123;count&#125;&#125; &gt;= 5', '&#123;&#123;tags&#125;&#125; contains urgent'. ==, != and contains ignore case. |
| `lhs` | string | No | Left Value. With Operator and Right Value, replaces Condition: each side is kept whole, so a value that contains '&gt;' or ' contains ' is not misread. |
| `op` | string | No | Operator for Left Value and Right Value. ==, != and contains ignore case; the others compare numbers. One of: `==`, `!=`, `>`, `>=`, `<`, `<=`, `contains`. |
| `rhs` | string | No | Right Value. |

### Loop (For Each)

`loop` version 2.0.0

Run a downstream branch once for each item in a list, then continue with the collected results.

Ports. In: `input` (In); Out: `loop_body` (Each Item), `done` (Done).

| Field | Type | Required | Notes |
|---|---|---|---|
| `array_field` | string | Yes | Dot-path to the array to iterate, e.g. 'items' or 'response.results' |
| `source_node` | string | Yes | Node ID to read the array from. |
| `index_var` | string | No | Variable name for the current index (default: 'index') |
| `item_var` | string | No | Variable name for the current item (default: 'item') |
| `max_iterations` | number | No | Stop after this many items instead of the full array (default: no limit — every item runs). 0 or blank also means no limit. Can only lower the bound: a value larger than the array, or larger than the 10,000-item cap, has no effect beyond the array's own length. |
| `stop_when` | string | No | Optional. Checked before every item, including the first. If it resolves to true the loop stops and continues from Done. Empty, 'false' (any case), '0' and 'null' count as false; anything else is true. A reference to a node that has not run yet resolves to empty, so the first item always runs. Use an expression that reads an earlier item's result, e.g. &#123;&#123;Agent.output.finished&#125;&#125; or &#123;&#123;$vars.stop&#125;&#125;. Can only shorten the loop; Max Iterations and the array's length still apply. |

Output fields: `all_results`, `done`, `index`, `item`, `items`, `stop_reason`, `stopped_early`, `total`.

### Merge

`merge` version 1.0.0

Wait for all incoming parallel branches to complete, then continue as a single execution path.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `mode` | string | No | How to combine inputs: object (keyed by node ID) or array (values only) One of: `object`, `array`. |

### Stop

`stop` version 1.0.0

End this branch of the workflow. Downstream nodes on this path will not run; other independent branches are unaffected.

Ports. In: `input` (In); Out: none.

| Field | Type | Required | Notes |
|---|---|---|---|
| `reason` | string | No | Reason to log when stopping |

### Switch

`switch` version 1.0.0

Route the workflow to one of several branches based on a value match, like a switch statement. Matching is exact and case-sensitive; a field path can index arrays (items.0.status).

Ports. In: `input` (In); Out: `case_1` (Case 1), `case_2` (Case 2), `case_3` (Case 3), `case_4` (Case 4), `case_5` (Case 5), `case_6` (Case 6), `case_7` (Case 7), `case_8` (Case 8), `default` (Default).

| Field | Type | Required | Notes |
|---|---|---|---|
| `cases` | string | Yes | JSON array of cases: [{"match": "ok", "port": "case_1"}, ...]. Matches are exact and case-sensitive. |
| `field` | string | Yes | Field path to switch on, e.g. status or response.code or items.0.status |
| `source_node` | string | Yes | Node ID to read field from |
| `default_port` | string | No | Port to route to when no case matches (default: 'default') |

Output fields: `data`, `matched_case`, `value`.

## Utility nodes

### Delay

`delay` version 1.0.0

Pause the workflow for a fixed duration before continuing to the next node.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `duration` | number | No | How long to wait Default: `1`. |
| `unit` | string | No | One of: `ms`, `s`, `m`, `h`. Default: `s`. |

### Get Variable

`get_variable` version 1.0.0

Read a named workflow variable set earlier in the run by a Set Variable node.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `key` | string | Yes | Variable name to retrieve |
| `default` | any | No | Value to return when the variable is not found (in this run, or in the DB if persist is true). |
| `persist` | boolean | No | If true, fall back to DB when variable not found in this run Default: `false`. |

### JSON

`json` version 1.0.0

Parse a JSON string into an object, or serialise an object into a JSON string.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `operation` | string | Yes | One of: `parse`, `stringify`, `extract`, `merge`, `array_get`. |
| `index` | number | No | Array index for array_get Default: `0`. |
| `input_text` | string | No | JSON string to parse |
| `pointer` | string | No | JSON pointer e.g. /user/name or /0/temperature |
| `source_node` | string | No | Node ID whose output stringify, extract, merge and array_get read. Leave blank to read the whole run context (deprecated). |

Output fields: `result`.

### Output

`output` version 1.0.0

Mark the final output of the workflow. Required for the Chat Panel and the embeddable widget.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `field` | string | No | Specific field to extract, e.g. 'content' or 'body.text'. Leave blank to show everything. |
| `label` | string | No | Label shown above the result (default: 'Result') |
| `source_node` | string | No | Node ID to display output from. Leave blank to use incoming data. |

Output fields: `label`, `output_type`, `type`, `value`.

### Set Variable

`set_variable` version 1.0.0

Store a value in a named workflow variable accessible to downstream nodes via Get Variable or expressions.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `key` | string | Yes | Variable name |
| `persist` | boolean | No | If true, persist value to DB so it survives between runs Default: `false`. |
| `value` | any | No | Value to store (any JSON type) |

### Text to File

`text_to_file` version 1.0.0

Converts a text string into a file object. Use between AI Prompt and Save to Folder to write AI output to disk.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `content` | string | Yes | Text content to encode as a file |
| `filename` | string | No | Output filename (default: output.&lt;format&gt;) |
| `format` | string | No | Output format. Falls back to the filename extension, then 'txt', when omitted. pdf/docx produce real binary documents (headings rendered from leading #/##/###; docx also renders **bold**/*italic*); other formats pass the text through unchanged. PDF uses a built-in Latin-1 font: other characters (CJK, emoji) are replaced with '?' and reported in the node log. One of: `txt`, `md`, `html`, `csv`, `json`, `pdf`, `docx`. |
| `mime_type` | string | No | MIME type (default: chosen from the format, e.g. text/csv for csv) |

Output fields: `files`.

### Transform Data

`transform` version 1.0.0

Reshape or extract data from the previous node's output using a template or expression.

Ports. In: `input` (In); Out: `output` (Out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `mappings` | array | Yes |  |
| `source_node` | string | Yes | Node ID to pull output from |

### Wait

`wait` version 1.0.0

Pause the workflow for a fixed duration, or check a field once and route to Done (match) or Timed out (no match).

Ports. In: `input` (In); Out: `output` (Done), `timed_out` (Timed out).

| Field | Type | Required | Notes |
|---|---|---|---|
| `mode` | string | Yes | duration: wait a fixed time. condition: check once whether a value matches; routes to Done if it does, Timed out if not (no waiting). One of: `duration`, `condition`. |
| `duration_secs` | number | No | Seconds to wait (duration mode). Default 5. |
| `expected` | any | No | Value the field must equal to leave through Done (condition mode). "200" matches 200 and "true" matches true; a missing or null field never matches. |
| `field` | string | No | Dot-path into upstream node outputs to check, e.g. 'node_http.status' (condition mode). |
| `poll_interval_secs` | number | No | No longer used. Kept so saved workflows load; the condition is checked once. |
| `timeout_secs` | number | No | No longer used. Kept so saved workflows load; a condition that does not match routes to Timed out immediately. |

Output fields: `mode`, `timed_out`, `waited_ms`.

