# Upgrading Flowo

## How schema migrations work

When you open a workflow saved by an older version of Flowo, the migration engine
updates it automatically to the current schema. Your original file is not modified
until you explicitly save the workflow.

Migrations are incremental — a file from any supported older version upgrades
through each intermediate step in sequence.

---

## Before upgrading (recommended)

Back up your data directory:

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/com.flowo.app/` |
| Windows | `%APPDATA%\com.flowo.app\` |
| Linux | `~/.local/share/com.flowo.app/` |

For `flowo-server`, back up `FLOWO_DATA_DIR` (default: `~/.local/share/flowo/`).

---

## If a workflow was created by a newer version of Flowo

Flowo will show an error rather than partially loading a workflow it doesn't
understand. Upgrade Flowo to at least the version that created the file.

---

## Migration history

| Schema version | Flowo version | What changed |
|---------------|---------------|--------------|
| 1.0 | 0.1.0 – current | Initial version. No migrations yet. |

This table is updated with every release that introduces a schema change.

---

## For server deployments

Stop the server before upgrading the binary. The server applies any pending
database migrations automatically on startup. Downgrading to a previous binary
version after a schema migration has run is not supported — restore from backup
if you need to roll back.
