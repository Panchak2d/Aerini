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
- Chat panel: restyled for legibility and accessibility. The "not running" banner now has a visible amber background and left edge, and its **Start** button no longer inherits the full-width primary-button style (which could squeeze the banner text); your messages and the Send button use a darker blue so white text meets 4.5:1 contrast on the dark theme; the message box and attach button have outlines that stand out against the panel, and the attach button's disabled state is more legible; Send, attach, and **Start** are at least 44 px tall, and the message box shows a keyboard focus ring; the message box and icon-only buttons are labelled for screen readers; the panel's slide-in and typing animations are skipped when the OS reduce-motion setting is on
- Chat panel: the "not running" banner and the empty-conversation notice are now one notice, and its wording says "press Start" instead of "use Start below". Filled primary buttons (Start, the onboarding example button, and similar) now use white text on the Paper theme, where the previous black text was below 4.5:1 contrast; the dark theme is unchanged. The session menu's delete control is now a real button reachable by keyboard, and small in-message controls (Copy, image overlay buttons, chip remove) have larger touch targets
- Chat panel: history is saved incrementally — sending a message writes only that message instead of the whole conversation (including image and attachment data), and only the open conversation is loaded when the panel opens. Large histories open faster, and switching to another session loads it on demand, briefly showing "Loading conversation…"; if that load fails, you stay in the current session.
- Run History: a run whose Webhook trigger received chat attachments stores the attachment data once instead of twice, so those runs take roughly half the space. Runs read back unchanged (including Replay). An older build opening a database written by this one shows a `{"$same_as": "files"}` placeholder in `body.attachments` of such runs

### Security
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
