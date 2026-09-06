# Schema Migrations

This page covers the mechanism behind a workflow's `schema_version` field and the version-by-version history of the format, for anyone working inside `aerini-engine` itself. It assumes you already know the file's overall shape; see [Workflow File Format](workflow-file-format.md) first if not. For what a user or operator actually sees when a file's version doesn't match the app or server opening it, see [Updating §Opening older or newer workflow files](../operations/updating.md#opening-older-or-newer-workflow-files); that outcome table isn't repeated here.

## Version history

| Version | Introduced | Changes |
|---|---|---|
| `1.0` | Aerini's first release | Initial format. Every workflow file that exists today uses this version. |

`migration.rs`'s `CURRENT_VERSION` is `"1.0"`, and its list of registered migrations is empty. This table has one row because the format hasn't changed since it was introduced, not because earlier rows were left out.

## The migration engine

`aerini-engine`'s `migration.rs` defines `MigrationEngine`: a list of `Migration` entries, each with a `from` version, a `to` version, and an `apply` function that mutates a workflow's raw JSON in place, before that JSON is parsed into a `Workflow` struct. `MigrationEngine::apply()` is the entry point:

- If the JSON's `schema_version` field is missing, it's treated as `"1.0"`, the oldest version the engine knows.
- If that version already matches `CURRENT_VERSION`, `apply()` returns immediately. No migration function runs, and `schema_version` is left as it was.
- Otherwise, the engine walks forward one step at a time. At each step it looks for the registered migration whose `from` matches the current version, runs that migration's `apply` function, writes its `to` value into `schema_version`, then repeats from the new current version, until the version reaches `CURRENT_VERSION`.

Two invariants keep that walk safe:

- **The chain can't have gaps.** Every version between the oldest file the engine still supports and `CURRENT_VERSION` needs a registered migration whose `from` matches the previous step's `to`. If the walk lands on a version with no matching migration and that version isn't `CURRENT_VERSION`, `apply()` stops and returns an error rather than silently giving up partway.
- **Migrations run in order, and `schema_version` updates after each one.** A file already partway through the chain only runs its remaining steps. A file that starts at `"0.9"` in a chain that also has an earlier `"0.8"` to `"0.9"` step never re-runs that earlier step, because the walk always starts from whatever version the file actually reports.

A set of visited versions guards against a registration mistake elsewhere in the chain: if the same version comes up twice during one walk, meaning two migrations point back into each other, `apply()` returns an error naming the cycle instead of looping forever.

## What apply() returns

| File's `schema_version` | Result |
|---|---|
| Equal to `CURRENT_VERSION` | `Ok(())` immediately. No migration runs. |
| Missing entirely | Treated as `"1.0"` and handled the same as any other version from there. Since `CURRENT_VERSION` is also `"1.0"` today, this currently resolves the same way as the row above, though the two defaults come from different places: the engine's own fallback runs first, before the `Workflow` struct's own field default ever gets a chance to matter. |
| Older, with a migration chain reaching `CURRENT_VERSION` | Walks the chain, applying each step and updating `schema_version` as it goes, then returns `Ok(())` once it arrives. |
| Older, with no chain reaching `CURRENT_VERSION` | `Err`, naming both versions and describing the file as created by an unrecognized version of Aerini. |
| Newer than `CURRENT_VERSION` | `Err`, naming both versions and telling the caller to upgrade Aerini instead. There's no downgrade path either way; a newer file never partially loads. |

The last two cases are told apart by comparing the file's version against `CURRENT_VERSION` as a major/minor pair, not by string equality, so a file at `"2.0"` is correctly reported as newer even though nothing in the registered chain matches it either.

## Where migration runs

`Workflow::from_json` (`model.rs`) is the only place that calls `MigrationEngine::apply()`. It parses the raw string into a JSON value, runs the migration engine against that value, then deserializes the result into a `Workflow`. Every real loader goes through this one function instead of deserializing `Workflow` directly: the desktop app's own workflow commands, `aerini-server`'s startup load and its save endpoint, and the engine's own scheduler and SQLite-backed workflow store all call `from_json` rather than keeping a migration path of their own.

## Adding a migration

When a change to the workflow JSON format needs old files to keep loading correctly:

1. Bump `CURRENT_VERSION` in `migration.rs`, for example `"1.0"` to `"1.1"`.
2. Write a function that takes the raw JSON and changes it in place: rename a key, restructure a node's `config`, whatever the format change actually requires.
3. Register a `Migration` entry for it in `MigrationEngine::new()`'s list, with `from` set to the version you're migrating away from and `to` set to the new `CURRENT_VERSION`.
4. Add a row to the [version history](#version-history) table above.

Keep the function pure: the same input has to produce the same output every time, with no reading from disk or the network involved. Never drop data silently either. If a field genuinely can't be carried forward, that's a real design decision worth documenting in the function itself, not something to fold quietly into an unrelated rename. `schema_version` updates automatically once your function returns; don't set it by hand inside `apply`.

## Not covered here

`CHANGELOG.md`, at the repository root, tracks Aerini's own application releases, in SemVer and Keep a Changelog format. It has no relationship to `schema_version`: an app release can ship with no format change at all, which is what every release through 0.4.0 has done, and a future format change wouldn't need to line up with any particular app version number either.

## What's next

- [Workflow File Format](workflow-file-format.md), for the current shape this version describes
- [Updating §Opening older or newer workflow files](../operations/updating.md#opening-older-or-newer-workflow-files), for what a user or operator sees when a version mismatch actually happens
