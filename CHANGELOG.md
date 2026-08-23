# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) — versioning: [SemVer](https://semver.org/).

---

## [Unreleased]

### Added
- Multi-node plugin packages (`.aerinipkg`): bundle several plugin nodes into one signed, installable unit. The Plugins tab install picker and drag-drop both accept `.aerinipkg` alongside single `.wasm` files, packs are grouped in the plugin list with their own "Remove pack" action, and `docs/plugin-authoring.md` documents the package format

### Planned (v0.4)
- Workflow versioning and history
- Improved run history UI
- More trigger types (file watch, database poll)
- Export / import workflow bundles

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
