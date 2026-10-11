# Nodes Reference

Every built-in **[node](../glossary.md#node)** Aerini ships with: 40 in total, one entry each. Each entry lists what the node does, its config fields (with the exact label you'll see in the app), its ports, and anything worth knowing before you rely on it.

This page assumes you already know what a node and a connection are. If "workflow," "node," or "trigger" aren't familiar yet, read [Concepts](../getting-started/concepts.md) first.

Nodes are grouped here the same way they're grouped in the app's node picker: Triggers, Core Actions, Files & Storage, Integrations, AI, Logic, and Utility.

A few things that apply across every node on this page, so they're not repeated 40 times:

- **Fields marked "resolved from Connections"** don't show a plain text box. They open the saved-credential picker instead, so the actual secret lives in Aerini's encrypted credential store, not typed into the node. See [Credentials](credentials.md).
- **Any text field can take a `{{...}}` expression** to pull in a value from an earlier node, a workflow variable, or the current run. Number and dropdown (enum) fields can't, since there's nowhere in a number box to type `{{`. See [Expressions](expressions.md) for the full syntax.
- **A number or true/false field that holds text it cannot read** (for example Delay Duration set to `abc`) fails the node with `INVALID_CONFIG` and names the field. A blank field uses its default, and a valid number outside the allowed range is clamped to it.
- **Retries** are off unless you turn them on in a node's retry settings. A rate limit (`429`) is retried for every node. A `502`, `503` or `504` (and an overloaded `529`) is retried only where repeating the call creates nothing: AI Prompt, AI Agent, Stripe (the same idempotency key goes with every attempt) and Google Sheets Get Values. A `500` is never retried, and neither is any failure from a node that sends, creates or starts a paid job (Slack, Telegram, Discord, SendGrid, GitHub, Notion, Sheets Append Row, image generation): a gateway often returns those after the service already did the work, so a retry could send a second email or message, or pay for a second generation. Retry by hand when you know the first attempt did nothing. The same rule covers a request that times out: a connection that could not be made is retried for every node, but a timeout is retried only where repeating the call creates nothing (the nodes above that retry a `502`), because the provider may already have acted on the request. HTTP Request retries a timeout for `GET`, `HEAD`, `OPTIONS`, `PUT` and `DELETE`, never for `POST` or `PATCH`.
- **"Ports"** lists what a node connects to on each side. Unless noted otherwise, a node has one input ("In") and one output ("Out"), the same shape you'll see for most boxes on the canvas.

## Triggers

Every workflow needs exactly one of these to start it. A trigger has no input port; it's always the first node in the chain.

A plugin that supplies its own trigger (see [Trigger plugins](../development/plugin-authoring.md#trigger-plugins)) is listed here too, with its **Plugin** badge, and counts as a trigger on the canvas and in the editor's checks. It starts its own events only when the workflow runs in the background; **Run** executes it once as an ordinary step.

### Manual Trigger

Run a workflow yourself, by pressing Run in the app or calling the server API. No config required to use it.

| Field | Label | Notes |
|---|---|---|
| `mock_payload` | Mock Payload | Optional. A JSON string injected as the trigger's output when you run manually, useful for testing a workflow that expects webhook-shaped data without needing a real webhook call. |

**Ports:** no input. Output: Start.

### Webhook

Starts a workflow when an HTTP request hits a local address. Outputs the request body, headers, method, and path. If the JSON body has an `attachments` array (as Chat sends when you attach files), it's also exposed as `files`, so `{{Webhook.output.files}}` can be wired to a Files input such as AI Prompt's; the same array stays in `body.attachments`. `files` is absent when the body has no `attachments` array.

| Field | Label | Notes |
|---|---|---|
| `port` | Port | Port to listen on, 1024-65535. Default 3456. A value that isn't a whole number in that range fails with `INVALID_PORT` instead of falling back to the default. In server/API mode, outside traffic can't reach `127.0.0.1` directly, so you'll need a reverse proxy in front of it. |
| `path` | Path | URL path to listen on. Default `/webhook`. A leading `/` is added if you leave it out. |
| `method` | Method | `GET`, `POST`, `PUT`, or `ANY`. Not case-sensitive; blank means `ANY`. |
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
| `cron_expr` | Cron Expression | Cron mode. Five-part format: `minute hour day month weekday`, e.g. `0 9 * * 1-5` for weekdays at 9 AM. Supports `*`, ranges, lists and steps, month names (`JAN`) and weekday names (`MON`) in their own fields, `0` or `7` for Sunday, and macros such as `@daily`. The month must always match. If both day and weekday are restricted (neither starts with `*`), a day matches when either does, so `0 9 1 * MON` runs on the 1st and on every Monday; if either starts with `*` (including `*/2`), both must match. A value outside its field's range is rejected with an error naming the field. The app offers a few common presets as quick-fill buttons. |
| `run_at` | Run At (ISO timestamp) | Once mode. Picked in your local time in the app, stored and sent to the engine as UTC. |

**Ports:** no input. Output: Triggered.

## Core Actions

### HTTP Request

Make an HTTP request to any URL. Supports GET, HEAD, OPTIONS, POST, PUT, PATCH, DELETE, custom headers, and a body.

| Field | Label | Notes |
|---|---|---|
| `url` | URL | Required. |
| `method` | Method | Required. `GET`, `HEAD`, `OPTIONS`, `POST`, `PUT`, `PATCH`, or `DELETE`. Not case-sensitive. Any other method fails with `INVALID_METHOD`. |
| `headers` | Headers | Request headers, as an object; text, number, and boolean values are sent. Edited as JSON in **Advanced: full config**. |
| `body` | Body | Request body, for POST/PUT/PATCH. Plain text or an expression edits normally; a JSON object/array value is edited in **Advanced: full config** instead. |
| `api_key` | API Key | Resolved from Connections. |

**Ports:** In → Success, Error.

**Output:** `status`, `body`, `headers`. A response header sent more than once (such as `Set-Cookie`) appears once, with its values joined by `, `. A `HEAD` response has no body, so `body` is an empty string and the information is in `headers` (for example `content-length` or `etag`); `OPTIONS` is typically used to read the `allow` header.

### Shell Command

Run a shell command on the machine Aerini is on, and capture its stdout, stderr, and exit code.

| Field | Label | Notes |
|---|---|---|
| `command` | Command | Required. |
| `cwd` | Cwd | Working directory. |
| `env` | Env | Environment variables, as an object. Edited as JSON in **Advanced: full config**. |
| `timeout_secs` | Timeout (seconds) | Default 30; values are kept between 1 and 86400. On timeout, or when the run is stopped, the command and every process it started are killed on macOS and Linux; on Windows only the command's own process is. A timeout is never retried by a retry policy, because the command may already have done part of its work. |

**Ports:** default (In → Out).

**Output:** `stdout`, `stderr`, `exit_code`, `success`, `truncated`. `stdout` and `stderr` each keep the first 10 MB; anything more is discarded and `truncated` is `true`. A command that writes more than 256 MB to one stream is stopped and fails with `OUTPUT_FLOOD`; redirect big output to a file. When the command fails, the error message shows the end of its stderr with URL passwords, `--password`/`--token`/`--passphrase` flags (with `=` or a space), curl `-u user:pass` credentials, and similar secrets masked. Masking is pattern-based: a secret passed in a flag it does not know can still appear in the failure message, in `stderr`, or in the run log.

**Gotcha:** this runs a real command on the host machine. Aerini treats Shell Command, Code (JS), and Database as "dangerous" nodes: any workflow containing one shows a confirmation dialog before its first run in a session ("This workflow contains nodes that execute code on your computer... Only run workflows from sources you trust"), and the node gets a small warning badge on the canvas. Run, Run this node, a node's Test button, and Replay in run history all ask. Your answer holds for the session only while those nodes stay as they were: changing one's settings, saved credential, type, or enabled state, or loading, importing, or restoring a version of the workflow where they differ, asks again. Moving or renaming a node does not.

### Code (JS)

Run a JavaScript snippet with a full Node.js runtime and return its result.

| Field | Label | Notes |
|---|---|---|
| `code` | Code | Required. Call `output(value)` to return a result. Has access to `input` and `context`. |
| `timeout_secs` | Timeout (seconds) | Max execution time. Default 10; values are kept between 1 and 60. On timeout, or when the run is stopped, the Node.js process and any process it started are killed on macOS and Linux; on Windows only the Node.js process is. |

**Ports:** In → Out, Error.

**Output:** `result` (whatever you passed to `output()`), `stdout` (the text the code printed, such as `console.log` output), `duration_ms`, `truncated`, and `_sandbox_partial` (present and `true` on macOS when the code sandbox is active: ESM import restrictions are enforced there, but the CPU/memory limits are Linux-only and don't apply on macOS).

**Gotcha:** counted among the "dangerous" nodes (see Shell Command above); triggers the same confirmation dialog and canvas badge.

Printing is safe before or after `output()`. Let the script finish on its own: if it calls `process.exit()`, `output()` is not delivered and the node fails with `NO_RESULT`. A failed run's error message shows only the last few KB of stderr, with URL passwords and `--password`-style secrets masked. Code that prints more than 256 MB to one stream is stopped with `OUTPUT_FLOOD`. Printed text and the result share a 10 MB limit: when the printed text is longer, only its last 10 MB is kept and `truncated` is `true`; a result too large to fit fails with `RESULT_TOO_LARGE`.

### Send Email

Send an email over SMTP, plain text or HTML, to one or more recipients.

| Field | Label | Notes |
|---|---|---|
| `smtp_host` | SMTP Host | Required, e.g. `smtp.gmail.com`. |
| `smtp_port` | SMTP Port | 587 (STARTTLS, recommended) or 465 (SSL). Default 587. A value outside 1-65535 fails with `INVALID_PORT`. |
| `from` | From | Required. Sender address. |
| `to` | To | Required. Comma-separated, or an array of addresses from an upstream node; 50 recipients max. A comma inside quotes or `<angle brackets>` does not split, so `"Doe, John" <a@b.c>` is one recipient. Anything that is neither a string nor an array of strings fails with `INVALID_TO`. |
| `subject` | Subject | Required. A missing or null value fails with `MISSING_SUBJECT`, and a value that is not text with `INVALID_SUBJECT`. An empty string is sent as an empty subject. |
| `body` | Body | Required. A missing or null value fails with `MISSING_BODY`, and a value that is not text (a number, an object) with `INVALID_BODY`; convert it to text first. An empty string is sent as an empty body. |
| `html` | HTML | Send as HTML instead of plain text. Default off. |
| `username` | Username | SMTP username, usually your email address. Set together with Password, or leave both empty for an unauthenticated relay. |
| `password` | Password | Resolved from Connections. |

**Ports:** default (In → Out).

**Output:** `sent`.

**Gotcha:** Send Email is retried (when a retry policy is on) only when the server cannot have received the message: a connection that could not be made or dropped, a timeout, a login that failed temporarily, or a temporary `4xx` reply to the sender, a recipient or `DATA` (a greylisting `451`, for example). The send is tracked step by step, so it knows whether the message body was sent. A permanent refusal (`550`, `535`) fails with `SMTP_ERROR` and is not retried. After the body is sent, a temporary `4xx` reply also fails without a retry, and a connection that drops or times out then fails with `SMTP_DELIVERY_UNCERTAIN`: the email may or may not have been delivered, so check before sending it again. Each step before the body must be answered within 30 seconds (a silent server is a recoverable `SMTP_ERROR`); the body and the server's final reply get 120 seconds, after which the send is stopped with `SMTP_DELIVERY_UNCERTAIN`. If the host has several addresses, the next is tried when one cannot be connected.

### Desktop Notification

Show a desktop notification on the machine running the workflow, while it runs.

| Field | Label | Notes |
|---|---|---|
| `title` | Title | Required text; a title that is not text fails with `INVALID_TITLE`. On Linux `<`, `>` and `&` are escaped, as in the body. |
| `body` | Body | Optional notification text; a body that is not text fails with `INVALID_BODY`. On Linux `<`, `>` and `&` are escaped so they show as typed; a notification daemon that does not read markup may show the escapes (`&lt;`) instead. |
| `urgency` | Urgency | `low`, `normal`, or `critical`. Applied on Linux only, but any other value fails with `INVALID_URGENCY` on every platform. Default `normal`. |

**Ports:** default (In → Out).

**Output:** `sent`.

**Gotcha:** the notifier (`notify-send`, `osascript`, or PowerShell on Windows) is stopped if it has not finished after 15 seconds, and the failure (`NOTIFY_ERROR`) includes whatever it printed to stderr, such as a missing notification daemon. A notification that fails is never retried.

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
| `allow_raw_sql` | Allow Raw SQL | SQL only. Default off: a query with single-quoted string literals (double-quoted on MySQL) is refused, so values go through `?` and Params. Only an admin caller on a server can turn it on; the desktop app ignores it. |
| `allow_local` | Allow Local | Postgres/MySQL/Redis only. Default off: connections to localhost, `127.0.0.1`, and private network ranges are blocked. Only turn this on for a database you're intentionally running yourself on this machine or LAN. |

**Ports:** In → Out, Error.

**Output (SQL):** `rows`, `rows_affected`, `last_insert_id` (SQLite and MySQL populate this; Postgres returns 0, use `RETURNING` instead), `columns`. A Query returns at most 100,000 rows and fails past that (add a `LIMIT`). Postgres and MySQL decode integers, floats, booleans and text; any other column type (NUMERIC/DECIMAL, dates and times, UUID, JSON, binary, arrays) comes back as a placeholder string naming the type, with a log line, so cast it to text in the query (`price::text`, `CAST(price AS CHAR)`).

**Output (Redis):** `value`, `ok`, `deleted`, `list_length`.

**Gotcha:** the `query` field flatly rejects `{{...}}` expressions and fails the node instead of running. Values are inserted directly into the SQL string, so a numeric injection (`1 OR 1=1`) produces no quotes and can't be sanitized by pattern-matching. Use `?` placeholders in `query` and put the dynamic values in `params` instead, e.g. `query: "SELECT * FROM t WHERE id = ?"` with `params: ["{{HTTP Request.output.id}}"]`.

**Gotcha:** the Query operation refuses anything it cannot prove is read-only, and also `INTO` (`SELECT ... INTO` creates a table or writes a file), a second statement after `;`, `$tag$` dollar quotes, `#` comments, `--` without a following space, MySQL `/*! */` comments, nested `/* */` comments, and a backslash inside a quoted literal, because the databases read these differently. It is a guard against mistakes, not a sandbox: a `SELECT` can still call functions with side effects, so connect with a read-only database role when the query text comes from untrusted data. On SQLite, Execute refuses `ATTACH`, `DETACH` and `VACUUM INTO`, which would reach files outside Database Path. Redis `set`, `hset`, `lpush` and `rpush` need a Value (an empty string stores an empty value); push and pop operations are not retried after a dropped connection, since the first attempt may already have run.

**Gotcha:** counted among the "dangerous" nodes (see Shell Command above); triggers the same confirmation dialog and canvas badge.

## Files & Storage

### File

Read, write, or delete a file at a path on the host machine.

| Field | Label | Notes |
|---|---|---|
| `operation` | Operation | Required. `read`, `write`, `append`, `delete`, or `exists`. |
| `path` | Path | Required. Absolute or relative. |
| `content` | Content | Required for `write`/`append`. Use an empty string for an empty file; a missing value is an error, so a forgotten field cannot empty a file. Numbers and true/false are written as text. |
| `encoding` | Encoding | `utf8` or `base64`. Default `utf8`. With `utf8`, reading a file that is not valid UTF-8 fails with `NOT_UTF8` instead of returning damaged text; use `base64` for binary files. |

**Ports:** default (In → Out).

**Output:** `content`, `exists`, `bytes`, `path`.

`read` takes regular files up to 50 MB (`FILE_TOO_LARGE` above that, `NOT_A_FILE` for a directory or device). On a server with `--file-sandbox-dir`, every path must stay inside that directory: a symlink is followed only when its target is inside too, a link that points nowhere is refused, and `delete` on a symlink removes the link, not its target.

### Save to Folder

Save one or more files to a folder on disk, optionally split across subfolders.

| Field | Label | Notes |
|---|---|---|
| `folder_path` | Destination Folder | Set via a folder-picker button in the config panel, not a plain text field. |
| `overwrite` | Overwrite existing files | Checkbox. When off, a file with the same name is kept and the incoming one is skipped. Default on. A value that is set but is not `true` or `false` fails with `INVALID_CONFIG`. |
| `filename_prefix` | Filename Prefix | Optional, prepended to every saved filename. Capped at 50 characters; a longer literal value is truncated in the field itself, and a longer `{{...}}`-resolved value is truncated at save time since the cap can't be enforced until the expression resolves. |
| Subfolders | Subfolders | Not a config field so much as a port builder: each subfolder you add in the config panel becomes its own input port, labeled with that subfolder's name. Connect a node that outputs a files array (Image Generation, Collect Files, S3 Storage) to route its files into that specific subfolder. |

**Ports:** dynamic. One input port per configured subfolder (each accepting files), or a single generic "In" port if no subfolders are configured. Output: Out.

**Output:** `saved`, `count`, `folder`, `skipped` (files not written because `overwrite` was off and a same-named file existed), `errors`.

Files whose names collide within one run are kept apart as `name.ext`, `name_2.ext`, `name_3.ext` (names compare without regard to case), so a later file never silently replaces an earlier one; a file with no name is saved as `file.bin`. `data` may be a `data:` URI and may contain line breaks. If no file is written and none is skipped, the node fails with `SAVE_FAILED`; a run where some files fail still succeeds and lists them in `errors`. With `--file-sandbox-dir`, the folder and every subfolder must resolve inside the sandbox.

### S3 Storage

Upload, download, list, or delete objects in an S3-compatible object store: AWS S3, Cloudflare R2, or MinIO.

| Field | Label | Notes |
|---|---|---|
| `operation` | Operation | `upload`, `download`, `list`, `delete`, or `presign_url`. |
| `provider` | Provider | `aws`, `r2`, or `minio`, in any letter case. Default `aws` when blank. Any other value fails with `UNKNOWN_PROVIDER`, which lists the valid ones. |
| `access_key_id` | Access Key ID | |
| `secret_access_key` | Secret Access Key | |
| `region` | Region | AWS region, e.g. `us-east-1` (default). For R2, use `auto`. Ignored for MinIO, use Endpoint instead. |
| `endpoint` | Endpoint | Custom endpoint URL. Required for R2 (`https://ACCOUNT_ID.r2.cloudflarestorage.com`) and MinIO (`http://host:9000`). Must be empty for AWS: a value there fails with `ENDPOINT_NOT_SUPPORTED`. |
| `bucket` | Bucket | Required for most operations. |
| `key` | Key | Object key/path. Required for `upload`, `download`, `delete`, `presign_url`. |
| `content` | Content | Content to upload, as text or base64 depending on Content Encoding. |
| `content_encoding` | Content Encoding | `text` or `base64`, in any letter case. Default `text` when blank. Base64 content may be a `data:` URI and may contain line breaks. Any other value fails with `UNKNOWN_CONTENT_ENCODING`. |
| `content_type` | Content Type | MIME type for upload. Default `application/octet-stream`. With `presign_url` and `PUT` it is signed into the URL, so the uploader must send the same `Content-Type`; `presign_url` with `GET` and a Content Type fails with `PRESIGN_PUT_ONLY`. |
| `prefix` | Prefix | Key prefix filter for `list`. Empty string lists everything. |
| `delimiter` | Delimiter | `list` only. Usually `/`. Keys that share the text between the Prefix and the next delimiter are returned once, in `common_prefixes`, like folders; only the keys directly under the Prefix are in `objects`. Empty lists every key. |
| `max_objects` | Max Objects | `list` only. Most entries to return, a whole number from 1 to 10000. Default 10000. Anything else fails with `INVALID_CONFIG`. |
| `content_length` | Content Length | `presign_url` with `PUT` only. The exact upload size in bytes, up to 5 GiB; it is signed into the URL, so an upload of another size is refused by S3. With `GET` it fails with `PRESIGN_PUT_ONLY`. |
| `method` | Method | `GET` or `PUT` (any letter case), for `presign_url`. `GET` (default when blank) gives a download link, `PUT` an upload link. Any other value fails with `UNKNOWN_METHOD`. |
| `expiry_secs` | Expiry Secs | Presigned URL lifetime. Default 3600, maximum 604800 (7 days). |

**Ports:** In → Out, Error.

**Output:** `key`, `size`, `content_type` (upload only), `content` (download only, base64-encoded), `content_encoding` (always `base64` for a download), `objects` (list only, array of `{key, size, last_modified}`), `prefix`, `count` (list only), `truncated` and `max_objects` (list only), `deleted`, `common_prefixes` (list only, array of strings, empty without a Delimiter), `url` (presign_url only), `method` (presign_url only), `headers` (presign_url only), `expires_in`.

`list` follows S3's continuation tokens and returns at most Max Objects entries (10,000 by default); `max_objects` in the output is the limit that applied, and `count` is the number of objects returned. Objects and common prefixes count together toward the limit. When more match, `truncated` is `true`; narrow the Prefix or raise Max Objects to see the rest.

`headers` on a presigned URL is an object of header names and values the request must send exactly as given (for a `PUT` with Content Type or Content Length set, these headers); it is empty otherwise. Send them with the request or S3 rejects the signature. A failure shows S3's own error code and message, for example `NoSuchBucket: The specified bucket does not exist`; with no reply from the server it shows the connection error instead, and a download that breaks off partway shows what interrupted it. Error text is cut at 500 characters.

### Social Upload

Upload video or image content to YouTube, Instagram, or TikTok.

| Field | Label | Notes |
|---|---|---|
| `files` | Files | Required. Media files array, typically wired from an upstream node. Each entry needs `data`. For Instagram, `data` is a public `http(s)` URL instead of base64 (see below). |
| `platform` | Platform | Required. `youtube`, `instagram`, or `tiktok`. |
| `title` | Title | Required. YouTube: up to 100 characters, no `<` or `>`. TikTok and Instagram: the start of the caption. |
| `description` | Description | Optional. YouTube: up to 5,000 bytes, no `<` or `>`. TikTok and Instagram: added after the title, with a blank line between. |
| `tags` | Tags | Comma-separated. YouTube only; empty entries are dropped. |
| `privacy` | Privacy | Default `private`. YouTube: `public`, `private`, or `unlisted`. TikTok: `public_to_everyone`, `mutual_follow_friends`, `follower_of_creator`, or `self_only`. Any other value fails the node (it is never silently made private). Instagram ignores it. |
| `client_id` | Client ID | OAuth client ID. Typed directly into the node. |
| `client_secret` | Client Secret | OAuth client secret. Typed directly into the node. |

**Ports:** default (In → Out).

**Checked before sign-in:** platform, title, privacy, client ID and secret, caption and title lengths, and every file entry (a missing `data`, a non-URL for Instagram, a file type TikTok cannot take). A problem there fails the node at once, without opening a browser.

**Instagram:** Instagram fetches the media itself, so each file's `data` must be a publicly reachable `http(s)` URL. Aerini creates a media container (an image, or a Reel for a video), waits for Instagram to process it (up to 2 minutes for an image, 10 for a video), then publishes it. The type comes from `mime_type`, or from the URL's extension when the type is generic. Only professional (Business or Creator) accounts can publish.

**TikTok:** accepts `video/mp4`, `video/quicktime` and `video/webm` (or a `.mp4`, `.mov` or `.webm` name when `mime_type` is generic), up to 4 GB. After the upload Aerini checks TikTok's status for up to 90 seconds so a processing failure is reported; a post still processing then returns `status: "processing"`. An app that has not passed TikTok's audit can only post `self_only`; TikTok's refusal is reported as `TIKTOK_APP_UNAUDITED`.

**YouTube:** uploads with the resumable protocol in one request, so large files are supported; the time allowed grows with the file size (at least 128 KiB/s is assumed, 2 minutes to 12 hours).

**Errors:** a provider's own reason is included in every error message (for example `invalid_param: ...`). A `401` discards the stored sign-in, so the next run signs in again. If no file was uploaded the node fails (code shared by all files, or `UPLOAD_FAILED`; only an all-`RATE_LIMITED` failure is retryable). If some files uploaded and others failed, the node succeeds and lists the failures in `errors`, because the uploaded posts cannot be undone and a retry would duplicate them.

**Output:** `uploaded` (each with `filename`, `platform_id`, `url` (`null` for TikTok, or when Instagram gives no link) and `status`), `count`, `failed`, `platform`, `errors`.

## Integrations

A `429 Too Many Requests` from a provider fails the node with the retryable code `RATE_LIMITED` instead of the node's own error code, so a retry policy can try again. For the nodes listed under **Retries** above, a `502`, `503` or `504` fails with the retryable code `UPSTREAM_UNAVAILABLE` the same way. Provider response bodies are read up to 10 MB, and a Discord or SendGrid error shows at most the first 1 KiB of the provider's reply.

### Slack

Post a message to a Slack channel via `chat.postMessage`, using a bot token.

| Field | Label | Notes |
|---|---|---|
| `channel` | Channel | Required. Channel ID or name, e.g. `C1234567890` or `#general`. |
| `text` | Text | Required. |
| `api_key` | API Key | Slack Bot Token (`xoxb-...`). Resolved from Connections. |

**Ports:** default (In → Out).

**Output:** `ok`, `ts` (message timestamp), `channel`.

**Gotcha:** Text over 40,000 characters fails with `TEXT_TOO_LONG`, because Slack would cut it short. A `missing_scope` failure names the scopes Slack reports, for example `missing_scope (needed: chat:write; provided: channels:read)`.

### Discord

Send a text message to a Discord channel via a webhook URL.

| Field | Label | Notes |
|---|---|---|
| `webhook_url` | Webhook URL | Required. `https://discord.com/api/webhooks/...`. |
| `content` | Content | Required. Maximum 2000 characters. |
| `username` | Username | Optional, overrides the webhook's display name for this message only. |
| `allow_everyone_mentions` | Allow @everyone mentions | Optional, default off. When on, `@everyone` and `@here` in Content ping the channel, if the webhook's channel permissions allow it. |

**Ports:** default (In → Out).

**Output:** `sent`.

**Gotcha:** the Webhook URL must be exactly `https://discord.com/api/webhooks/<id>/<token>` (or `discordapp.com`, optionally with an API version such as `/api/v10/`); surrounding spaces are ignored, and a URL with another host, a port, a login, extra path segments or `..` fails with `INVALID_WEBHOOK_URL`. `@everyone` and `@here` in Content do not ping anyone unless **Allow @everyone mentions** is on; `<@user>` and `<@&role>` mentions still do.

### GitHub

Create an issue or add a comment via the GitHub API.

| Field | Label | Notes |
|---|---|---|
| `action` | Action | Required. `create_issue` or `add_comment`. |
| `owner` | Owner | Required. Repository owner, user or org: up to 39 letters, digits, hyphens or underscores. Anything else fails with `INVALID_OWNER`. |
| `repo` | Repo | Required. Up to 100 letters, digits, `-`, `_` or `.`, and not `.` or `..`. Anything else fails with `INVALID_REPO`. |
| `title` | Title | Required for `create_issue`. |
| `body` | Body | Issue body or comment text. |
| `issue_number` | Issue Number | Required for `add_comment`. A positive integer. |
| `api_key` | API Key | GitHub personal access token. Resolved from Connections. |

**Ports:** default (In → Out).

**Output:** `number`, `html_url`, `id`, `state`.

**Gotcha:** only `create_issue` and `add_comment` are implemented. Pull requests and repository file edits aren't available through this node yet.

### Google Sheets

Read from or write to a spreadsheet via the Sheets API.

| Field | Label | Notes |
|---|---|---|
| `action` | Action | Required. `append_row` or `get_values`. |
| `spreadsheet_id` | Spreadsheet ID | Required. From the sheet's URL (the part after `/spreadsheets/d/`). The full `https://docs.google.com/spreadsheets/d/...` address also works. Anything else fails with `INVALID_SPREADSHEET_ID`. |
| `range` | Range | Required. A1 notation, e.g. `Sheet1!A1:D1`. |
| `value_input` | Value Input | Optional, `append_row` only. `RAW` (default) or `USER_ENTERED`; see below. |
| `values` | Values | Required for `append_row`. Rows of cells as JSON text, e.g. `[["a","b"],["c","d"]]`; an expression that resolves to JSON text works too. A cell is text, a number, `true`/`false` or `null`. |
| `client_id` | Client ID | Google OAuth client ID. Enables automatic token refresh; recommended. Entered directly on this node; not offered as a saved-credential picker field. |
| `client_secret` | Client Secret | Google OAuth client secret. Entered directly on this node; not offered as a saved-credential picker field. |
| `api_key` | API Key | A pasted OAuth 2.0 access token instead of client ID/secret. Does not auto-refresh and expires after about an hour. Only used when Client ID/Secret aren't set. |

**Ports:** default (In → Out).

**Output:** `values` (from `get_values`; an empty list when the range holds no data), `updatedRange`, `updatedRows`, `updatedColumns`, `updatedCells`. If Google returns no update summary for an append, the output is Google's whole reply instead.

**Gotchas:**
- Everything is checked before the Google sign-in opens: an unknown action, a missing or malformed Spreadsheet ID or Range, and Values that are not rows fail straight away. Values that are not valid JSON fail with `INVALID_VALUES_JSON`; a flat list such as `["a","b"]`, an empty list, an empty row or a cell that is a list or object fails with `INVALID_VALUES_SHAPE`; unset Values fails with `MISSING_VALUES`. None of these are retried.
- By default values are written as typed (`valueInputOption=RAW`): text such as `2026-01-05` or `1,5` stays text and is not turned into a date or number, and a formula (`=SUM(A1:A2)`) is stored as text. Set **Value Input** (`value_input`) to `USER_ENTERED` to have Sheets parse values as if typed in the sheet: formulas run and dates and numbers are converted. Do not use `USER_ENTERED` with data you do not control: a value that starts with `=` becomes a formula. `get_values` ignores the setting; any other value fails with `INVALID_CONFIG`.
- `append_row` asks Google to insert new rows (`insertDataOption=INSERT_ROWS`) after the last row of the table found in the range, so existing rows below it are pushed down, never written over.
- A `401` from Google deletes the stored sign-in for the Client ID, so the next run signs in again. A pasted API Key has nothing stored and is not affected.

### Notion

Create or update pages and database entries.

| Field | Label | Notes |
|---|---|---|
| `action` | Action | Required. `create_page` or `update_page`. |
| `database_id` | Database ID | For `create_page`: this or Data Source ID. A 32-character Notion ID, with or without dashes, or a link to the database copied from `notion.so` or `notion.site` (the ID is the last 32 characters of the link's last path part; `?v=...` is ignored). Anything else fails with `INVALID_DATABASE_ID`. |
| `data_source_id` | Data Source ID | `create_page` only, for a database with several data sources: the ID of the data source to add the page to. Use it instead of Database ID; setting both fails with `PARENT_CONFLICT`. A value that is not a 32-character Notion ID fails with `INVALID_DATA_SOURCE_ID`. This call is sent with Notion API version `2025-09-03`, as Notion's upgrade guide directs for this parent; every other call stays on `2022-06-28`. |
| `page_id` | Page ID | Required for `update_page`. A 32-character Notion ID, with or without dashes. Anything else fails with `INVALID_PAGE_ID`. |
| `title` | Title | Page title, written to the title property named in Title Property. |
| `title_property` | Title Property | Name of the database's title property. Default `Name`. Set it when the database calls its title column something else (`Task`, `Title`); with the wrong name Notion refuses the request. |
| `properties` | Properties | Additional Notion page properties, as a JSON object in JSON text, e.g. `{"Status": {"select": {"name": "Done"}}}`; an expression that resolves to JSON text works too. |
| `api_key` | API Key | Notion integration token (`secret_...`). Resolved from Connections. |

**Ports:** default (In → Out).

**Output:** `id`, `url`, `object`, `archived`.

**Gotchas:**
- Properties that are not valid JSON fail with `INVALID_PROPERTIES_JSON`, and JSON that is not an object (a list, a number, `null`) fails with `INVALID_PROPERTIES_SHAPE`. Neither is retried, and a Properties value is never dropped silently. A blank Properties means none.
- Set the title in one place. If Title is set and Properties also holds a title property (or any value under the Title Property name), the node fails with `TITLE_CONFLICT` instead of sending two.
- `update_page` with no Title and no properties to change fails with `NOTHING_TO_UPDATE`.

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
| `to_email` | To Email | Required. One address, or several separated by commas or semicolons (up to 1000, duplicates sent once). Each recipient gets a separate copy and cannot see the others. |
| `from_email` | From Email | Required. Must be a verified sender in SendGrid. |
| `from_name` | From Name | Optional display name shown to recipients, up to 200 characters, no line breaks. Fails with `INVALID_FROM_NAME` otherwise. |
| `subject` | Subject | Required. |
| `body` | Body | Required. Plain text; an empty body fails with `MISSING_BODY`. |
| `api_key` | API Key | SendGrid API key (`SG....`). Resolved from Connections. |

**Ports:** default (In → Out).

**Output:** `sent`, `message_id`.

**Gotcha:** every address is checked before anything is sent. A bad address fails with `INVALID_TO_EMAIL` or `INVALID_FROM_EMAIL` (write bare addresses, not `Name <address>`; put a sender name in **From Name**), and more than 1000 recipients fails with `TOO_MANY_RECIPIENTS`.

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
| `prompt` | Prompt | Required, unless the node has at least one readable attachment (for example a file-only Chat message), in which case a default instruction stands in for the missing text. |
| `system` | System | System/persona instructions. |
| `model` | Model | Model name, e.g. `gpt-5.6`, `claude-sonnet-5`, `gemini-3.6-flash`, `llama3`. Blank uses the provider's default (`local` has none and needs a name). A **Fetch Models** button queries the provider's `/models` endpoint and offers a dropdown; typing a name directly always still works if the fetch fails or the provider doesn't support it. |
| `provider` | Provider | `auto`, `openai`, `anthropic`, `gemini`, or `local`. `auto` detects from Base URL, including local servers such as Ollama. |
| `base_url` | Base URL | API base URL. Leave blank for OpenAI. Loopback and private-network addresses are allowed here specifically, for local models like Ollama. See [Local Models](local-models.md) for how to point this at one. |
| `api_key` | API Key | Resolved from Connections. |
| `temperature` | Temperature | 0.0 (precise) to 2.0 (creative). Default 0.7. Claude accepts 0.0 to 1.0, so higher values are sent as 1.0. Models that take no temperature (OpenAI's GPT-5 and reasoning models, newer Claude models) refuse it, and the node then repeats the call once without it and says so in the log. |
| `max_tokens` | Max Tokens | Default 2048. The real ceiling is whatever the provider/model you picked allows, not this field. Reasoning models count their thinking against it, so a reply can come back empty: the node then fails with `OUTPUT_TRUNCATED`; raise Max Tokens. |
| `rate_limit_rpm` | Rate Limit (RPM) | Max requests per minute. 0 = unlimited. |

**Ports:** In → Success, Error. A second, "Files" input port accepts a files array wired from an upstream node (Image Generation, Collect Files, Text to File, or AI Memory's remembered files), merged with any static attachments configured on the node itself.

**Output:** `content`, `model`, `provider`, `input_tokens`, `output_tokens`. A reply the provider refuses to write fails with `REFUSED`; a Gemini prompt that is blocked, or a reply with no text, fails with `GEMINI_NO_CONTENT`.

### AI Agent

Give an AI model a goal and a set of tool definitions. The node makes one model call and returns its answer, or the tools it asks for. It does not run tools itself: the nodes you wire after it do the work (for example a Switch on `tool_calls.0.tool`), so you stay in control of what actually runs and nothing is made up for the model to continue on.

| Field | Label | Notes |
|---|---|---|
| `goal` | Goal | Required, unless the node has at least one readable attachment (for example a file-only Chat message), in which case a default instruction stands in for the missing text. What the agent should accomplish. Be specific and state constraints. |
| `system` | System | System instructions for the agent's persona and behavior. |
| `tools` | Tools | JSON array of tool definitions, each `{name, description, parameters}` (the OpenAI `{type, function}` form is accepted too). Used with OpenAI-compatible and Gemini providers; Anthropic ignores them and says so in `reasoning`. Text that is not a JSON array of named tools fails the node with `INVALID_TOOLS`. |
| `context` | Context | Extra context to hand the agent, e.g. from earlier nodes or variables. |
| `provider` | Provider | Required. `openai`, `anthropic`, `gemini`, `local`, or `auto`. OpenAI, Gemini, and `local` receive the tool definitions; Anthropic receives none and answers in a single pass. |
| `model` | Model | E.g. `gpt-5.6` (OpenAI), `claude-sonnet-5` (Anthropic), `gemini-3.6-flash` (Gemini). A **Fetch Models** button queries the provider's `/models` endpoint and offers a dropdown; typing a name directly always still works if the fetch fails or the provider doesn't support it. |
| `base_url` | Base URL | API base URL, leave blank for the provider's default. Loopback and private-network addresses are allowed here too, for local models like Ollama; see [Local Models](local-models.md). |
| `api_key` | API Key | Resolved from Connections. |
| `max_iterations` | Max Iterations | Not used: the agent makes one call. Kept so saved workflows still load. |
| `max_tokens` | Max Tokens | Default 2048. |
| `temperature` | Temperature | 0.0 to 1.0. Agents tend to work best around 0.2 to 0.5. |

**Ports:** In → Done, Error. A second, "Files" input port works the same way as AI Prompt's.

**Output:** `result`, `iterations` (always 1), `tool_calls` (each `{id, tool, arguments, parsed_arguments}`; the tools the model asked for), `awaiting_tools` (true when it asked for tools and gave no final answer), `reasoning`, `finished` (false if it asked for tools or the reply was cut off), `truncated` (true if the provider cut the reply off; raise Max Tokens). A reply with no text that was cut off at Max Tokens fails with `OUTPUT_TRUNCATED`.

### AI Memory

Read or write persistent AI conversation history, scoped per session, so an AI node can remember earlier turns and, if you opt in, the files shared in them.

| Field | Label | Notes |
|---|---|---|
| `operation` | Operation | Required. `read` (get history as a messages array), `write` (replace history), `append` (add one message), `clear` (delete all messages and files), or `forget_files` (delete the session's remembered files and keep its messages). |
| `session_id` | Session ID | Required. Unique ID for the conversation thread, e.g. `user_123`. |
| `role` | Role | `user`, `assistant`, or `system` (any case). Required for write/append; any other value fails with `INVALID_ROLE`. |
| `content` | Content | Message content. Required for write/append, at most 4 MiB (`CONTENT_TOO_LARGE`). |
| `files` | Files | `write`/`append` only. An expression that resolves to a files array, e.g. `{{Webhook.output.files}}`; those files are stored with the message. Blank stores none. |
| `include_files` | Include Files | `read`/`append`/`write`. Off by default. Adds the session's remembered files to the output's `files` array. |
| `max_files` | Max Files | Most files returned when Include Files is on, newest first. Default 5, maximum 20. |
| `max_messages` | Max Messages | Max messages returned on read, newest first. Default 20. |
| `max_stored` | Max Stored | Max messages retained per session; oldest are pruned after an append past this. Default 1000, minimum 1. |
| `max_stored_files` | Max Stored Files | Max files retained per session; oldest are pruned after an append past this. Default 20, minimum 1. |

**Ports:** default (In → Out).

**Output:** `messages` (array of `{role, content}`), `count`, `session_id`. With Include Files on, also `files` (array of `{filename, mime_type, data}`, oldest first) and `files_omitted` (remembered files that didn't fit the limits below). `forget_files` returns `session_id` and `files_removed`.

The [Chat Panel](chat-panel.md)'s Clear button and session delete call this node's `clear` operation directly, using the Chat Panel's own session ID — if a workflow uses that same ID for its AI Memory calls, clearing a chat session clears its memory too, files included.

#### Remembering files

An AI node sees a file only in the run that carries it. To let the model keep seeing a file on later messages, store it with the message and recall it each turn:

1. Set **Files** to `{{Webhook.output.files}}` on the AI Memory node that appends the user's message, and turn on **Include Files**.
2. Wire that node's output to the AI node's **Files** port. `append` returns the recent messages and the remembered files, current message included, so no separate `read` is needed. Put `{{AI Memory (append user).output.messages}}` in the prompt, and append the assistant's reply afterwards as usual.

`append` still requires text `content`, so a Chat message that is only a file needs a fallback in that field. Both steps are opt-in. Nothing is stored unless **Files** is set, and nothing comes back unless **Include Files** is on, so existing workflows behave exactly as before.

| Limit | Value | When it's exceeded |
|---|---|---|
| Types | `png`, `jpeg`, `webp`, `gif`, `pdf`, `txt`, `md` (the types AI Prompt reads) | The file is skipped and the run log says why; the message is still stored. |
| One file | 3 MiB | Skipped, same as above. |
| Per session | **Max Stored Files** (default 20) and 32 MiB | The oldest files are deleted. |
| Per recall | **Max Files** (default 5, maximum 20) and 12 MiB | The oldest files are left out and counted in `files_omitted`. |
| Message window | Only files attached to messages inside **Max Messages** come back | A file whose message has scrolled out of the window is not sent. |

Files are never cut short: a file is kept whole or dropped whole, oldest first. A file whose content the session already holds is stored once, moved to the message that attached it most recently. The limits sit under the smallest request ceilings among the providers (Anthropic rejects an image over 5 MB encoded; Gemini's inline request limit is 20 MB in total; OpenAI allows 50 MB in total).

Deleting: **Clear**, **Write**, pruning an old message past **Max Stored**, and the Chat Panel's Clear and session delete all remove the files with their messages. `forget_files` removes only the files and leaves the conversation.

What to know before turning it on:

- Remembered files are stored in `ai_memory.db` in the app's data folder, unencrypted, like the message history, and stay until one of the deletions above.
- Each run that wires them to an AI node sends them to the provider again, and a recalled PDF costs its input tokens again every turn. With a `local` provider they stay on your machine.
- The recalled files are part of this node's output, so run history keeps a copy for each of the last 500 runs. Keep **Max Files** low, and turn **Include Files** on only for the node that feeds the AI node.
- PDFs reach OpenAI-compatible endpoints as `file` blocks, which only OpenAI itself is known to accept; a `local` or third-party endpoint may reject a conversation that carries a PDF. Text and markdown files are decoded into the system prompt for every provider.

### Text Splitter

Split a long piece of text into smaller chunks, by character count, word count, sentence count, or paragraph boundaries.

| Field | Label | Notes |
|---|---|---|
| `text` | Text | Required unless `source_field` is set. |
| `mode` | Mode | `chars`, `words`, `sentences`, or `paragraphs`. Default `chars` when blank. Any other value fails with `UNKNOWN_MODE`, which lists the valid ones. |
| `chunk_size` | Chunk Size | Max size of each chunk, in the unit `mode` sets, 1 or more (0 fails with `INVALID_CONFIG`). Default 1000 chars / 200 words / 5 sentences / 3 paragraphs. |
| `overlap` | Overlap | How much each chunk overlaps the previous one, same unit as Chunk Size. Helps an AI model keep context across chunks. Default 100 chars / 20 words / 1 sentence / 0 paragraphs. An overlap at or above Chunk Size is lowered to half of it, and a log line says so. |
| `source_field` | Source Field | Optional dot-path to a text field in the output of the node wired into this one, e.g. `body`; it is used instead of Text. If the field is missing, null or not text, the node fails with `SOURCE_FIELD_NOT_FOUND` rather than using Text. To read another node's output, use an expression such as `{{NodeName.output.body}}` in Text. |

**Ports:** In → Chunks.

**Output:** `chunks`, `total_chunks`, `total_chars`, `mode`, `chunk_size`, `overlap`.

**How each mode splits:**

- `chars`: by character, not byte. Each chunk is trimmed.
- `words`: splits on any whitespace and joins each chunk's words with a single space, so line breaks and repeated spaces inside a chunk are not kept. Use `chars` to keep the text's own spacing.
- `sentences`: a `.`, `!` or `?` ends a sentence only when whitespace follows it, so `3.14` and URLs stay whole; `。`, `！` and `？` end a sentence with no space after them. A `.`, `!` or `?` followed directly by a Chinese, Japanese or Korean character (`Hello.你好`) also ends a sentence. With `.`, `!` or `?`, a piece of up to 3 characters is kept with the next one. Arabic (`؟` `۔`), Armenian (`։`), Devanagari (`।` `॥`), Ethiopic (`።`), Khmer (`។`), Myanmar (`။`) and the `‼ ⁇ ⁈ ⁉` marks end a sentence wherever they appear. The Greek question mark is not recognized: Unicode normalization turns it into an ordinary semicolon. A short built-in list of common English, German, French and Spanish abbreviations does not end a sentence: `Mr.`, `Mrs.`, `Ms.`, `Mx.`, `Dr.`, `Prof.`, `Sr.`, `Jr.`, `St.`, `Mt.`, `vs.`, `cf.`, `Capt.`, `Col.`, `Gen.`, `Lt.`, `Sgt.`, `Rev.`, `Hon.`, `Mme.`, `Mlle.`, `Sra.`, `Srta.`, `Herr`, `Fr.`, `approx.`, `dept.`, `fig.`, `e.g.`, `i.e.`, `Ph.D.`, `z.B.`, `d.h.`, `u.a.`, and a single capital initial before another capital (`J. K. Rowling`). Words that often end a sentence (`etc.`, `No.`, `Inc.`, `U.S.`, `p.m.`) are not on the list. Any other abbreviation can still end a sentence.
- `paragraphs`: a paragraph ends at a blank line, including CRLF and bare CR line endings and lines holding only spaces or tabs. Chunks are joined with a blank line.

A Chunk Size at or above the number of units gives one chunk in every mode. Text over 2,000,000 characters fails with `TEXT_TOO_LARGE`, and a split that would return more than 100,000 chunks fails with `TOO_MANY_CHUNKS`; raise Chunk Size.

### Image Generation

Generate images from a text prompt. Cloud providers: GPT Image 1/2 (OpenAI), Nano Banana/Gemini (Google), Flux Pro/2 Pro (BFL). Local: Automatic1111, ComfyUI.

| Field | Label | Notes |
|---|---|---|
| `prompt` | Prompt | Required for every provider except `comfyui`, whose text lives in the workflow. |
| `provider` | Provider | Required; a missing provider fails with `MISSING_PROVIDER` and an unknown one with `UNKNOWN_PROVIDER`, both listing the valid values (it used to default to `gpt_image_1`). `gpt_image_1`, `gpt_image_2`, `flux_pro`, `flux_2_pro`, `imagen4`, `nano_banana`, `dalle3`, `a1111`, `comfyui`. `gpt_image_1` is the recommended cloud default; `a1111`/`comfyui` are for local inference. `imagen4`/`nano_banana` both route to NanoBanana, and `dalle3` is a legacy alias that routes to `gpt_image_1`. |
| `n` | N | Images to generate, 1 to 4. Default 1. GPT Image, NanoBanana and Flux send one request per image, so `n` images take about `n` times as long; Automatic1111 sends one batch. ComfyUI ignores `n` (a log line says so): its workflow decides how many images it saves. |
| `size` | Size | GPT Image only. `gpt_image_1`: `1024x1024`, `1536x1024`, `1024x1536`, or `auto`. `gpt_image_2`: any width x height divisible by 16, or the same presets. Default `1024x1024`. |
| `quality` | Quality | GPT Image only. `auto`, `low`, `medium`, `high`. Default `auto`. |
| `model` | Model | NanoBanana/Gemini only. `gemini-2.5-flash-image` (default), `gemini-nano-banana-2.1` or `gemini-3.1-flash-lite-image` (1K images only). Google shuts `gemini-2.5-flash-image` down on March 15 2027, and a run on it logs a warning saying so. Google's documentation shows the two newer models only on its Interactions API and does not say they work through the `generateContent` request this node sends, so a refusal comes back as `API_ERROR` with Google's message; switch back if that happens. Any other value fails with `UNKNOWN_MODEL`. |
| `aspect_ratio` | Aspect Ratio | NanoBanana/Gemini only. One of `1:1 1:4 1:8 2:3 3:2 3:4 4:1 4:3 4:5 5:4 8:1 9:16 16:9 21:9`. Default `1:1`. |
| `width` | Width | Flux and Automatic1111. `flux_pro`: 256 to 1440, rounded to the nearest 32. `flux_2_pro`: minimum 64. `a1111`: 64 to 2048, rounded to the nearest 8 (the server would otherwise cut the image down to a multiple of 8); the log says when a size was adjusted. Default 1024 (512 for `a1111`). |
| `height` | Height | Flux and Automatic1111. Same rules as Width. Default 768 (`flux_pro`), 1024 (`flux_2_pro`) or 512 (`a1111`). |
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

**Output:** `files` (generated images as media objects), `count`, `source` (the provider string configured on the node, for example `imagen4` or `nano_banana`).

**Downloads (Flux, ComfyUI):** an image is downloaded after the job finishes. A reply that is not a success, an empty one, or one whose `Content-Type` is text, JSON or XML (an error page served with a 2xx status) fails the node with `DOWNLOAD_ERROR`; it is never returned as an image. A missing or `application/octet-stream` type is accepted. A network error, a body that breaks off part-way, `408`, `429` or `5xx` is tried up to three times with a short wait; any other status fails at once. If any ComfyUI image cannot be downloaded the whole node fails. Flux links expire after 10 minutes.

**More than one image (`n`):** images are requested one after another. The loop stops, and says so in the log, after a bad or restricted key, no credits or quota, a blocked prompt, a location Google does not serve, a connection failure, or a timeout or poll failure (the provider may still be running, and billing, that job). A failure that concerns one image only (a failed job, a rate limit, a bad download, a blocked result) is skipped and the rest run. Images already made are returned with the error noted in the log.

**Failures worth knowing:**

| Code | Provider | Meaning |
|---|---|---|
| `INVALID_API_KEY` | Nano Banana | Google rejected the key (HTTP 401, or "API key not valid"). |
| `API_KEY_LEAKED`, `PROJECT_DENIED`, `API_KEY_RESTRICTED`, `PERMISSION_DENIED` | Nano Banana | An HTTP 403, told apart by Google's message: the key was reported as leaked, Google denied the project, the key's API restrictions exclude the Generative Language API, or any other refusal. The message repeats Google's text. Google documents no machine code for these, so the match is on the message. |
| `REGION_NOT_SUPPORTED` | Nano Banana | Google does not serve the Gemini API from this location (it answers HTTP 400, not 403). |
| `UNKNOWN_MODEL` | Nano Banana | `model` is not one of the listed values. |
| `PROMPT_BLOCKED` | Nano Banana | Gemini blocked the prompt; the message carries the `blockReason`. Stops the `n` loop. |
| `IMAGE_BLOCKED` | Nano Banana | Gemini blocked the image (`IMAGE_SAFETY` or `IMAGE_PROHIBITED_CONTENT`). |
| `NO_IMAGE_RETURNED` | Nano Banana | No image came back; the message carries the `finishReason` and anything the model said. |
| `INSUFFICIENT_QUOTA` | GPT Image | OpenAI credit or spending limit used up. Not retried, unlike a rate limit. |
| `CONTENT_POLICY_VIOLATION` | GPT Image, Flux | OpenAI `moderation_blocked` or content-policy rejection, or a BFL `Request Moderated` or `Content Moderated` job. Stops the `n` loop. |
| `RESPONSE_TOO_LARGE` | GPT Image | One image passed the 10 MB response limit and was discarded. Lower **Size** or **Quality**. |
| `INVALID_REFERENCE_IMAGE` | GPT Image | A reference image is empty or not valid base64. Nothing is sent. |
| `VALIDATION_ERROR` | ComfyUI | ComfyUI refused the workflow (HTTP 400). The message names the failing nodes and why, for example a missing model file. |

**Flux timeout:** after 120 seconds the node fails with `TIMEOUT`. The job is not cancelled and may still finish and be billed; the message holds the job id and polling URL. A ComfyUI `TIMEOUT` (10 minutes), `POLL_FAILED` or `POLL_REJECTED`, and a cancelled run, cancel the job on the ComfyUI server: a running job is interrupted by its `prompt_id`, a queued one is removed from the queue, and the error message says which. If the server's queue cannot be read, nothing is interrupted (an older server may ignore the id and stop someone else's job) and the message says the job may still run. A cancel that races the job's last moments can find it already finished.

**ComfyUI images:** previews (`type: temp`, from a Preview Image node) are skipped when the workflow also saves images; a workflow that only previews still returns them. A failed job reports ComfyUI's own error message and the node that raised it.

**Automatic1111 grid:** with `n` above 1 the server can put a grid image first; it is skipped when the response says where the real images start.

**Polling (Flux, ComfyUI):** after the provider accepts a job, the node polls for the result. A poll that fails for a passing reason (network error, `408`, `425`, `429`, `5xx`, or a body that is not JSON) is tried again with a growing wait, and the count resets after any good poll. Five failures in a row end the node with an unrecoverable `POLL_FAILED`, and `401`, `403`, `404` and other `4xx` replies end it at once with `POLL_REJECTED`. Neither error is retried by the scheduler, because a retry would submit, and bill, a second job: the message says the job was not resubmitted, and the provider may still finish it. A failed download of a finished Flux image is also not retried for the same reason.

## Logic

### If / Condition

Branch the workflow on a condition. True paths go to one output, false to another.

| Field | Label | Notes |
|---|---|---|
| `lhs` | Left Value | Left side of the comparison. Each side is kept whole, so a value that itself contains `>`, `=` or ` contains ` is not misread. |
| `op` | Operator | `==`, `!=`, `>`, `>=`, `<`, `<=` or `contains`. Default `==`. |
| `rhs` | Right Value | Right side of the comparison. |
| `condition` | Condition | The same test as one string, e.g. `{{temperature_2m}} > 20`, `{{status}} == ok`, `{{count}} >= 5`. Used when Left Value and Right Value are both blank, so workflows saved before those fields existed keep working. See [Expressions](expressions.md) for exactly how this gets evaluated, it's a smaller comparison language, not the full expression function set. |

**Ports:** In → True, False.

**Gotcha:** if Left Value or Right Value has text, Operator is used and Condition is ignored. Both sides are read after `{{...}}` expressions resolve, so a comparison against an empty value (`{{missing}} == `, with nothing in Right Value either) leaves both sides blank and falls back to Condition. Two numbers compare as numbers; anything else compares as text, ignoring case. An Operator other than the seven above fails the node with `INVALID_CONFIG`. An empty or blank condition doesn't error, it evaluates to false and logs "No condition specified, defaulting to false." `==`, `!=` and `contains` ignore case (Switch, below, does not).

### Switch

Route the workflow to one of several branches based on a value match, like a switch statement.

| Field | Label | Notes |
|---|---|---|
| `field` | Field | Required. Dot-path to switch on, e.g. `status`, `response.code` or `items.0.status` (a number indexes into an array). |
| `source_node` | Source Node | Required. Which upstream node's output to read Field from. |
| `cases` | Cases | Required. A JSON array of cases, e.g. `[{"match": "ok", "port": "case_1"}]`. |
| `default_port` | Default Port | Port to route to when nothing matches. Default `default`. |

**Ports:** In → Case 1 through Case 8 (eight fixed case ports), plus Default. Case labels on the canvas are generic; the app reads your actual case config to show the label you gave each one alongside the port ID.

**Gotcha:** a case with no `port` set gets auto-assigned to its position (`case_1`, `case_2`, ...); explicit ports are validated against the real port set, so a typo or an out-of-range case number fails the node loudly at config time instead of silently misrouting at run time. Only 8 case ports exist; a 9th case with no explicit port set fails for the same reason. Matching is exact and case-sensitive: a case `ok` does not match a field value of `OK`.

### Loop (For Each)

Run a downstream branch once for each item in a list, then continue with the collected results.

| Field | Label | Notes |
|---|---|---|
| `array_field` | Array Field | Required. Dot-path to the array to iterate, e.g. `items` or `response.results` (a number indexes into an array, e.g. `pages.0.items`). |
| `source_node` | Source Node | Required. Which upstream node's output to read Array Field from. |
| `item_var` | Item Variable | Variable name for the current item. Default `item`. |
| `index_var` | Index Variable | Variable name for the current index. Default `index`. |
| `max_iterations` | Max Iterations | Stop after this many items instead of the full array. Blank or 0 means no limit. Can only lower the effective bound, a value larger than the array (or larger than the hard 10,000-item cap) has no extra effect. |

**Ports:** In → Each Item, Done.

**Output:** `items`, `total`, `item`, `index`, `all_results`, `done`. On the Done output, `items` holds only the items that ran: when Max Iterations stops the loop early it is cut to `total`, not the full array.

**Gotcha:** 10,000 items is a hard cap regardless of Max Iterations. Item Variable and Index Variable can't be set to one of the node's own reserved output keys (`total`, `item`, `index`, `all_results`, `done`, and a couple of internal loop-control keys), and the two can't be set to the same name as each other, both fail the node at config time rather than silently corrupting the loop. A Loop can't sit inside another Loop's body: the run fails with a message naming both nodes (before this, the inner Loop quietly processed only its first item); put the inner Loop in a separate workflow or flatten the data first. A disabled inner Loop is ignored. `all_results` is capped at 256 MB once serialized, and the Loop fails past that, so return only the fields you need from the body.

### Stop

End this branch of the workflow. Downstream nodes on this path don't run; other independent branches are unaffected.

| Field | Label | Notes |
|---|---|---|
| `reason` | Reason | Optional. Logged when the branch stops. |

**Ports:** In only, no output. This is the one node on this page with no output port at all, since its whole job is to end a branch.

### Merge

Wait for every incoming branch that runs to finish, then continue as one execution path.

| Field | Label | Notes |
|---|---|---|
| `mode` | Mode | `object` (combine inputs keyed by node ID) or `array` (values only, no keys). |

**Ports:** In → Out. In accepts any number of incoming connections, one per branch it's merging; every other node's input accepts only one, and a workflow that wires several sources into one fails before running. Route them through a Merge node instead.

**Output:** the outputs of the nodes wired into this one, merged into one value and shaped per Mode. Only a node whose connection into the Merge actually fired contributes: a node on a branch that was not taken, a disabled or failed node, and any node that is not connected to the Merge are left out. In `array` mode the values follow the order the nodes finished. If nothing contributes, the output is `{}` (`object`) or `[]` (`array`).

**Skipped branches:** a branch that is not taken doesn't hold the Merge up. Send an If's two outputs through different nodes into one Merge and it runs with whichever branch ran. It still waits for any branch that is running or has yet to start. This is the same whether or not parallel execution is on.

### How Loop and Merge behave together

This is what the Loop (For Each), Merge and If / Switch pages above add up to when they are wired into one workflow.

- **Order.** A Loop runs its iterations one after another, never in parallel, and runs the nodes of its body in workflow order within each iteration. Every iteration finishes before the next starts, and Done runs after the last one.
- **What is in the body.** Every node reachable from **Each Item**, except what is only reachable through **Done**. Nodes wired straight to Each Item run every iteration. A node further down runs in an iteration only if the path to it was taken: after an If or Switch, only the branch that fired runs, and the other branch's nodes show as skipped for that iteration.
- **What counts as the iteration's result.** `all_results` gets one entry per iteration: the output of the last body node that ran and produced one. A body node does not add its own entry, and an iteration where no body node produced output adds none.
- **Failures.** A failing body node stops the loop with an error, unless that node has an `on_error` connection (or a failure route) to another body node. Then the loop carries on with the next iteration after running that route. A stop or cancel ends the loop between iterations.
- **Stale values.** A body node that was skipped or failed in this iteration still holds its output from an earlier one. Merge, the Code node's `input`, and Output ignore that old output and read only nodes that succeeded in the current iteration. An expression like `{{Node.output.x}}` that names such a node directly still reads the old value, so wire the node in, or use Merge.
- **Merge in a body.** Put a Merge after an If's two branches inside a Loop and it runs each iteration with whichever branch ran; its result holds only that iteration's outputs.
- **Merge after Done.** A Merge downstream of Done sees the Loop's output (`items`, `total`, `all_results`, `done`) like any other node.

### Collect Files

Gather files from multiple upstream sources into a single list for downstream processing.

| Field | Label | Notes |
|---|---|---|
| Sources | Sources | Not a plain config field, each source you add in the config panel becomes its own input port. Connect an upstream node to each one. |

**Ports:** dynamic. One input port per configured source, or a single generic "In" port if none are configured yet. Output: Out.

**Output:** `files` (merged from every source), `count`, `source`.

A file whose name is already taken (compared without regard to case) gets `_<source number>` before its extension, then `_<source number>_2`, `_3`, and so on, so every name in the result is unique; a file with no name becomes `file.bin`. A source that resolves to text that is not JSON is skipped and logged, a source holding broken JSON fails the node with `INVALID_SOURCE`, and a file entry that is not an object fails it with `INVALID_FILE_ENTRY`. The 10 MB limit counts base64 characters.

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

Pause the workflow for a fixed duration, or check a field once and route to Done (match) or Timed out (no match).

| Field | Label | Notes |
|---|---|---|
| `mode` | Mode | Required. `duration` (wait a fixed time) or `condition` (check once whether a value matches; no waiting). |
| `duration_secs` | Duration (seconds) | Duration mode. Default 5. |
| `field` | Field | Condition mode. Dot-path into upstream node outputs, e.g. `node_http.status` or `node_http.items.0.id`. |
| `expected` | Expected | Condition mode. The value Field must equal to leave through Done. `"200"` matches the number `200` and `"true"` matches `true`; other text compares exactly. A missing or null field never matches, even when Expected is blank. |
| `poll_interval_secs` | Poll Interval (seconds) | No longer used. Still accepted so saved workflows load. |
| `timeout_secs` | Timeout (seconds) | No longer used. Still accepted so saved workflows load. |

**Ports:** In → Done, Timed out. A condition that does not match leaves through Timed out straight away; if nothing is connected there, the workflow continues from Done.

**Output:** `waited_ms`, `timed_out`, `mode`.

**Gotcha:** this is a separate node from Delay, with a separate purpose. Delay is a plain fixed pause with one output port. Wait adds a one-time condition check and a dedicated Timed Out branch for when the value does not match. The check runs once, because node outputs are fixed when the node starts and waiting could not change them; to wait on a result, connect the node that produces it upstream.

### Transform Data

Reshape or extract data from the previous node's output using a template or expression.

| Field | Label | Notes |
|---|---|---|
| `source_node` | Source Node | Required. Which node to pull output from. |
| `mappings` | Mappings | Required. Array of `{from, to}` pairs: `from` is a JSON pointer into the source (e.g. `/user/name`), `to` is the key it lands on in this node's output. Edited as JSON in **Advanced: full config**. A pointer follows RFC 6901: `/` addresses a key named "" (an empty string), not the whole output, and `//a` looks up `a` inside that empty-named key. A blank `from` is skipped. `$` as `from` copies the whole source output (a key literally named `$` is `/$`). |

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
| `source_node` | Source Node | Which node's output `stringify`, `extract`, `merge`, and `array_get` read. `extract` pointers are relative to that output. Leave blank to read every node's output in the run (deprecated). |

**Ports:** default (In → Out).

**Output:** `result`.

**Gotcha:** with Source Node blank, these four operations read the whole run context, and `stringify` serializes every node's output, including data you may not mean to expose. The run log shows a warning each time. Set Source Node; it will become required in a future minor release. `parse` does not use it. A Source Node that names no node in the run fails with `SOURCE_NOT_FOUND`.

### Set Variable

Store a value in a named workflow variable, so any node downstream in the same run can read it back, either through the Get Variable node or an expression.

| Field | Label | Notes |
|---|---|---|
| `key` | Key | Required. Variable name. |
| `value` | Value | Value to store, any JSON type. |
| `persist` | Persist | When on, also saves the value to disk so it survives between runs, instead of only lasting for this one. Default off. In server mode running a single workflow with no database attached, this has no effect: the value still works for the rest of the current run, and the workflow logs a warning that nothing was saved to disk. If the save to disk fails, the node fails with `DB_ERROR` instead of succeeding. |

**Ports:** default (In → Out).

**Output:** `key`, `value`.

**Gotcha:** run logs for Set Variable and Get Variable show the variable's name and the type and size of its value, never the value itself. A variable set here is readable by name from the moment this node succeeds onward, using `{{$vars.key}}` in any downstream node's field, or from a Get Variable node placed later in the same branch. See [Expressions](expressions.md) for the `$vars` syntax.

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

PDF output uses the built-in Helvetica font with real glyph widths, so long lines and long words wrap inside the margins. That font covers Latin-1 plus common punctuation; any other character (CJK, emoji) is replaced with `?` and the node log says how many. In PDF and Word output only matched markdown emphasis is interpreted: `2 * 3 * 4` and `*.txt` stay as typed. An unrecognized `format` fails with `INVALID_FORMAT`; an unrecognized filename extension is passed through as plain text. When `mime_type` is blank it follows the format (`text/csv`, `application/json`, `text/html`, `text/markdown`, `text/plain`, `application/pdf`, or the Word type).

### Output

Mark the final output of the workflow. Required for the Chat Panel and the [embeddable widget](widget-embedding.md).

| Field | Label | Notes |
|---|---|---|
| `label` | Label | Shown above the result. Default `Result`. |
| `source_node` | Source Node | Which node's output to display. Leave blank to use whatever data reaches this node directly: the output of the node wired into this one. |
| `field` | Field | A specific field to pull out, e.g. `content` or `body.text`. Leave blank to show everything. |

**Ports:** default (In → Out).

**Output:** `value`, `label`, `type` (detected type: string, number, boolean, object, array, or null), `output_type` (set to `media_batch` when Value is a media payload).

## What's next

- [Expressions](expressions.md): the `{{...}}` syntax referenced throughout this page, full syntax and available context.
- [Concepts](../getting-started/concepts.md): the mental model this page assumes.
- [Glossary](../glossary.md): quick lookup for any term on this page.
