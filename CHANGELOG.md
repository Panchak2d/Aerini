# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) — versioning: [SemVer](https://semver.org/).

---

## [Unreleased]

### Security
- Widget relay (`POST /api/widget/:id/trigger`): now refuses with `403` to trigger any workflow containing a Shell Command, Code, or Database node, unconditionally. `--allow-shell`/`--allow-code`/`--allow-database` are server-wide, not per-workflow, so enabling one for an unrelated internal workflow previously also left that node type reachable through this route on any other, publicly-embedded workflow the same server runs

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
