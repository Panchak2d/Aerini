# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) — versioning: [SemVer](https://semver.org/).

---

## [Unreleased]

### In progress
- Pre-built binary releases for macOS, Windows, Linux

### Planned (v0.3)
- Plugin / custom node API
- Workflow versioning and history
- Improved run history UI
- More trigger types (file watch, database poll)
- Export / import workflow bundles

---

## [0.2.0]

### Added
- Parallel execution (per-workflow opt-in via `parallel_execution: true` in workflow JSON)
- Global concurrent run limit (`--max-concurrent-runs` server flag)
- Per-workflow SSE ACL (`token_workflow_acl` table, `/api/tokens/:id/workflows` route)
- Code node sandbox (`--code-sandbox` — ESM module import blocking for `fs`, `net`, `child_process`; Linux adds `setrlimit` CPU/memory caps)
