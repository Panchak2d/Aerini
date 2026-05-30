# Workflow Schema Migrations

How Flowo handles workflow format changes across versions.

## Overview

Every workflow — whether stored in the local database, exported as a `.flowo` file, or deployed via `Export for Server` — carries a `schema_version` field. When the workflow JSON format needs to change in a way that would break old files, Flowo increments this version and ships a migration function that automatically upgrades old workflows on load.

You never run migrations manually. They happen silently the first time a workflow is loaded by a newer Flowo version.

## Current version

```
"1.0"
```

No migrations exist yet. The framework shipped in Flowo 0.2.x ready for future schema changes.

## How loading works

`Workflow::from_json()` does the following on every load:

1. Parse the raw JSON into an untyped value tree.
2. Read `schema_version` from the root. If missing, treat it as `"1.0"`.
3. If `schema_version` matches the current version — skip migration, go to step 4.
4. Find the migration function for that version. Apply it. Update `schema_version`. Repeat until current.
5. Deserialize the (now-current) JSON tree into a `Workflow` struct.

This means an old workflow file is silently upgraded in memory every time it loads. The on-disk copy is only updated when you explicitly save the workflow.

## What happens if no migration path exists

If a workflow carries a `schema_version` that this build of Flowo does not recognise — typically because it was created by a **newer** version — loading will fail with:

```
Schema migration error: no migration path from schema_version '1.2' to '1.0'.
This workflow was created by a newer version of Flowo. Upgrade Flowo to load it.
```

This is intentional. Silently loading a newer workflow in an older build risks data loss. The correct fix is to upgrade Flowo.

## Compatibility guarantees

| Scenario | Result |
|---|---|
| Load v1.0 workflow in v1.0 Flowo | OK — no migration |
| Load v1.0 workflow in v1.1 Flowo | OK — migration applied automatically |
| Load v1.1 workflow in v1.0 Flowo | Error — no downgrade path |
| Load workflow with missing `schema_version` | OK — treated as v1.0 |

## For developers: adding a migration

When a pull request changes the workflow JSON format in a breaking way:

### Step 1 — Increment `CURRENT_VERSION`

In `flowo-engine/src/migration.rs`:

```rust
pub const CURRENT_VERSION: &str = "1.1";  // was "1.0"
```

### Step 2 — Write the migration function

```rust
fn migrate_1_0_to_1_1(raw: &mut serde_json::Value) {
    // `raw` is the entire workflow JSON object.
    // Mutate it in-place to match the new format.
    //
    // Example: rename `config.timeout` to `config.timeout_ms` and multiply by 1000.
    if let Some(nodes) = raw.get_mut("nodes").and_then(|n| n.as_array_mut()) {
        for node in nodes {
            if let Some(config) = node.get_mut("config").and_then(|c| c.as_object_mut()) {
                if let Some(old_val) = config.remove("timeout") {
                    let ms = old_val.as_f64().unwrap_or(30.0) * 1000.0;
                    config.insert("timeout_ms".to_string(), serde_json::json!(ms as u64));
                }
            }
        }
    }
}
```

Rules for migration functions:

- **Pure and deterministic.** Same input → same output. No I/O, no randomness.
- **Never silently drop data.** If you must remove a field, document why in the function body.
- **Only widen, never narrow.** Migrations run against user data. A bug here corrupts every workflow on load. Review carefully.
- **No panics.** Every `unwrap()` is a potential corruption path. Use `?` or `unwrap_or`.

### Step 3 — Register it in `MigrationEngine::new()`

```rust
pub fn new() -> Self {
    Self {
        migrations: vec![
            Migration { from: "1.0", to: "1.1", apply: migrate_1_0_to_1_1 },
        ],
    }
}
```

If you're adding a second migration on top of a previous one:

```rust
migrations: vec![
    Migration { from: "1.0", to: "1.1", apply: migrate_1_0_to_1_1 },
    Migration { from: "1.1", to: "1.2", apply: migrate_1_1_to_1_2 },
],
```

The chain must be contiguous. If there is a gap (e.g. you register `1.0 → 1.2` without `1.1 → 1.2`), workflows at `"1.1"` will fail to load.

### Step 4 — Write a test

In `migration.rs`, add a test that:

1. Constructs a JSON object representing a v1.0 workflow (the old format).
2. Runs the migration engine.
3. Asserts the result matches the v1.1 format exactly.

Do not ship a migration without a test.

### Step 5 — Update the default in `model.rs`

```rust
fn default_schema_version() -> String { "1.1".to_string() }

// and in Workflow::new():
schema_version: "1.1".to_string(),
```

New workflows created after the release will carry the new version. Old workflows load via migration.

## Backup guidance

Because migration happens in memory on load and is only written to disk when you save, an existing database or `.flowo` export is not automatically overwritten. However:

- If you run an older Flowo build after migration has updated an in-memory workflow and saved it, the file will now contain the new `schema_version` and the older build won't be able to load it.

**Recommendation:** before upgrading Flowo in a production deployment, back up:
- `~/.local/share/com.flowo.app/workflows.db` (Linux desktop)
- `~/Library/Application Support/com.flowo.app/workflows.db` (macOS desktop)
- The data directory specified by `--data-dir` (server deployments)
- Any `.flowo` export files you care about

This is especially important if you run both a desktop instance and a server instance against different Flowo versions.
