# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) — versioning: [SemVer](https://semver.org/).

---

## [Unreleased]

### Added
- Chat panel: files attached to a sent message now appear as filename chips in that message, and are saved with the session, so they are still shown after switching sessions or restarting the app. Adds a nullable `attachments_json` column to the local chat-message table (local database version 9, unrelated to a workflow file's `schema_version`; existing messages are unaffected); see `docs/guide/chat-panel.md#attachments`
- Webhook: a request body with an `attachments` array (as the desktop Chat panel sends when files are attached) is now also exposed as a top-level `files` output, so `{{Webhook.output.files}}` can be wired to a Files input such as AI Prompt's; `body.attachments` is unchanged, and `files` is omitted when the body has no `attachments` array. Applies to both ad-hoc Run and background (scheduler-driven) runs; see `docs/guide/nodes.md#webhook`
- Chat panel: you can send attachments without any text (the message is posted to the webhook as an empty string), and clicking a sent file chip opens an image full-size or downloads any other file; see `docs/guide/chat-panel.md#attachments`
- Execution Transcript: clicking a log line that's tied to a specific node now selects that node on the canvas, the same way clicking a run-history entry does; keyboard-accessible via Enter/Space when the line is focused
- `aerini-server`: a prebuilt container image is now published to GHCR (`ghcr.io/panchak2d/aerini-server`) on every tagged release, tagged with the release version and `latest`; `docker pull` replaces cloning the repo and building the image yourself as the fastest way to get a working binary. See [Server Deployment](docs/operations/server-deploy.md#getting-the-aerini-server-binary)

### Changed
- Building from source now requires Rust 1.96 or later (was 1.94.1 for `aerini-server`, 1.95 for the desktop app), set by Wasmtime 49
- **Breaking** for an existing desktop install: the app identifier changed from `com.aerini.app` to `org.aerini.desktop` (the old one ended in `.app`, which conflicts with the macOS bundle extension), so the app's data folder moves to a new location, listed in [Installation](docs/getting-started/installation.md). A copy already on your machine starts empty; to keep your workflows and settings, quit Aerini and move the contents of the old folder into the new one. The credential encryption key lives in the OS keychain under a fixed name, so moved credentials still decrypt
- **Breaking** for pages that call `POST /api/widget/:workflow_id/mint-token` from browser JavaScript (`aerini-server`, `api`): the server no longer allows any origin, including localhost and `--allow-origin` ones, cross-origin access to that endpoint, so the browser blocks the call. It was already documented as server-to-server only; call it from your backend, which sends no `Origin` and is unaffected. See [Embeddable Chat Widget](docs/guide/widget-embedding.md)
- `aerini-server` (`api`): CORS now exposes `Retry-After` to browsers, so a client on an allowed origin can read it from a `429`, and preflight answers are cacheable for an hour, which cuts repeat `OPTIONS` requests. The embedded chat widget shows the retry delay when it is rate-limited.
- **Breaking** for tokens with workflow grants (`aerini-server`, `api`): the per-token workflow ACL now confines a token on every workflow route, not only the scheduler, memory, performance and event routes. A restricted token gets `403` on `GET|DELETE /api/workflows/:id`, `POST /api/workflows/:id/run` and `POST /api/workflows` for a workflow outside its grants (`POST` cannot create a new workflow either), sees only its granted workflows in `GET /api/workflows`, and is refused on `/api/credentials`. Admin tokens and tokens with no grants are unchanged. Give such a token the grants it needs, or use an unrestricted token; see [Per-token workflow ACL](docs/operations/server-api-reference.md#per-token-workflow-acl)
- `aerini-server` (`api`): `DELETE /api/tokens/:id/workflows/:workflow_id` says in its response when the revoked grant was the token's last, since a token with no grants is unrestricted. The grant and list responses' `note` text no longer describes the ACL as covering only SSE events.
- `aerini-server` (`api`): rate-limit `429` responses now carry a JSON `{"error": ...}` body and a `Retry-After: 60` header instead of an empty body.
- Chat panel: restyled for legibility and accessibility. The "not running" banner now has a visible amber background and left edge, and its **Start** button no longer inherits the full-width primary-button style (which could squeeze the banner text); your messages and the Send button use a darker blue so white text meets 4.5:1 contrast on the dark theme; the message box and attach button have outlines that stand out against the panel, and the attach button's disabled state is more legible; Send, attach, and **Start** are at least 44 px tall, and the message box shows a keyboard focus ring; the message box and icon-only buttons are labelled for screen readers; the panel's slide-in and typing animations are skipped when the OS reduce-motion setting is on
- Chat panel: the "not running" banner and the empty-conversation notice are now one notice, and its wording says "press Start" instead of "use Start below". Filled primary buttons (Start, the onboarding example button, and similar) now use white text on the Paper theme, where the previous black text was below 4.5:1 contrast; the dark theme is unchanged. The session menu's delete control is now a real button reachable by keyboard, and small in-message controls (Copy, image overlay buttons, chip remove) have larger touch targets
- Chat panel: history is saved incrementally — sending a message writes only that message instead of the whole conversation (including image and attachment data), and only the open conversation is loaded when the panel opens. Large histories open faster, and switching to another session loads it on demand, briefly showing "Loading conversation…"; if that load fails, you stay in the current session.
- Run History: a run whose Webhook trigger received chat attachments stores the attachment data once instead of twice, so those runs take roughly half the space. Runs read back unchanged (including Replay). An older build opening a database written by this one shows a `{"$same_as": "files"}` placeholder in `body.attachments` of such runs

### Fixed
- AI Prompt and AI Agent: a request whose prompt (or goal) resolves to empty but that has a readable attachment (for example a Chat message with only a file) no longer fails with "prompt field is required"; a default instruction is sent in place of the text. A prompt with no text and no readable attachment still fails as before; see `docs/guide/nodes.md`
- Chat panel: sending files when no node reads the Webhook's `files` output now shows a notice, since the model otherwise receives only the message text; see `docs/guide/chat-panel.md#attachments`
- Shell Command and Code (JS) nodes: on timeout, or when the run is cancelled or its workflow-level timeout fires, the command and everything it started (pipelines, background jobs, child processes) are now killed on macOS and Linux, where before only the command's own process was; such processes could keep running after the run ended. The command now runs in its own process group. On Windows only the command's own process is killed, as before
- Shell Command and Code (JS) nodes: `timeout_secs` now also covers reading the command's output. A command that left a background process holding its output open (for example `sleep 300 &` with no redirect) used to keep the node waiting past the timeout until that process exited; it now times out and the process is killed
- Database node: a running SQLite statement is now interrupted, and a running Postgres or MySQL `Execute` statement (in runs that support cancellation, as API and desktop runs do) is cancelled on the server, when the run is cancelled or times out, instead of running to completion. The MySQL cancel watcher no longer stays alive after a run that times out
- `aerini-server` (`api`): token lookups, ACL lookups, credential operations and the scheduler stop on workflow delete no longer block the async runtime, so slow database access no longer stalls other requests.
- `aerini-server` (`api`): shutdown no longer waits indefinitely for open `/api/events` streams. It ends them as soon as the signal arrives and closes any remaining connections after 10 seconds, then drains in-flight workflow runs as before; previously `docker stop` or `systemctl stop` could end in SIGKILL before that drain ran. Clients see the stream end, and events emitted while disconnected are not replayed on reconnect; see [Events (SSE)](docs/operations/server-api-reference.md#events-sse)
- `aerini-server` (`api`): the global rate-limit `429` now carries CORS headers, so browser clients on an allowed origin can read it instead of seeing an opaque network error; and the rate limiters no longer risk a panic within the first minute after a Windows or macOS boot.
- `aerini-server` (`api`): `GET /api/workflows/:id` returns `500` if the workflow cannot be serialized instead of `200` with an empty body; `POST /api/tokens` returns `400` for an `expires_in_secs` too large to represent instead of panicking; the `Bearer` scheme in `Authorization` is now matched case-insensitively.

### Security
- `aerini-server` (`api`): a token restricted by the per-token workflow ACL could still list, read, delete, run, create or overwrite any workflow, and use the server-wide credential routes, limited only by its `read`/`write` scope. Those routes now enforce the ACL; see Changed above.
- `aerini-server`: `--trusted-proxy-count` picked the wrong `X-Forwarded-For` entry, one position too far left, so a client could put its own address at the front of the header and have that used as its IP by the per-IP rate limits (including the widget trigger and `mint-token` limits) and the status page. The Nth entry from the right is now used, as each trusted proxy appends its peer's address. Deployments that already matched their proxy count to their topology see no change for well-formed requests.
- `aerini-server`: a token whose stored expiry cannot be parsed is now rejected instead of never expiring.
- Debug tab: the node id fragment shown in each log line is now HTML-escaped before being inserted into the panel, matching the message text next to it. Verified exploitable: node ids are taken as-is from imported workflow JSON with no validation, so a crafted `id` value could inject markup into the Debug tab once a log entry referenced that node
- Embeddable chat widget (`aerini-widget.js`): session-id generation now tries `crypto.getRandomValues()` before falling back to `Math.random()` for the last `data-session-id` (CWE-338) — closes the gap between `crypto.randomUUID()`, tried first, and the previous direct-to-`Math.random()` fallback. The session id is stored in `localStorage` and sent to the target workflow's Webhook trigger as `session_id`, where it commonly keys per-visitor state (e.g. an AI Memory node, as in `examples/Chatbot + AI Memory + Widget.md`); a `Math.random()`-derived id is predictable enough that another visitor could guess it and inject messages into someone else's conversation thread on a browser/runtime that lacks `crypto.randomUUID` but still has the older, more widely supported `crypto.getRandomValues`

### Planned (v0.5)
- Sub-workflows
- More trigger types (file watch, database poll)
- Workflow bundle import (restore from an `aerini-backup-*.zip`)

---

## [0.4.1] - 2026-09-20

### Added
- Keyboard shortcuts are now discoverable from the UI: a **Shortcuts** button next to Settings opens the same list previously only reachable by pressing `?`
- Plugin nodes: `node-input.credentials` is now populated with resolved credential values (previously always empty) — the same values already available via `params`, now also structured and keyed by config field name; see `docs/development/plugin-authoring.md`
- AI Prompt and AI Agent: new `local` provider for Ollama, LM Studio, vLLM, llama.cpp, and any other OpenAI-compatible local server; see `docs/guide/local-models.md`. `provider: auto` now detects loopback, private-network, and `localhost` base URLs as `local` automatically, and a blank Model with `local` selected (explicitly or via `auto`) now returns a `MISSING_MODEL` error instead of silently falling back to `gpt-5.6`. Saved credentials no longer require a Secret Value when Advanced Provider is set to `local`
- New `list_provider_models` command discovers a provider's available models (`openai`, `anthropic`, `gemini`, `local`) by querying its `/models` endpoint; see `docs/development/desktop-ipc-reference.md`
- AI Prompt and AI Agent: a **Fetch Models** button on the Model field calls the new model-discovery command and offers a dropdown of what's actually available, including for `local` servers and `provider: auto` once it resolves to a known provider; typing a model name directly always still works
- Fetch Models failures (missing/invalid API key, blocked or unreachable server, etc.) now show the specific reason instead of a generic "Couldn't fetch"; applies to both the node popover and the Credentials panel
- AI Prompt, AI Agent, and the Credentials panel: Base URL now offers a one-click "Use Ollama defaults" fill once Provider is `local`, plus a matching placeholder — `local` still has no cloud default (any OpenAI-compatible server, any port), this just covers the common case; see `docs/guide/local-models.md`
- Config fields can opt into a native file/folder picker via the `x-aerini-path-picker` schema flag (`"file"`, `"directory"`, or `"file-or-directory"`) — adds a picker button alongside the existing free-text input; any built-in or plugin node can set it, no host-side wiring needed per node type
- Array-typed config fields (fixed `items.enum`, or free-form strings) now render generically — a checkbox group for a fixed option set, an editable chip list otherwise — instead of needing a bespoke node-configs extension; array-of-object fields are unaffected
- Plugin nodes: the "P" identity badge already shown in the palette and search results now also appears on the canvas node card, the config panel header, and the node-info hover tooltip — previously those three surfaces had no plugin indicator at all; see `docs/development/plugin-authoring.md`

### Changed
- Plugins: choosing a plugin folder from the **Plugins** tab now loads the plugins already in it immediately, instead of requiring a separate **Reload Plugins** click before they appear in the node panel

### Fixed
- Export for Server → Docker: the generated Dockerfile now downloads, checksum-verifies, and bundles the same pinned Node.js runtime the repository's own root Dockerfile uses, placed next to `aerini-server` as `node-bundled`. A workflow with a Code (JS) node in the exported Docker package now runs it correctly, the same way it already did in the Linux Server export and the official Docker image; previously the generated image had no Node runtime at all, so that node failed every run even with `--allow-code` set
- AI Prompt, AI Agent, and Image Generation: the **Use Saved Credential** dropdown is now filtered to credentials whose Advanced Provider matches the node's currently selected Provider, plus any credential with no Provider set — previously it listed every saved credential regardless of provider, so a mismatched one could be attached silently and only fail with a provider auth error at run time; see `docs/guide/credentials.md`
- Plugin nodes: config properties named `files`, `sources`, `subfolders`, `folder_path`, `overwrite`, or `attachments` now get an ordinary field in the config panel — they were previously left out entirely, because those names were reserved for built-in nodes' custom controls; that exclusion now applies only to the built-in nodes that own those controls; see `docs/development/plugin-authoring.md`
- Plugin nodes: any config value that is an object, or an array the form field can't hold — declared as `type: "object"`, as an array of non-string items, or as an array with no `items` declared (for example file objects) — is now edited as JSON in an **Advanced: full config (JSON)** section of the config panel, instead of a text or list field that showed `[object Object]` and replaced or dropped the stored value on the first edit. The same applies to a node whose plugin is no longer installed. Credential fields are left out of that JSON; see `docs/development/plugin-authoring.md`

### Security
- Code (JS) sandbox (`--code-sandbox`): blocked `v8`/`inspector` builtins and removed `process.binding()`/`process._linkedBinding()`/`process.dlopen()` outright. Verified exploitable: with only the prior blocklist in place, sandboxed code could call `process.binding('spawn_sync').spawn(...)` to run arbitrary OS commands, and `v8.writeHeapSnapshot()` to write a script-influenced file to any path — both bypass the ESM-loader import hook entirely, the same class of gap as the `module`/`process.getBuiltinModule()` fix below
- Widget relay (`POST /api/widget/:id/trigger`): now refuses with `403` to trigger any workflow containing a Shell Command, Code, or Database node, unconditionally. `--allow-shell`/`--allow-code`/`--allow-database` are server-wide, not per-workflow, so enabling one for an unrelated internal workflow previously also left that node type reachable through this route on any other, publicly-embedded workflow the same server runs
- Dependencies: `wasmtime`/`wasmtime-wasi`/`wasmtime-wasi-http` 46.0.2 → 46.0.3 (RUSTSEC-2026-0268, RUSTSEC-2026-0269 — WASI filesystem sandbox escape and guest-controlled host heap allocation); `h2` bumped off 0.4.15 (RUSTSEC-2026-0258); `chacha20` bumped off yanked 0.10.1; `aws-sdk-s3` 1.140.0 → 1.145.0 to pull in `lru` >=0.18.2 (RUSTSEC-2026-0253)
- Widget (`aerini-widget.js`): the auto-generated session id no longer falls back to `Math.random()` when the browser has no Web Crypto API (CodeQL `js/insecure-randomness`, CWE-338). That id is what keeps one visitor's replies from reaching another visitor, so a guessable one weakened that isolation; the widget now refuses to start in such a browser unless `data-session-id` is set
- Dependencies: `wasmtime`/`wasmtime-wasi`/`wasmtime-wasi-http` 46.0.3 → 49.0.1 (RUSTSEC-2026-0313, RUSTSEC-2026-0314, RUSTSEC-2026-0316, GHSA-m63x-6p34-q65x — guest-driven host memory exhaustion through an outgoing HTTP body write, a host panic from an out-of-range filesystem datetime, and fuel not charged for dynamic record lifting or for callees of `call_ref`). No 46.x release carries the fixes; only 48.0.3+, 49.0.1+ and 36.0.16 do. Plugin outbound HTTP is still SSRF-filtered by the same policy for both action and trigger plugins
- Dependencies: `rustls` 0.23.43 → 0.23.45 (RUSTSEC-2026-0285 — TLS 1.3 handshake messages accepted at the wrong encryption level); the new `rustls` requires `aws-lc-rs` >=1.18 and `rustls-webpki` >=0.103.14, so `aws-lc-rs` 1.17.3 → 1.18.1, `aws-lc-sys` 0.43.0 → 0.45.0, and `rustls-webpki` 0.103.13 → 0.103.15 move with it

---

## [0.4.0]

### Added
- Workflow Settings → Execution now has a Max duration (seconds) control, so `max_duration_secs` can be set from the desktop UI instead of only by hand-editing the workflow file or scripting the engine directly. Hidden while Unlimited Duration is on; blank means no limit; enforces the same 10–86400s range the executor already clamps to
- Multi-node plugin packages (`.aerinipkg`): bundle several plugin nodes into one signed, installable unit. The Plugins tab install picker and drag-drop both accept `.aerinipkg` alongside single `.wasm` files, packs are grouped in the plugin list with their own "Remove pack" action, and `docs/plugin-authoring.md` documents the package format
- Version History panel: named snapshots via Save Snapshot (message optional), Compare against the current canvas through a diff view, Restore, Delete, and a filter box once more than a few versions exist
- Run History panel: filter runs by All / Success / Failed, pagination, and a "Clear all" action per workflow, with relative timestamps ("2m ago", "yesterday")
- "Export All" (toolbar → Export): bundles every saved workflow into one `aerini-backup-<date>.zip` of individual `.aerini` files, so a full-library backup doesn't mean exporting one at a time

### Security
- Code (JS) sandbox (`--code-sandbox`): blocked the `module` builtin (`import('node:module')`) and overrode `process.getBuiltinModule()`, both of which could hand sandboxed code a working `createRequire()` and, through it, `child_process`/`fs`/`net`/etc. — unrestricted access the sandbox exists to block, reachable through neither of its ESM-loader-based or global-deletion mitigations

---

## [0.3.0]

### Added
- Plugin management UI (**Plugins** tab): set a plugin folder, see installed plugins (including ones that failed to load, with the error shown), install via file picker or drag-and-drop, and remove plugins
- Plugin nodes are marked with a small "P" badge in the palette and search results
- Pre-built binary releases for macOS, Windows, Linux
- Run history now records a `running` status the moment a run starts, and relabels leftover `running` entries as `interrupted` on next startup if Aerini was closed mid-run
- `aerini-server` drains in-progress runs on `SIGTERM` (`systemctl stop`/`restart`) instead of cutting them off, bounded by `--max-workflow-duration-secs` (default 30s)
- Export for Server now lists any `$vars.*` variables your workflow needs as a separate "Required variables" section (`AERINI_VAR_*`), alongside the existing credential variables (`AERINI_CRED_*`)

---

## [0.2.0]

### Added
- Parallel execution (per-workflow opt-in via `parallel_execution: true` in workflow JSON)
- Global concurrent run limit (`--max-concurrent-runs` server flag)
- Per-workflow SSE ACL (`token_workflow_acl` table, `/api/tokens/:id/workflows` route)
- Code node sandbox (`--code-sandbox` — ESM module import blocking for `fs`, `net`, `child_process`; Linux adds `setrlimit` CPU/memory caps)
