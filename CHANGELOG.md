# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) — versioning: [SemVer](https://semver.org/).

---

## [Unreleased]

### Added
- Plugin nodes: `node-input.credentials` is now populated with resolved credential values (previously always empty) — the same values already available via `params`, now also structured and keyed by config field name; see `docs/development/plugin-authoring.md`
- AI Prompt and AI Agent: new `local` provider for Ollama, LM Studio, vLLM, llama.cpp, and any other OpenAI-compatible local server; see `docs/guide/local-models.md`. `provider: auto` now detects loopback, private-network, and `localhost` base URLs as `local` automatically, and a blank Model with `local` selected (explicitly or via `auto`) now returns a `MISSING_MODEL` error instead of silently falling back to `gpt-5.6`. Saved credentials no longer require a Secret Value when Advanced Provider is set to `local`
- New `list_provider_models` command discovers a provider's available models (`openai`, `anthropic`, `gemini`, `local`) by querying its `/models` endpoint; see `docs/development/desktop-ipc-reference.md`
- AI Prompt and AI Agent: a **Fetch Models** button on the Model field calls the new model-discovery command and offers a dropdown of what's actually available, including for `local` servers and `provider: auto` once it resolves to a known provider; typing a model name directly always still works
- AI Prompt, AI Agent, and Image Generation: the **Use Saved Credential** dropdown is now filtered to credentials whose Advanced Provider matches the node's currently selected Provider, plus any credential with no Provider set — previously it listed every saved credential regardless of provider, so a mismatched one could be attached silently and only fail with a provider auth error at run time; see `docs/guide/credentials.md`

### Security
- Code (JS) sandbox (`--code-sandbox`): blocked `v8`/`inspector` builtins and removed `process.binding()`/`process._linkedBinding()`/`process.dlopen()` outright. Verified exploitable: with only the prior blocklist in place, sandboxed code could call `process.binding('spawn_sync').spawn(...)` to run arbitrary OS commands, and `v8.writeHeapSnapshot()` to write a script-influenced file to any path — both bypass the ESM-loader import hook entirely, the same class of gap as the `module`/`process.getBuiltinModule()` fix below
- Widget relay (`POST /api/widget/:id/trigger`): now refuses with `403` to trigger any workflow containing a Shell Command, Code, or Database node, unconditionally. `--allow-shell`/`--allow-code`/`--allow-database` are server-wide, not per-workflow, so enabling one for an unrelated internal workflow previously also left that node type reachable through this route on any other, publicly-embedded workflow the same server runs
- Dependencies: `wasmtime`/`wasmtime-wasi`/`wasmtime-wasi-http` 46.0.2 → 46.0.3 (RUSTSEC-2026-0268, RUSTSEC-2026-0269 — WASI filesystem sandbox escape and guest-controlled host heap allocation); `h2` bumped off 0.4.15 (RUSTSEC-2026-0258); `chacha20` bumped off yanked 0.10.1; `aws-sdk-s3` 1.140.0 → 1.145.0 to pull in `lru` >=0.18.2 (RUSTSEC-2026-0253)

### Planned (v0.5)
- Sub-workflows
- More trigger types (file watch, database poll)
- Workflow bundle import (restore from an `aerini-backup-*.zip`)

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
- Plugin management UI (Settings → Plugins): set a plugin folder, see installed plugins (including ones that failed to load, with the error shown), install via file picker or drag-and-drop, and remove plugins
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
