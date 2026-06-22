# Workflow Schema Migrations

Every workflow file carries a `schema_version` field. When Aerini needs to change the workflow JSON format in a way that would break older files, it increments this version and ships a migration function. Old workflows are upgraded automatically the first time they're loaded by a newer Aerini version.

You never run migrations manually.

---

## Current version

```
"1.0"
```

No migrations exist yet. The framework shipped in Aerini 0.2.x ready for future schema changes.

---

## How loading works

`Workflow::from_json()` runs this sequence on every load:

1. Parse the raw JSON.
2. Read `schema_version` from the root. If the field is missing, treat it as `"1.0"`.
3. If `schema_version` already matches the current version, skip to step 5.
4. Look up the migration function for that version, apply it, update `schema_version`, and repeat until the current version is reached.
5. Deserialize the (now current) JSON into a `Workflow` struct.

The on-disk file is not modified. The migration runs in memory. The updated format is written to disk only when you explicitly save the workflow.

---

## Compatibility matrix

| Scenario | Result |
|---|---|
| Load v1.0 workflow in v1.0 Aerini | OK — no migration needed |
| Load v1.0 workflow in v1.1 Aerini | OK — migration applied in memory |
| Load v1.1 workflow in v1.0 Aerini | Error — no downgrade path exists |
| Load workflow with missing `schema_version` | OK — treated as v1.0 |

If a workflow carries a `schema_version` this build of Aerini doesn't recognize (because it was created by a newer version), loading fails with:

```
Schema migration error: no migration path from schema_version '1.2' to '1.0'.
This workflow was created by a newer version of Aerini. Upgrade Aerini to load it.
```

This is intentional. Silently loading a newer format in an older build risks data corruption.

---

## Adding a migration (for contributors)

When a pull request changes the workflow JSON format in a breaking way, follow these five steps.

### Step 1 — Increment `CURRENT_VERSION`

In `aerini-engine/src/migration.rs`:

```rust
pub const CURRENT_VERSION: &str = "1.1";  // was "1.0"
```

### Step 2 — Write the migration function

The function receives the entire workflow JSON as a mutable `serde_json::Value` and transforms it in-place:

```rust
fn migrate_1_0_to_1_1(raw: &mut serde_json::Value) {
    // Example: rename `config.timeout` to `config.timeout_ms` and convert seconds to ms.
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

Rules migration functions must follow:

- **Pure and deterministic.** Same input always produces the same output. No I/O, no randomness.
- **Never silently drop data.** If a field must be removed, document why in a comment.
- **Only widen, never narrow.** Migrations run against real user data. A bug here corrupts every affected workflow on load.
- **No panics.** Every `unwrap()` is a potential data corruption path. Use `?` or `unwrap_or` throughout.

### Step 3 — Register the migration

In `MigrationEngine::new()`:

```rust
pub fn new() -> Self {
    Self {
        migrations: vec![
            Migration { from: "1.0", to: "1.1", apply: migrate_1_0_to_1_1 },
        ],
    }
}
```

If a second migration is added later, append it to the chain:

```rust
migrations: vec![
    Migration { from: "1.0", to: "1.1", apply: migrate_1_0_to_1_1 },
    Migration { from: "1.1", to: "1.2", apply: migrate_1_1_to_1_2 },
],
```

The chain must be contiguous. A gap — registering `1.0 → 1.2` without a `1.1 → 1.2` step — means workflows at version `1.1` fail to load.

### Step 4 — Write a test

In `migration.rs`, add a test that constructs a JSON object in the old format, runs it through the migration engine, and asserts the result exactly matches the new format. Do not merge a migration without a test.

### Step 5 — Update the default version in `model.rs`

```rust
fn default_schema_version() -> String { "1.1".to_string() }

// and in Workflow::new():
schema_version: "1.1".to_string(),
```

New workflows created after the release will carry the new version. Existing workflows migrate on first load.

---

## Backup guidance

Because migrations run in memory and write to disk only on save, your existing database isn't automatically modified. However, once you open a workflow in a newer Aerini version and save it, the file carries the new `schema_version` — an older Aerini build can no longer open it.

Before upgrading Aerini in any production setup, back up:

- **Desktop (Linux):** `~/.local/share/com.aerini.app/workflows.db`
- **Desktop (macOS):** `~/Library/Application Support/com.aerini.app/workflows.db`
- **Server:** the `--data-dir` directory (default: `~/.aerini-server/`)
- Any exported `.aerini` files you care about

This matters especially if you run both a desktop instance and a server on different Aerini versions.
