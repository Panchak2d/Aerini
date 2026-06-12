# Upgrading Flowo

## How upgrades work

When you open a workflow saved by an older version of Flowo, the app automatically updates the workflow format to the current version. This happens in memory — the file on disk isn't changed until you explicitly save the workflow.

Upgrades are incremental: a workflow from any supported older version is stepped through each intermediate format until it reaches the current one.

---

## Before you upgrade

Back up your data directory first. If something goes wrong, you can restore from here.

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/com.flowo.app/` |
| Windows | `%APPDATA%\com.flowo.app\` |
| Linux | `~/.local/share/com.flowo.app/` |

For `flowo-server`, back up `FLOWO_DATA_DIR` (default: `~/.local/share/flowo/`).

---

## Upgrading the server binary

Stop the service before replacing the binary. The server applies any pending database migrations automatically on startup — you don't need to run them manually.

```bash
sudo systemctl stop flowo-server
sudo cp new-flowo-server /usr/local/bin/flowo-server
sudo systemctl start flowo-server
```

**Downgrading is not supported** after a schema migration has run. If you need to roll back, restore from the backup you made before upgrading.

---

## If a workflow was created by a newer version

Flowo shows an error rather than partially loading a workflow it doesn't recognize:

```
Schema migration error: no migration path from schema_version '1.2' to '1.0'.
This workflow was created by a newer version of Flowo. Upgrade Flowo to load it.
```

The fix is to upgrade Flowo to at least the version that created the file. Silently loading a newer format in an older build risks data corruption.

---

## Migration history

| Schema version | Flowo version | What changed |
|---|---|---|
| 1.0 | 0.1.0 – current | Initial version. No migrations yet. |

This table is updated with every release that changes the workflow format.
