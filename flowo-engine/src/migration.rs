//! Workflow JSON schema migration engine.
//!
//! Every `.flowo` file and workflow stored in the database carries a
//! `schema_version` string. When the workflow JSON format changes between
//! Flowo releases, `MigrationEngine::apply()` upgrades the raw JSON in-place
//! before it is deserialized into [`crate::model::Workflow`].
//!
//! # Adding a migration
//!
//! When you need to make a breaking change to the workflow JSON format:
//!
//! 1. Increment [`CURRENT_VERSION`] (e.g. `"1.0"` → `"1.1"`).
//! 2. Write a migration function:
//!    ```rust,ignore
//!    fn migrate_1_0_to_1_1(raw: &mut serde_json::Value) {
//!        // Mutate `raw` in-place.
//!        // Example: rename a field across all nodes.
//!        if let Some(nodes) = raw.get_mut("nodes").and_then(|n| n.as_array_mut()) {
//!            for node in nodes {
//!                if let Some(config) = node.get_mut("config") {
//!                    if let Some(old_val) = config.get("old_field_name").cloned() {
//!                        config["new_field_name"] = old_val;
//!                        config.as_object_mut().unwrap().remove("old_field_name");
//!                    }
//!                }
//!            }
//!        }
//!    }
//!    ```
//! 3. Register it in [`MigrationEngine::new()`]:
//!    ```rust,ignore
//!    migrations: vec![
//!        Migration { from: "1.0", to: "1.1", apply: migrate_1_0_to_1_1 },
//!    ],
//!    ```
//!
//! # Invariants
//!
//! - Each migration function receives the full workflow JSON object and mutates it in-place.
//! - After applying a migration, `schema_version` in the JSON is updated to `to` automatically.
//! - Migration functions must be pure and deterministic — same input always produces same output.
//! - The `from` values in the migration list must form a chain from the oldest supported version
//!   up to `CURRENT_VERSION`. Gaps in the chain cause an error at load time for affected files.
//! - Migration functions must never remove data silently. Prefer renaming or converting fields.
//!   If removal is unavoidable, document it clearly in the function body.
//!
//! # What to do when `apply()` returns `Err`
//!
//! This means the workflow was created by a newer version of Flowo and no downgrade path exists.
//! The caller (`Workflow::from_json`) surfaces this as a load error. The correct response is to
//! upgrade Flowo to a version that understands the workflow's `schema_version`.

/// The schema version this build of Flowo produces and understands.
///
/// Increment this (e.g. `"1.0"` → `"1.1"`) whenever the workflow JSON format changes
/// in a way that requires a migration function to load old files correctly.
pub const CURRENT_VERSION: &str = "1.0";

type MigrateFn = fn(&mut serde_json::Value);

struct Migration {
    from:  &'static str,
    to:    &'static str,
    apply: MigrateFn,
}

/// Upgrades workflow JSON from an older `schema_version` to [`CURRENT_VERSION`].
pub struct MigrationEngine {
    migrations: Vec<Migration>,
}

impl Default for MigrationEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MigrationEngine {
    /// Build the engine with all registered migrations.
    ///
    /// Add new entries here (in version order) when introducing a schema change.
    /// The list is empty now because `CURRENT_VERSION` is still `"1.0"` — no
    /// migration is needed to reach it.
    pub fn new() -> Self {
        Self {
            migrations: vec![
                // Example of how to register when Phase 7+ bumps the schema:
                //
                // Migration { from: "1.0", to: "1.1", apply: migrate_1_0_to_1_1 },
                //
                // For now: no migrations. CURRENT_VERSION == "1.0" == every stored workflow.
            ],
        }
    }

    /// Apply all pending migrations to `raw` in-place.
    ///
    /// - If `raw["schema_version"]` == [`CURRENT_VERSION`] (or the field is absent and
    ///   defaults to `"1.0"`): returns `Ok(())` immediately — no work done.
    /// - If a chain of migrations bridges the gap: applies each step in order, updating
    ///   `raw["schema_version"]` after each step.
    /// - If no chain exists to [`CURRENT_VERSION`]: returns `Err` with a descriptive message.
    ///
    /// The caller is responsible for deserializing `raw` into [`crate::model::Workflow`]
    /// after this returns `Ok`.
    pub fn apply(&self, raw: &mut serde_json::Value) -> Result<(), String> {
        let file_version = raw
            .get("schema_version")
            .and_then(|v| v.as_str())
            .unwrap_or("1.0")
            .to_string();

        if file_version == CURRENT_VERSION {
            return Ok(());
        }

        let mut current = file_version.clone();
        loop {
            if current == CURRENT_VERSION {
                return Ok(());
            }

            match self.migrations.iter().find(|m| m.from == current.as_str()) {
                None => {
                    let is_newer = {
                        let parse = |s: &str| -> (u32, u32) {
                            let mut it = s.splitn(2, '.').map(|p| p.parse::<u32>().unwrap_or(0));
                            (it.next().unwrap_or(0), it.next().unwrap_or(0))
                        };
                        parse(file_version.as_str()) > parse(CURRENT_VERSION)
                    };
                    return Err(format!(
                        "no migration path from schema_version '{}' to '{}'. \
                         This workflow was created by a {} version of Flowo. \
                         Upgrade Flowo to load it.",
                        file_version,
                        CURRENT_VERSION,
                        if is_newer { "newer" } else { "unknown" }
                    ));
                }
                Some(m) => {
                    (m.apply)(raw);
                    let next = m.to.to_string();
                    raw["schema_version"] = serde_json::Value::String(next.clone());
                    current = next;
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn engine() -> MigrationEngine { MigrationEngine::new() }

    // ── Baseline: no-op paths ──────────────────────────────────────────────────

    #[test]
    fn current_version_is_noop() {
        let mut raw = json!({ "schema_version": "1.0", "id": "wf-1", "name": "Test" });
        engine().apply(&mut raw).expect("current version must succeed");
        assert_eq!(raw["schema_version"], "1.0", "version must be unchanged");
    }

    #[test]
    fn missing_schema_version_defaults_to_current_and_is_noop() {
        let mut raw = json!({ "id": "wf-1", "name": "Test" });
        engine().apply(&mut raw).expect("missing field must default to 1.0 and succeed");
        // schema_version is not written back if it was absent (no-op path returns early)
    }

    // ── Error path: unknown version ────────────────────────────────────────────

    #[test]
    fn unknown_version_returns_err() {
        let mut raw = json!({ "schema_version": "99.0", "id": "wf-1" });
        let result = engine().apply(&mut raw);
        assert!(result.is_err(), "unknown version must return Err");
        let msg = result.unwrap_err();
        assert!(msg.contains("99.0"), "error must mention the bad version: {}", msg);
        assert!(msg.contains("1.0"),  "error must mention the target version: {}", msg);
    }

    // ── Migration execution: synthetic engines ─────────────────────────────────
    //
    // The production engine has no migration functions (CURRENT_VERSION == "1.0").
    // These tests build synthetic engines that chain versions below "1.0" to exercise
    // the apply() loop directly.

    #[test]
    fn single_step_migration_applied() {
        fn upgrade(raw: &mut serde_json::Value) {
            raw["upgraded"] = json!(true);
        }
        let eng = MigrationEngine {
            migrations: vec![Migration { from: "0.9", to: "1.0", apply: upgrade }],
        };

        let mut raw = json!({ "schema_version": "0.9", "id": "wf-1" });
        eng.apply(&mut raw).expect("0.9 -> 1.0 must succeed");

        assert_eq!(raw["schema_version"], "1.0",  "version must be updated");
        assert_eq!(raw["upgraded"],       true,    "migration fn must have run");
    }

    #[test]
    fn two_step_chain_applied_in_order() {
        fn step_a(raw: &mut serde_json::Value) { raw["ran_a"] = json!(true); }
        fn step_b(raw: &mut serde_json::Value) { raw["ran_b"] = json!(true); }

        let eng = MigrationEngine {
            migrations: vec![
                Migration { from: "0.8", to: "0.9", apply: step_a },
                Migration { from: "0.9", to: "1.0", apply: step_b },
            ],
        };

        let mut raw = json!({ "schema_version": "0.8" });
        eng.apply(&mut raw).expect("0.8 -> 0.9 -> 1.0 must succeed");

        assert_eq!(raw["schema_version"], "1.0", "version must reach current");
        assert_eq!(raw["ran_a"], true, "first migration must have run");
        assert_eq!(raw["ran_b"], true, "second migration must have run");
    }

    #[test]
    fn mid_chain_entry_applies_only_remaining_steps() {
        fn step_a(raw: &mut serde_json::Value) { raw["ran_a"] = json!(true); }
        fn step_b(raw: &mut serde_json::Value) { raw["ran_b"] = json!(true); }

        let eng = MigrationEngine {
            migrations: vec![
                Migration { from: "0.8", to: "0.9", apply: step_a },
                Migration { from: "0.9", to: "1.0", apply: step_b },
            ],
        };

        // File is at 0.9 — only step_b should run, not step_a.
        let mut raw = json!({ "schema_version": "0.9" });
        eng.apply(&mut raw).expect("0.9 -> 1.0 must succeed");

        assert_eq!(raw["schema_version"], "1.0", "version must reach current");
        assert_eq!(raw["ran_a"], json!(null), "step_a must NOT run for a 0.9 file");
        assert_eq!(raw["ran_b"], true,        "step_b must run");
    }

    #[test]
    fn gap_in_chain_returns_err() {
        fn step_a(raw: &mut serde_json::Value) { raw["ran_a"] = json!(true); }

        // Chain goes 0.8 → 0.9, but there's no 0.9 → 1.0.
        // A file at "0.8" should fail because the chain can't reach CURRENT_VERSION.
        let eng = MigrationEngine {
            migrations: vec![
                Migration { from: "0.8", to: "0.9", apply: step_a },
            ],
        };

        let mut raw = json!({ "schema_version": "0.8" });
        let result = eng.apply(&mut raw);
        assert!(result.is_err(), "broken chain must return Err");
        let msg = result.unwrap_err();
        assert!(msg.contains("0.8"), "error must mention the file version: {}", msg);
    }
}
