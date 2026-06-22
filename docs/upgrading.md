# Upgrading Aerini

## How upgrades work

When you open a workflow saved by an older version of Aerini, the app automatically updates the workflow format to the current version. This happens in memory — the file on disk isn't changed until you explicitly save the workflow.

Upgrades are incremental: a workflow from any supported older version is stepped through each intermediate format until it reaches the current one.

---

## Before you upgrade

Back up your data directory first. If something goes wrong, you can restore from here.

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/com.aerini.app/` |
| Windows | `%APPDATA%\com.aerini.app\` |
| Linux | `~/.local/share/com.aerini.app/` |

For `aerini-server`, back up `AERINI_DATA_DIR` (default: `~/.local/share/aerini/`).

---

## Upgrading the server binary

Stop the service before replacing the binary. The server applies any pending database migrations automatically on startup — you don't need to run them manually.

```bash
sudo systemctl stop aerini-server
sudo cp new-aerini-server /usr/local/bin/aerini-server
sudo systemctl start aerini-server
```

**Downgrading is not supported** after a schema migration has run. If you need to roll back, restore from the backup you made before upgrading.

---

## If a workflow was created by a newer version

Aerini shows an error rather than partially loading a workflow it doesn't recognize:

```
Schema migration error: no migration path from schema_version '1.2' to '1.0'.
This workflow was created by a newer version of Aerini. Upgrade Aerini to load it.
```

The fix is to upgrade Aerini to at least the version that created the file. Silently loading a newer format in an older build risks data corruption.

---

## Migration history

| Schema version | Aerini version | What changed |
|---|---|---|
| 1.0 | 0.1.0 – current | Initial version. No migrations yet. |

This table is updated with every release that changes the workflow format.
