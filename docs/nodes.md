# Nodes Reference

All 39 built-in nodes, organized by category.

**Finding a node:** press `Space` or `Ctrl+K` on the canvas to open node search. Type any part of the node's name.

**`type_id`** is the node's internal identifier — stored in workflow JSON and used in the server API. You never need to type this in the visual interface, but it matters when building or managing workflows programmatically.

**Parameter types:** `string`, `number`, `boolean`, `object`, `array`. Required parameters are marked **required**.

---

## Quick index

| Category | Nodes |
|---|---|
| **Triggers** | [Manual Trigger](#manual-trigger), [Schedule](#schedule), [Webhook](#webhook) |
| **Logic** | [If / Condition](#if--condition), [Switch](#switch), [Loop (For Each)](#loop-for-each), [Merge](#merge), [Stop](#stop), [Collect Files](#collect-files) |
| **Flow Control** | [Delay](#delay), [Wait](#wait) |
| **AI** | [AI Prompt](#ai-prompt), [AI Agent](#ai-agent), [AI Memory](#ai-memory), [Text Splitter](#text-splitter), [Image Generation](#image-generation) |
| **Actions** | [HTTP Request](#http-request), [Shell Command](#shell-command), [Code (JS)](#code-js), [Send Email](#send-email), [SendGrid](#sendgrid), [File](#file), [Desktop Notification](#desktop-notification), [Save to Folder](#save-to-folder), [Social Upload](#social-upload), [Database](#database), [S3 Storage](#s3-storage) |
| **Integrations** | [Slack](#slack), [Discord](#discord), [GitHub](#github), [Google Sheets](#google-sheets), [Notion](#notion), [Telegram](#telegram), [Stripe](#stripe) |
| **Data & Utility** | [Transform Data](#transform-data), [JSON](#json), [Set Variable](#set-variable), [Get Variable](#get-variable), [Output](#output) |

---

## Per-node retry and error routing

Every node has two optional settings that control failure behavior.

### Retry

| Field | Type | Default | Notes |
|---|---|---|---|
| `max_attempts` | number | `1` | Total attempts including the first. `1` = no retry. Max `10`. |
| `backoff_ms` | number | `500` | Milliseconds to wait between retry attempts. |

Only set retries greater than 1 on operations that are safe to run twice — HTTP GET, database reads, AI prompts. Nodes with side effects (Send Email, Slack, Stripe) fire that side effect on every attempt.

### on_error port

Every node has a secondary `on_error` output port. When a node fails on its final attempt, execution routes through `on_error` instead of stopping the workflow.

To wire it: hover a node on the canvas. The `on_error` port appears on the right edge alongside the normal output. Drag from it to a downstream node — typically a Stop node, a Send Email alert, or a Slack message.

If `on_error` is not wired and a node fails, the workflow stops and the failure is recorded in run history.

---

## Triggers

Trigger nodes have no input port. They must be the first node in a workflow.

### Manual Trigger

`type_id: manual_trigger`

Runs when you press the Run button or call `POST /api/workflows/:id/run` on the server. No parameters.

Output: `{ triggered_at: string }`

Use this during development. Cannot be used as a scheduled trigger — exporting a workflow with only a Manual Trigger for server deployment is blocked.

### Schedule

`type_id: schedule`

| Parameter | Type | Notes |
|---|---|---|
| `mode` | string | **required.** `interval`, `cron`, or `once` |
| `interval_secs` | number | Seconds between runs. Minimum `10`. Used when `mode` is `interval`. |
| `cron_expr` | string | Standard 5-field cron expression, e.g. `0 9 * * 1-5`. Used when `mode` is `cron`. |
| `run_at` | string | ISO 8601 timestamp. Used when `mode` is `once`. |

Output: `{ triggered_at: string, mode: string }`

Common cron expressions:

| Expression | When |
|---|---|
| `* * * * *` | Every minute |
| `0 * * * *` | Every hour |
| `0 9 * * *` | Daily at 9 AM |
| `0 9 * * 1-5` | Weekdays at 9 AM |
| `*/15 * * * *` | Every 15 minutes |
| `0 9 1 * *` | Monthly on the 1st at 9 AM |

### Webhook

`type_id: webhook`

Listens for an incoming HTTP request and triggers the workflow when one arrives.

| Parameter | Type | Notes |
|---|---|---|
| `port` | number | Port to listen on. Default `3456`. Must be ≥ 1024. |
| `path` | string | URL path. Default `/webhook`. |
| `method` | string | `GET`, `POST`, `PUT`, or `ANY`. |
| `secret` | string | Optional. If set, incoming requests must include an `X-Aerini-Secret` header with this value. Comparison is timing-safe. |
| `timeout_secs` | number | How long to wait for a request before timing out. Default `60`. |

Output: `{ body: any, headers: object, method: string, path: string }`

The Webhook node binds to `127.0.0.1` (localhost only). In the desktop app, only processes on the same machine can reach it. For webhooks reachable from the internet — GitHub, Stripe, etc. — deploy to a server with a reverse proxy. See [Server Deployment](server-deploy.md#receiving-webhooks-on-a-server).

Always set a `secret` for any webhook that triggers sensitive actions. Without it, anyone who discovers the port can trigger your workflow.

Body size is capped at 1 MB. Connections that never complete their HTTP request are dropped after 30 seconds.

---

## Logic

### If / Condition

`type_id: if_condition`

Evaluates a condition and routes execution to one of two output ports.

| Parameter | Type | Notes |
|---|---|---|
| `condition` | string | Expression to evaluate. Supports `==`, `!=`, `>`, `<`, `>=`, `<=`, and `contains`. |

Ports: `on_true` and `on_false`.

The condition field is evaluated after expression resolution, so you write conditions using the resolved values — the expression system handles the substitution before this node runs:

```
{{weather.output.body.temperature}} > 25
{{status.output.code}} == ok
{{response.output.body.message}} contains error
```

String comparisons are case-insensitive. Numeric comparisons parse both sides as floats. The `contains` operator is a substring check on the left-hand side string.

If no condition is specified, the node routes to `on_false`.

Output: `{ result: boolean, branch: "on_true"|"on_false", condition: string }`

### Switch

`type_id: switch`

Routes execution to one of up to 8 named output ports based on a field's value.

| Parameter | Type | Notes |
|---|---|---|
| `field` | string | **required.** Dot-path to the value to switch on, e.g. `status` or `response.code` |
| `source_node` | string | Node ID to read the field from. Searches all previous outputs if blank. |
| `cases` | string | **required.** JSON array of `{ "match": "value", "port": "case_1" }` objects. |
| `default_port` | string | Port to route to when no case matches. Default: `default`. |

Available output ports: `case_1` through `case_8`, plus `default`. You assign each case to a port in the `cases` array — you can use any port ID for any case value. Port labels on the canvas show your case values alongside the port IDs.

```json
[
  { "match": "success", "port": "case_1" },
  { "match": "pending", "port": "case_2" },
  { "match": "failed",  "port": "case_3" }
]
```

Matching is exact and case-sensitive. Numbers and booleans are converted to strings for comparison (`true` becomes `"true"`, `42` becomes `"42"`). Null values match the string `"null"`.

Output: `{ matched_case: string | null, port: string, value: any, data: any }`

### Loop (For Each)

`type_id: loop`

Iterates over an array, running downstream nodes once per item.

| Parameter | Type | Notes |
|---|---|---|
| `array_field` | string | **required.** Dot-path to the array to iterate, e.g. `items` or `response.results` |
| `source_node` | string | Node ID to read the array from. Searches all previous outputs if blank. |
| `item_var` | string | Variable name for the current item. Default: `item` |
| `index_var` | string | Variable name for the current index. Default: `index` |

Ports: `loop_body` (fires once per item) and `done` (fires after all items are processed).

Each iteration produces:
```json
{
  "item":  "<current item>",
  "index": 0,
  "total": 10,
  "done":  false
}
```

When all items are processed, the `done` port fires with `all_results` populated — an array of the outputs collected from each `loop_body` execution.

The full array is not included in per-iteration output to avoid memory overhead on large arrays. Downstream nodes that need the full array should reference the loop node's context directly rather than the per-iteration `item` output.

Arrays over 10,000 items are rejected. For larger datasets, split into batches using Collect Files or multiple HTTP requests with pagination.

### Merge

`type_id: merge`

Waits for multiple upstream branches to complete, then combines their outputs into one value and continues execution.

| Parameter | Type | Notes |
|---|---|---|
| `mode` | string | `object` (default) — outputs keyed by node ID. `array` — outputs as a flat array. |

Output in `object` mode: `{ "node_id_1": <output>, "node_id_2": <output>, ... }`

Output in `array` mode: `[ <output1>, <output2>, ... ]`

Use Merge after branching logic (If / Condition or Switch) to recombine branches before continuing to shared downstream nodes.

### Stop

`type_id: stop`

Immediately terminates workflow execution at this point. Downstream nodes are not run.

| Parameter | Type | Notes |
|---|---|---|
| `reason` | string | Optional message logged when stopping. Visible in the Logs tab. |

Use Stop as an early exit in conditional branches — for example, after an If / Condition node when the false branch should end the workflow without error.

### Collect Files

`type_id: collect_files`

Merges file arrays from multiple upstream nodes into a single files array. Each connected upstream node becomes one labeled input port.

**Configuration:** in the node's config panel, click **Add Source** to add a named source slot. Each source gets a label (shown on the input port) and an expression field. Wire upstream nodes to the input ports, and the expression is auto-populated. You can also set the expression manually using `{{NodeName.output.files}}`.

Output: `{ files: array, count: number, source: "collect_files" }`

**Filename collision handling:** if two sources produce files with the same filename, the second file gets `_{source_index}` inserted before the extension automatically. For example: `image.png` and `image_1.png`. You will see a note in the Logs tab when this happens.

Collect Files is used to gather images from multiple Image Generation nodes before passing them to Save to Folder or Social Upload as a single batch.

---

## Flow Control

### Delay

`type_id: delay`

Pauses execution for a fixed number of milliseconds before continuing.

| Parameter | Type | Notes |
|---|---|---|
| `delay_ms` | number | **required.** Milliseconds to wait. |

Output: `{ delayed_ms: number }`

### Wait

`type_id: wait`

Pauses execution for a set duration, or until a field in the workflow context reaches an expected value.

| Parameter | Type | Notes |
|---|---|---|
| `mode` | string | **required.** `duration` or `condition` |
| `duration_secs` | number | Seconds to wait. Duration mode only. Default: `5`. Max: `3600`. |
| `field` | string | Dot-path to a context field to monitor, e.g. `node_http.status`. Condition mode only. |
| `expected` | any | Value the field must equal before continuing. Condition mode only. |
| `poll_interval_secs` | number | How often to re-check the field. Default: `2`. Minimum: `0.5`. |
| `timeout_secs` | number | Max time to wait before routing to `timed_out`. Default: `60`. |

Ports: `output` (condition met or duration elapsed) and `timed_out` (timeout reached in condition mode).

Output: `{ waited_ms: number, timed_out: boolean, mode: string }`

---

## AI

### AI Prompt

`type_id: ai_prompt`

Single-turn LLM call. Sends a prompt to an AI model, returns the response.

| Parameter | Type | Notes |
|---|---|---|
| `prompt` | string | **required.** The user message. |
| `system` | string | System/persona instructions. Default: `"You are a helpful assistant."` |
| `model` | string | Model name, e.g. `gpt-4o`, `claude-opus-4-5`, `gemini-2.5-flash`, `llama3`. |
| `provider` | string | `auto`, `openai`, `anthropic`, or `gemini`. `auto` detects from `base_url`. |
| `base_url` | string | API base URL. Leave blank for OpenAI. For Ollama: `http://localhost:11434/v1`. Note: localhost URLs are blocked in server mode. |
| `api_key` | string | API key — use a Credential. |
| `temperature` | number | 0.0–2.0. Default `0.7`. Lower = more precise, higher = more creative. |
| `max_tokens` | number | Maximum response tokens. Default `2048`. |
| `rate_limit_rpm` | number | Max requests per minute. `0` = unlimited. A per-iteration delay is injected when this is set. |

Output: `{ content: string, model: string, provider: string, input_tokens: number, output_tokens: number }`

Request timeout is 120 seconds. Network errors and timeouts are retried if `max_attempts > 1`. Rate limit errors (HTTP 429) are treated as recoverable.

**Provider detection:** when `provider` is `auto`, the provider is detected from the `base_url`:
- URL contains `anthropic.com` → Anthropic
- URL contains `googleapis.com` or `generativelanguage` → Gemini
- Everything else (including Ollama, Groq, Together, OpenRouter, Mistral) → OpenAI-compatible

Ports: `output` (Success) and `on_error` (Error).

### AI Agent

`type_id: ai_agent`

Autonomous reasoning loop. The agent is given a goal, thinks through it step by step, optionally calls tools, and repeats until it reaches a conclusion or hits `max_iterations`.

| Parameter | Type | Notes |
|---|---|---|
| `goal` | string | **required.** What the agent should accomplish. Be specific — vague goals produce vague results. |
| `provider` | string | **required.** `openai`, `anthropic`, `gemini`, or `auto`. |
| `system` | string | Persona and behavioral instructions. Default: `"You are a helpful AI agent..."` |
| `context` | string | Additional context from previous nodes. Use expressions: `{{http.output.body}}` |
| `tools` | string | JSON array of tool definitions: `[{"name": "...", "description": "...", "parameters": {...}}]`. OpenAI and Gemini only. |
| `model` | string | Model name. OpenAI default: `gpt-4o`. Anthropic default: `claude-opus-4-5`. Gemini default: `gemini-2.5-flash`. |
| `base_url` | string | Custom API base URL. Leave blank for defaults. |
| `api_key` | string | API key — use a Credential. |
| `max_iterations` | number | Safety limit on reasoning cycles. Default `5`. Max `20`. Ignored for Anthropic. |
| `max_tokens` | number | Max tokens per response. Default `2048`. Max `8192`. |
| `temperature` | number | 0.0–1.0. Agents work best at 0.2–0.5. Default `0.3`. |

Output: `{ result: string, iterations: number, tool_calls: array, reasoning: array, finished: boolean }`

`finished: true` means the agent reached a conclusion. `finished: false` means it hit `max_iterations` without concluding.

**Provider differences:**

OpenAI and Gemini support multi-step tool-calling loops. The agent can call tools, receive results, and reason across multiple iterations before returning a final answer. Tool calls are recorded in `tool_calls` in the output.

Anthropic runs a single reasoning pass — it thinks through the goal once and returns a final answer. There is no iteration. The `tools` parameter is ignored for Anthropic. Use OpenAI or Gemini when you need a multi-step tool loop.

Gemini uses a different message format internally (role `model` instead of `assistant`, `functionCall`/`functionResponse` instead of `tool_calls`) — this is handled automatically.

Ports: `output` (Done) and `on_error` (Error).

### AI Memory

`type_id: ai_memory`

Stores and retrieves conversation history across workflow runs using a local SQLite database. Use it to build chatbots or AI assistants that remember previous turns.

| Parameter | Type | Notes |
|---|---|---|
| `operation` | string | **required.** `read`, `write`, `append`, or `clear` |
| `session_id` | string | **required.** Identifies the conversation thread. Use a consistent ID per user or conversation, e.g. `user_123` or `session_abc`. |
| `role` | string | `user`, `assistant`, or `system`. Required for `write` and `append`. |
| `content` | string | Message content. Required for `write` and `append`. |
| `max_messages` | number | How many past messages to return on `read`. Default `20`. Most recent messages are returned first. |

**Operations:**

`read` returns the stored message history as a `messages` array, ordered oldest-first, ready to pass to an AI Prompt or AI Agent node's context field.

`write` replaces the entire session history with a single message. Use this to start a new conversation or set a system prompt.

`append` adds one message to the existing history without erasing it. Use after each AI turn: append the user's message, run the AI, then append the assistant's response.

`clear` deletes all messages for the session. Does not fail if the session doesn't exist.

Output: `{ messages: array, count: number, session_id: string }`

**Typical pattern for a multi-turn chatbot:**

1. AI Memory (append, role: user, content from webhook)
2. AI Memory (read, max_messages: 20) → passes `messages` to AI
3. AI Prompt (prompt uses `{{memory.output.messages}}`)
4. AI Memory (append, role: assistant, content from AI Prompt output)

### Text Splitter

`type_id: text_splitter`

Splits a long text into overlapping chunks. Used to prepare large documents for AI nodes that have context window limits.

| Parameter | Type | Notes |
|---|---|---|
| `text` | string | **required.** The text to split. |
| `chunk_size` | number | Target chunk size in characters. Default `1000`. |
| `overlap` | number | Characters of overlap between consecutive chunks. Default `100`. |

Output: `{ chunks: string[], count: number }`

Overlap helps AI nodes that process each chunk independently to maintain context across chunk boundaries. Set `overlap` to roughly 10% of `chunk_size`.

Pass the resulting `chunks` array to a Loop node to process each chunk with an AI Prompt node, then collect results with Merge.

### Image Generation

`type_id: image_gen`

Generates images using DALL-E 3 or Google Imagen 4 and returns them as a media contract — a standard file format accepted by Save to Folder, Social Upload, and Collect Files.

| Parameter | Type | Notes |
|---|---|---|
| `prompt` | string | **required.** Image description. |
| `provider` | string | **required.** `dalle3` or `imagen4` |
| `api_key` | string | **required.** OpenAI key for DALL-E 3. Google AI API key for Imagen 4. |
| `n` | number | Images to generate. 1–4. Default `1`. |
| `size` | string | DALL-E 3 only. `1024x1024` (default), `1792x1024`, or `1024x1792`. |
| `quality` | string | DALL-E 3 only. `standard` (default) or `hd`. |
| `aspect_ratio` | string | Imagen 4 only. `1:1` (default), `3:4`, `4:3`, `9:16`, or `16:9`. |

Output: `{ files: array, count: number, source: "dalle3"|"imagen4" }`

Each file object: `{ filename: string, data: string (base64), mime_type: "image/png" }`

**DALL-E 3 note:** DALL-E 3's API only generates one image per request. When `n > 1`, Aerini makes `n` sequential API calls. If one call fails, the node returns partial results — the successfully generated images are included in the output with a warning in the Logs tab. This means `n = 4` costs 4x the API credits and takes roughly 4x the time.

**Imagen 4 note:** Imagen 4 supports up to 4 images in a single API call. Set `n` between 1 and 4.

**Image file naming:** files are named automatically — `dalle3_{timestamp}_{index}.png` for DALL-E, `imagen4_{timestamp}_{index}.png` for Imagen 4.

**Common content policy errors (DALL-E 3):** if the prompt is rejected by OpenAI's content policy, the node fails with `CONTENT_POLICY_VIOLATION`. This is a hard failure — retrying with the same prompt will produce the same rejection. Modify the prompt.

---

## Actions

### HTTP Request

`type_id: http_request`

Makes an HTTP request to any URL and returns the response.

| Parameter | Type | Notes |
|---|---|---|
| `method` | string | **required.** `GET`, `POST`, `PUT`, `PATCH`, or `DELETE` |
| `url` | string | **required.** Must be `http` or `https`. |
| `headers` | object | Key-value pairs added to the request. |
| `body` | any | Request body. Objects are serialized as JSON automatically. |
| `timeout_secs` | number | Default `30`. |

Output: `{ status: number, headers: object, body: any }`

URLs pointing to private IP ranges (10.x.x.x, 172.16.x.x, 192.168.x.x, 127.x.x.x, etc.) and cloud metadata endpoints (169.254.169.254) are blocked to prevent SSRF attacks. Redirects are disabled — if a URL returns a 3xx response, you receive it directly rather than Aerini following it. Response body is capped at 10 MB.

### Shell Command

`type_id: shell_exec`

Runs a shell command on the local machine and returns stdout, stderr, and the exit code.

| Parameter | Type | Notes |
|---|---|---|
| `command` | string | **required.** Shell command to run. |
| `cwd` | string | Working directory for the command. |
| `env` | object | Additional environment variables for the subprocess. |
| `timeout_secs` | number | Default `30`. |

Output: `{ stdout: string, stderr: string, exit_code: number, success: boolean }`

On Windows, commands run via `cmd /C`. On Unix, via `sh -c`.

In the desktop app, every workflow containing this node shows a confirmation dialog before running. In server mode, this node is disabled by default — pass `--allow-shell` to enable it. Treat workflows with Shell Command nodes as trusted code: a workflow with this node has your full user permissions.

Sensitive values in the logged command (passwords, API keys) are automatically redacted from run history.

### Code (JS)

`type_id: code`

Runs a JavaScript snippet using the system `node` binary. Useful for data transformation that expressions can't handle.

> **Requires Node.js 18+** on your PATH. Only needed if you use this node — not a global prerequisite for Aerini.
> If Node.js is not found when this node runs, you will see a clear error message.
> Install from [nodejs.org](https://nodejs.org).

| Parameter | Type | Notes |
|---|---|---|
| `code` | string | **required.** JavaScript to execute. |
| `timeout_secs` | number | Max execution time. Default `10`. Max `60`. |

Output: `{ result: any, stdout: string, duration_ms: number }`

The snippet receives two variables:
- `input` — the data passed from the upstream node
- `context` — all node outputs from the current run (keyed by node ID)

Return a value using `output()`:

```javascript
const price = input.body?.price ?? 0;
const tax = price * 0.2;
output({ price, tax, total: price + tax });
```

Code runs as an ES module (`--input-type=module`). Use `import` syntax, not `require()`. Only built-in Node.js modules are available — npm packages are not supported. Node.js must be installed and on PATH.

In server mode, disabled by default — pass `--allow-code` to enable.

### Send Email

`type_id: email_send`

Sends an email via SMTP. Works with Gmail, Outlook, Fastmail, and any SMTP provider.

| Parameter | Type | Notes |
|---|---|---|
| `smtp_host` | string | **required.** e.g. `smtp.gmail.com` |
| `smtp_port` | number | `587` (STARTTLS, recommended) or `465` (TLS). Default `587`. |
| `from` | string | **required.** Sender email address. |
| `to` | string | **required.** Recipient(s). Comma-separate multiple addresses. |
| `subject` | string | **required.** |
| `body` | string | **required.** |
| `html` | boolean | Send as HTML email. Default `false`. |
| `username` | string | SMTP username (usually your email address). |
| `password` | string | SMTP password — use a Credential. |

Output: `{ sent: boolean }`

Common SMTP settings:

| Provider | Host | Port | Notes |
|---|---|---|---|
| Gmail | `smtp.gmail.com` | `587` | Use an App Password, not your account password. Requires 2-factor auth enabled. |
| Outlook | `smtp.office365.com` | `587` | Full email address as username. |
| Fastmail | `smtp.fastmail.com` | `587` | Full email address and a Fastmail app password. |

### SendGrid

`type_id: sendgrid`

Sends transactional email via the SendGrid API.

| Parameter | Type | Notes |
|---|---|---|
| `to_email` | string | **required.** Recipient address. |
| `from_email` | string | **required.** Must be a verified sender address in your SendGrid account. |
| `subject` | string | **required.** |
| `body` | string | **required.** Plain text only. |
| `api_key` | string | SendGrid API key (starts with `SG.`) — use a Credential. |

Output: `{ sent: boolean, message_id: string }`

### File

`type_id: file`

Read, write, append to, delete, or check existence of a file on the local filesystem.

| Parameter | Type | Notes |
|---|---|---|
| `operation` | string | **required.** `read`, `write`, `append`, `delete`, or `exists` |
| `path` | string | **required.** Absolute or relative file path. |
| `content` | string | Content to write or append. Required for `write` and `append`. |
| `encoding` | string | `utf8` (default) or `base64`. |

Output: `{ content: string, exists: boolean, bytes: number, path: string }`

Read limit: 50 MB. Path traversal sequences (`..`) are rejected. In the desktop app, a confirmation prompt appears before any workflow with a File node runs. In server mode, use `--file-sandbox-dir` to restrict File nodes to a specific directory tree.

### Desktop Notification

`type_id: notification`

Shows an OS desktop notification on the machine running Aerini.

| Parameter | Type | Notes |
|---|---|---|
| `title` | string | **required.** Notification title. |
| `body` | string | Notification message body. |
| `urgency` | string | Linux only. `low`, `normal` (default), or `critical`. |

In server mode (no desktop), this node logs a warning and continues — it does not fail the workflow. Don't configure retries on it.

### Save to Folder

`type_id: save_to_folder`

Writes media contract files to a local folder. Accepts two modes: flat (one source, one folder) and subfolder (multiple named sources, each written to its own subfolder).

#### Flat mode

Connect a single upstream node (Image Generation, Collect Files, or Social Upload) directly to the input port. All files go into the configured folder.

| Parameter | Type | Notes |
|---|---|---|
| `folder_path` | string | **required.** Set using the Choose Folder button in the config panel. |
| `overwrite` | boolean | Overwrite existing files with the same name. Default `true`. |
| `filename_prefix` | string | Optional prefix prepended to every filename, e.g. `batch_`. |

#### Subfolder mode

When you add subfolders in the config panel, each subfolder becomes a separate labeled input port on the node. Each port accepts one media source, and its files are written to `folder_path/<subfolder_name>/`.

**To add a subfolder:** open the node's config panel and click **Add Subfolder**. Give it a name — this becomes both the port label and the subfolder name on disk. Connect upstream nodes to each subfolder's input port. Subfolders are created automatically if they don't exist.

**Example:** you have two Image Generation nodes — one generating product photos and one generating lifestyle shots. Add two subfolders named `product` and `lifestyle`. Connect the first Image Generation to `product` and the second to `lifestyle`. The files land at `~/images/product/dalle3_*.png` and `~/images/lifestyle/dalle3_*.png`.

Output: `{ saved: array, count: number, folder: string, skipped: number, errors: array }`

Each entry in `saved`: `{ filename: string, path: string, bytes: number }`

Each entry in `errors`: `{ filename: string, reason: string }`

**Filename sanitization:** forward slashes, backslashes, and `..` sequences are stripped from every filename before writing. A filename that reduces to nothing after sanitization is saved as `file.bin`. This is logged in the Logs tab.

**Concurrent writes:** all files in a batch are written concurrently. On large batches, disk I/O may be the bottleneck.

In server mode, `--file-sandbox-dir` applies. Symlinks inside the sandbox that resolve outside it are detected and blocked.

### Social Upload

`type_id: social_upload`

Uploads media to YouTube, Instagram, or TikTok using OAuth 2.0.

| Parameter | Type | Notes |
|---|---|---|
| `platform` | string | **required.** `youtube`, `instagram`, or `tiktok` |
| `files` | array | **required.** Media contract files array (from Image Generation or Collect Files). |
| `title` | string | **required.** Post title. |
| `description` | string | Post description. |
| `tags` | string | Comma-separated tags. YouTube only. |
| `privacy` | string | YouTube: `public`, `private` (default), or `unlisted`. TikTok: `public_to_everyone`, `mutual_follow_friends`, or `self_only` (default). |
| `client_id` | string | **required.** OAuth client ID (TikTok: client key). |
| `client_secret` | string | **required.** OAuth client secret. |

Output: `{ uploaded: array, count: number, platform: string, errors: array }`

Each entry in `uploaded`: `{ filename: string, platform_id: string, url: string }`

Each entry in `errors`: `{ filename: string, code: string, message: string, explanation: string, action: string }` — the `explanation` and `action` fields describe exactly what went wrong and what to do about it.

**OAuth flow:** on the first run, Aerini opens a browser window and prompts you to authorize the app. OAuth tokens are stored in the OS keychain and refreshed automatically. The OAuth redirect listener uses port `42069` (localhost). Subsequent runs use the stored token without prompting.

**Platform-specific details:**

YouTube: uploads use the multipart upload protocol. Each file in the `files` array is uploaded as a separate video with the same `title` and `description`. Tags are split on commas.

Instagram: requires media hosted at a publicly accessible URL. The media contract's base64 data cannot be sent to Instagram directly — Instagram's API only accepts URLs. If you pass base64 data, the node returns `INSTAGRAM_NEEDS_PUBLIC_URL` with instructions. Upload your files to S3 first, generate a presigned URL, and use that as the `data` value.

TikTok: uses the Direct Post API with chunked upload (10 MB chunks). Files up to 4 GB are supported. Chunk uploads are retried up to 3 times on transient network errors. TikTok processes videos asynchronously after all chunks are received — the `platform_id` in the output is a `publish_id` you can use to check status.

### Database

`type_id: database`

Query or write to SQLite, PostgreSQL, MySQL, or Redis.

#### SQLite

| Parameter | Type | Notes |
|---|---|---|
| `db_type` | string | `sqlite` |
| `db_path` | string | **required.** Absolute path ending in `.db`, `.sqlite`, or `.sqlite3`. Relative paths are rejected. |
| `operation` | string | `query` (SELECT) or `execute` (INSERT / UPDATE / DELETE). Default `query`. |
| `query` | string | **required.** SQL statement. Use `?` placeholders for parameters. |
| `params` | string | JSON array of positional parameters, e.g. `[42, "Alice"]`. |

SQLite connections use WAL mode and a 5-second busy timeout. Up to 4 concurrent connections per database file.

The `query` operation only accepts SELECT (and WITH) statements. For writes, use `execute`. If you use `query` with an INSERT or UPDATE, you'll get an error explaining this.

Output: `{ rows: array, rows_affected: number, last_insert_id: number, columns: array }`

**SQL injection warning:** always use `?` placeholders for any value that comes from workflow data or external input. Passing values directly in the query string via expression substitution is not safe:

```sql
-- Safe: value is bound as a parameter
SELECT * FROM users WHERE id = ?
```
```json
["{{trigger.output.body.id}}"]
```

```sql
-- Unsafe: do not do this
SELECT * FROM users WHERE id = '{{trigger.output.body.id}}'
```

If Aerini detects single-quoted values in the query string, a warning is added to the Logs tab.

#### PostgreSQL / MySQL

| Parameter | Type | Notes |
|---|---|---|
| `db_type` | string | `postgres` or `mysql` |
| `connection_url` | string | **required.** Full connection URL including credentials. Use a Credential. |
| `operation` | string | `query` or `execute`. Default `query`. |
| `query` | string | **required.** SQL. Use `?` placeholders. Aerini automatically rewrites `?` to PostgreSQL's native `$1, $2, ...` syntax — you do not need to use `$N` yourself. |
| `params` | string | JSON array of positional parameters. |

Connection URLs:
- Postgres: `postgres://user:pass@host:5432/dbname`
- MySQL: `mysql://user:pass@host:3306/dbname`

`last_insert_id` is always `0` for Postgres. Use a `RETURNING id` clause with `operation: query` to get the inserted ID:
```sql
INSERT INTO users (name) VALUES (?) RETURNING id
```

Connection pools are cached per URL (keyed by a hash of the URL, not the URL itself). Idle connections are evicted after 30 minutes.

Private IP addresses in connection URLs are blocked in server mode (SSRF protection).

#### Redis

| Parameter | Type | Notes |
|---|---|---|
| `db_type` | string | `redis` |
| `connection_url` | string | **required.** Redis URL. Use a Credential. |
| `operation` | string | **required.** `get`, `set`, `del`, `lpush`, `rpush`, `lpop`, `rpop`, `hget`, or `hset` |
| `key` | string | **required.** Redis key. |
| `value` | string | Required for `set`, `lpush`, `rpush`, `hset`. |
| `field` | string | Hash field name. Required for `hget`, `hset`. |
| `expire` | number | TTL in seconds for `set`. Omit for no expiry. |

Redis connections are multiplexed and cached. On connection drop, a single reconnect attempt is made automatically before failing.

Outputs by operation:

| Operation | Output |
|---|---|
| `get`, `hget`, `lpop`, `rpop` | `{ value: string \| null }` |
| `set`, `hset` | `{ ok: true, key: string }` |
| `del` | `{ deleted: number, key: string }` |
| `lpush`, `rpush` | `{ ok: true, list_length: number }` |

### S3 Storage

`type_id: s3_storage`

Upload, download, list, delete, and generate presigned URLs for S3-compatible object storage. Supports AWS S3, Cloudflare R2, and MinIO.

| Parameter | Type | Notes |
|---|---|---|
| `operation` | string | **required.** `upload`, `download`, `list`, `delete`, or `presign_url` |
| `provider` | string | `aws` (default), `r2`, or `minio` |
| `access_key_id` | string | **required.** Access key ID — use a Credential. |
| `secret_access_key` | string | **required.** Secret access key — use a Credential. |
| `bucket` | string | **required.** Bucket name. |
| `region` | string | AWS region, e.g. `us-east-1`. For R2: use `auto`. Ignored for MinIO. Default `us-east-1`. |
| `endpoint` | string | Custom endpoint URL. Required for `r2` and `minio`. Unused for `aws`. |
| `key` | string | Object key (path in the bucket). Required for `upload`, `download`, `delete`, `presign_url`. |
| `content` | string | Content to upload. Required for `upload`. |
| `content_encoding` | string | `text` (default) or `base64`. Use `base64` when uploading binary data from an upstream node. |
| `content_type` | string | MIME type for upload. Default `application/octet-stream`. |
| `prefix` | string | Key prefix filter for `list`. Empty string lists all objects in the bucket. |
| `expiry_secs` | number | Presigned URL TTL in seconds. Default `3600`. Max `4294967295`. |

Provider setup:
- AWS S3: set `region`. No `endpoint` needed.
- Cloudflare R2: set `provider: r2` and `endpoint: https://ACCOUNT_ID.r2.cloudflarestorage.com`. Set `region: auto`.
- MinIO: set `provider: minio` and `endpoint: http://host:9000`. Set any string for `region`.

R2 and MinIO use path-style addressing automatically. AWS uses virtual-hosted style.

Endpoints pointing to private/internal IP ranges are blocked in server mode (SSRF protection).

Outputs by operation:

| Operation | Output |
|---|---|
| `upload` | `{ key, size, content_type }` |
| `download` | `{ key, content (base64), content_encoding: "base64", size }` |
| `list` | `{ objects: [{key, size, last_modified}], prefix, count }` |
| `delete` | `{ key, deleted: true }` |
| `presign_url` | `{ url, key, expires_in }` |

---

## Integrations

### Slack

`type_id: slack`

Posts a message to a Slack channel.

| Parameter | Type | Notes |
|---|---|---|
| `channel` | string | **required.** Channel ID (`C1234567890`) or name (`#general`). |
| `text` | string | **required.** Message text. Slack markdown is supported. |
| `api_key` | string | Slack Bot Token (`xoxb-...`) — use a Credential. |

Output: `{ ok: boolean, ts: string, channel: string }`

The bot must be invited to the channel before posting. Use `/invite @BotName` in the channel. If `ok` is false, the Logs tab shows Slack's error message.

### Discord

`type_id: discord`

Posts a message to a Discord channel via webhook.

| Parameter | Type | Notes |
|---|---|---|
| `webhook_url` | string | **required.** Discord webhook URL from Server Settings → Integrations → Webhooks. |
| `content` | string | **required.** Message text. Max 2000 characters. |
| `username` | string | Override the webhook's display name for this message. |

Output: `{ sent: boolean }`

### GitHub

`type_id: github`

Create issues or add comments to existing issues on a GitHub repository.

| Parameter | Type | Notes |
|---|---|---|
| `action` | string | **required.** `create_issue` or `add_comment` |
| `owner` | string | **required.** Repository owner (username or org name). |
| `repo` | string | **required.** Repository name. |
| `title` | string | Issue title. Required for `create_issue`. |
| `body` | string | Issue body or comment text. Markdown supported. |
| `issue_number` | number | Issue number. Required for `add_comment`. |
| `api_key` | string | GitHub personal access token — use a Credential. |

Output: `{ number: number, html_url: string, id: number, state: string }`

Token scopes needed: `repo` for private repositories, `public_repo` for public ones.

### Google Sheets

`type_id: google_sheets`

Read from or append rows to a Google Sheets spreadsheet.

| Parameter | Type | Notes |
|---|---|---|
| `action` | string | **required.** `append_row` or `get_values` |
| `spreadsheet_id` | string | **required.** The ID from the spreadsheet URL (the long string between `/d/` and `/edit`). |
| `range` | string | **required.** A1 notation, e.g. `Sheet1!A1:D1`. |
| `values` | array | Row data for `append_row`. Format: `[["col1", "col2", "col3"]]`. Outer array = rows, inner array = cells. |
| `api_key` | string | Google OAuth 2.0 access token — use a Credential. |

Output: `{ values: array, updatedRange: string, updatedRows: number, updatedColumns: number, updatedCells: number }`

The `api_key` field takes a Google OAuth 2.0 access token — not a plain API key. You need a Google Cloud service account with the Sheets API enabled. Share the target spreadsheet with the service account's email address, or all requests return 403. See [Credentials — Google Sheets](credentials.md#google-sheets) for setup.

### Notion

`type_id: notion`

Create or update pages in a Notion database.

| Parameter | Type | Notes |
|---|---|---|
| `action` | string | **required.** `create_page` or `update_page` |
| `database_id` | string | Notion database ID. Required for `create_page`. |
| `page_id` | string | Notion page ID. Required for `update_page`. |
| `title` | string | Page title. |
| `properties` | object | Additional Notion page properties as a JSON object matching your database schema. |
| `api_key` | string | Notion integration token (`secret_...`) — use a Credential. |

Output: `{ id: string, url: string, object: string, archived: boolean }`

Create an integration at [notion.so/my-integrations](https://www.notion.so/my-integrations). Share each database with the integration — the integration cannot see databases that haven't been shared with it.

### Telegram

`type_id: telegram`

Sends a message to a Telegram chat, channel, or group.

| Parameter | Type | Notes |
|---|---|---|
| `chat_id` | string | **required.** Chat ID (numeric, e.g. `123456789`) or public username (e.g. `@mychannel`). |
| `text` | string | **required.** Message text. |
| `parse_mode` | string | `` (none), `Markdown`, or `HTML`. |
| `api_key` | string | Bot token from @BotFather — use a Credential. |

Output: `{ message_id: number, chat: object, text: string, date: number }`

To get a chat ID: forward a message from the target chat to @userinfobot, or use the getUpdates API method after sending the bot a message.

### Stripe

`type_id: stripe`

Create payment intents in Stripe.

| Parameter | Type | Notes |
|---|---|---|
| `action` | string | **required.** `create_payment_intent` |
| `amount` | number | **required.** Amount in the smallest currency unit. $10.00 USD = `1000`. €5.50 EUR = `550`. |
| `currency` | string | Three-letter ISO 4217 currency code, lowercase. e.g. `usd`, `eur`, `gbp`. |
| `description` | string | Optional description attached to the payment intent. |
| `api_key` | string | Stripe secret key (`sk_live_...` or `sk_test_...`) — use a Credential. |

Output: `{ id: string, client_secret: string, status: string, amount: number, currency: string }`

Only `create_payment_intent` is currently implemented.

Do not set `max_attempts > 1` on this node. A retry on transient failure creates a new payment intent — you may end up with multiple intents for the same transaction. Use `max_attempts: 1` and handle failures via the `on_error` port.

---

## Data & Utility

### Transform Data

`type_id: transform`

Extracts and remaps fields from a node's output into a new object. Use this to reshape data between nodes when you need specific field names.

| Parameter | Type | Notes |
|---|---|---|
| `source_node` | string | Node ID to pull output from. Uses all previous node outputs if omitted. |
| `mappings` | array | **required.** Array of `{ "from": "/json/pointer", "to": "output_key" }` objects. |

`from` is a JSON Pointer (RFC 6901). It starts with `/` and uses `/` to navigate nested objects:

```json
{
  "mappings": [
    { "from": "/body/user/name",  "to": "username" },
    { "from": "/body/user/email", "to": "email" },
    { "from": "/status",         "to": "http_status" }
  ]
}
```

Arrays are addressed by index: `/body/results/0/title` gives the `title` of the first result.

JSON Pointer escape sequences: `~1` = `/`, `~0` = `~`. Use these if a key name literally contains a slash or tilde.

If a `from` path isn't found in the source, the corresponding output field is set to `null` and a warning is logged.

Output: the mapped object. Only the keys you defined in `mappings` are present.

### JSON

`type_id: json`

Parse, stringify, extract, merge, or index into JSON data.

| Parameter | Type | Notes |
|---|---|---|
| `operation` | string | **required.** `parse`, `stringify`, `extract`, `merge`, or `array_get` |
| `input_text` | string | JSON string to parse. Used by `parse`. |
| `pointer` | string | JSON Pointer path, e.g. `/user/name` or `/0/temperature`. Used by `extract`. |
| `index` | number | Array index (0-based). Used by `array_get`. Default `0`. |

Operations:

`parse` — converts a JSON string to a value. Use when you have a string that contains JSON and need to access its fields.

`stringify` — serializes all context outputs into a single JSON string.

`extract` — retrieves a value from context using a JSON Pointer path.

`merge` — flattens all upstream node outputs into a single object.

`array_get` — retrieves one element from an array in context by zero-based index.

Output: `{ result: any }`

### Set Variable

`type_id: set_variable`

Stores a value for use later in the same workflow run or across runs.

| Parameter | Type | Notes |
|---|---|---|
| `key` | string | **required.** Variable name. |
| `value` | any | Value to store. Accepts any JSON-serializable value. |
| `persist` | boolean | If `true`, write to the database so the value survives after the run ends. Default `false`. |

Variables without `persist: true` exist only for the duration of the current run. They're useful for passing computed values between nodes that aren't directly connected.

`persist: true` writes to the workflow database (`workflows.db`). The value is available to Get Variable on the next run and every run after that until overwritten. In server serve mode (single-workflow daemon), `persist: true` is a no-op with a warning — use API mode for cross-run persistence on the server.

### Get Variable

`type_id: get_variable`

Retrieves a variable previously set by a Set Variable node.

| Parameter | Type | Notes |
|---|---|---|
| `key` | string | **required.** Variable name to retrieve. |
| `persist` | boolean | If `true`, fall back to the database when the variable isn't found in the current run. Default `false`. |

Output: `{ key: string, value: any, found: boolean }`

If the variable is not found, `value` is `null` and `found` is `false`. The node does not fail — execution continues on the normal output port.

### Output

`type_id: output`

Marks data as the workflow's final result. The output is displayed in a dedicated viewer directly on the canvas, without requiring the run drawer to be open.

| Parameter | Type | Notes |
|---|---|---|
| `label` | string | Label shown above the result. Default `Result`. |
| `source_node` | string | Node ID to display output from. Uses the incoming data if omitted. |
| `field` | string | Specific field to extract from the source, e.g. `content` or `body.text`. Shows everything if blank. |

Output: `{ value: any, label: string, type: string }`

If the value is a media contract (files array with `filename`, `data`, and `mime_type` fields), the frontend renders an image gallery instead of raw JSON.

---

## Running a single node

Right-click any node on the canvas and choose **Run from here**. Aerini builds a subgraph containing that node and all its upstream dependencies, then runs only that portion. Downstream nodes are not touched.

This is the fastest way to test a single node's configuration without triggering downstream side effects like sending emails or posting to Slack.
