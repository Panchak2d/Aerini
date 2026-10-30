use std::path::Path;
use std::sync::Arc;

use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::OptionalExtension;

use crate::plugin_loader::bindings::wit_storage;

/// A [`PluginStorage`] handle plus the key-space `scope` this particular
/// `Store` reads and writes under -- see [`PluginStorage`]'s own doc
/// comment for what `scope` is derived from.
#[derive(Clone)]
pub(super) struct PluginStorageHandle {
    pub(super) db: Arc<PluginStorage>,
    pub(super) scope: String,
}

/// File name of the per-`plugin_dir` storage database — a dotfile sidecar
/// matching the `.aerini-plugin-trust.json` naming convention exactly (same
/// directory, same "hidden management file living next to the `.wasm`s"
/// pattern), scanned past by every existing `.wasm`-extension-filtered
/// directory walk in this crate and in `commands/plugins.rs` without any
/// change needed there.
const STORAGE_DB_FILENAME: &str = ".aerini-plugin-storage.db";

/// Max bytes for a single storage key.
const STORAGE_MAX_KEY_BYTES: usize = 256;
/// Max bytes for a single storage value.
const STORAGE_MAX_VALUE_BYTES: usize = 64 * 1024;
/// Max distinct keys one plugin (one `scope`) may hold at once — bounds
/// row/index growth independently of the byte quota below (many tiny keys
/// would otherwise sail under a bytes-only cap).
const STORAGE_MAX_KEYS_PER_SCOPE: i64 = 10_000;
/// Max total bytes (sum of all value lengths) one plugin (one `scope`) may
/// hold at once. A durable-notes budget, not a database's — see
/// `interface storage`'s own doc comment in `wit/node.wit`.
const STORAGE_MAX_SCOPE_BYTES: i64 = 5 * 1024 * 1024;

/// Bounded, host-provided key-value storage backing the WIT `storage`
/// import — one `PluginStorage` per `plugin_dir`, opened lazily and cached
/// (see [`PluginLoader::plugin_storage`]), shared via `Arc` across every
/// [`WasmPluginNode`] loaded from that directory. `r2d2::Pool` is
/// `Send + Sync` and cheap to clone, matching `Engine`'s own sharing model
/// elsewhere in this module — the same `rusqlite` + `r2d2`/`r2d2_sqlite`
/// stack already used by `crate::db::WorkflowDb`, reused here rather than
/// adding a new storage-backend dependency.
///
/// **Scoped per plugin type-id**, not per placed node instance: every
/// execution of a given plugin, across every workflow and every
/// placement, reads and writes the same key space. Resolved this way over
/// per-node-instance scoping because (a) it needs zero new plumbing —
/// `WasmPluginNode` already carries `type_id`, while node-instance scoping
/// would need additions to `node-input.params`'s shape that nothing else
/// here requires, and (b) it matches this capability's own framing as "a
/// safe substitute for real filesystem access" (see `plugin_loader`'s own
/// module-level doc comment) — a plugin's own app-data directory, on a
/// real filesystem, wouldn't vary by which workflow node happens to be
/// calling into it either. A plugin author who wants isolation between
/// two placements of their own node type must namespace their own keys
/// (e.g. fold a config value into the key) — documented on `interface
/// storage`. Revisit as per-node-instance scoping if shared state across
/// placements proves to cause real collisions in practice, not guessed at
/// here.
pub(super) struct PluginStorage {
    pool: r2d2::Pool<SqliteConnectionManager>,
}

impl PluginStorage {
    /// Opens (creating if absent) the storage database at
    /// `plugin_dir/`[`STORAGE_DB_FILENAME`]. Does not create `plugin_dir`
    /// itself — every real caller only reaches this after a `.wasm` file
    /// was already successfully read from that same directory (see
    /// [`PluginLoader::plugin_storage`]'s doc comment), so it's already
    /// known to exist.
    pub(super) fn open(plugin_dir: &Path) -> Result<Self, String> {
        let path = plugin_dir.join(STORAGE_DB_FILENAME);
        let manager = SqliteConnectionManager::file(&path).with_init(|conn| {
            conn.execute_batch(
                "PRAGMA busy_timeout=5000;
                 PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;"
            )
        });
        let pool = r2d2::Pool::builder().max_size(4).build(manager).map_err(|e| e.to_string())?;

        let conn = pool.get().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS plugin_storage (
                scope TEXT NOT NULL,
                key   TEXT NOT NULL,
                value BLOB NOT NULL,
                PRIMARY KEY (scope, key)
            );"
        ).map_err(|e| e.to_string())?;
        drop(conn);

        Ok(Self { pool })
    }

    pub(super) fn get(&self, scope: &str, key: &str) -> Option<String> {
        let conn = self.pool.get().map_err(|e| {
            tracing::warn!("plugin storage: pool unavailable on get: {e}");
        }).ok()?;
        conn.query_row(
            "SELECT value FROM plugin_storage WHERE scope = ?1 AND key = ?2",
            rusqlite::params![scope, key],
            |row| row.get::<_, String>(0),
        ).ok()
    }

    /// Checks the key/value size limits and the per-scope key-count/byte
    /// quotas, then writes — all inside one `IMMEDIATE` transaction so a
    /// concurrent `set` on the same scope (two workflow runs of the same
    /// plugin type in parallel) can't race the quota check against the
    /// write (`BEGIN IMMEDIATE` takes SQLite's write lock upfront, closing
    /// the check-then-write TOCTOU gap a plain `BEGIN` would leave open).
    /// Known, accepted trade-off: under real contention on the same scope,
    /// a losing transaction waits on `busy_timeout` (5s, set in `open`)
    /// rather than queuing indefinitely — a `set` can surface `unavailable`
    /// under sustained concurrent writes to one plugin's key space, not
    /// just on a genuine backend failure. Not expected to matter at this
    /// capability's intended scale (a few small notes, not a write-heavy
    /// database); documented rather than engineered around.
    pub(super) fn set(&self, scope: &str, key: &str, value: &str) -> Result<(), wit_storage::StorageError> {
        if key.len() > STORAGE_MAX_KEY_BYTES {
            return Err(wit_storage::StorageError::KeyTooLong);
        }
        if value.len() > STORAGE_MAX_VALUE_BYTES {
            return Err(wit_storage::StorageError::ValueTooLarge);
        }

        let mut conn = self.pool.get().map_err(|e| {
            tracing::warn!("plugin storage: pool unavailable on set: {e}");
            wit_storage::StorageError::Unavailable
        })?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| {
                tracing::warn!("plugin storage: failed to start transaction: {e}");
                wit_storage::StorageError::Unavailable
            })?;

        let existing_len: Option<i64> = tx
            .query_row(
                "SELECT LENGTH(CAST(value AS BLOB)) FROM plugin_storage WHERE scope = ?1 AND key = ?2",
                rusqlite::params![scope, key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| {
                tracing::warn!("plugin storage: quota pre-check failed: {e}");
                wit_storage::StorageError::Unavailable
            })?;

        if existing_len.is_none() {
            let key_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM plugin_storage WHERE scope = ?1",
                    rusqlite::params![scope],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    tracing::warn!("plugin storage: key-count check failed: {e}");
                    wit_storage::StorageError::Unavailable
                })?;
            if key_count >= STORAGE_MAX_KEYS_PER_SCOPE {
                return Err(wit_storage::StorageError::QuotaExceeded);
            }
        }

        let scope_total: i64 = tx
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(CAST(value AS BLOB))), 0) FROM plugin_storage WHERE scope = ?1",
                rusqlite::params![scope],
                |row| row.get(0),
            )
            .map_err(|e| {
                tracing::warn!("plugin storage: total-bytes check failed: {e}");
                wit_storage::StorageError::Unavailable
            })?;

        let projected = scope_total - existing_len.unwrap_or(0) + value.len() as i64;
        if projected > STORAGE_MAX_SCOPE_BYTES {
            return Err(wit_storage::StorageError::QuotaExceeded);
        }

        tx.execute(
            "INSERT INTO plugin_storage (scope, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(scope, key) DO UPDATE SET value = excluded.value",
            rusqlite::params![scope, key, value],
        ).map_err(|e| {
            tracing::warn!("plugin storage: write failed: {e}");
            wit_storage::StorageError::Unavailable
        })?;

        tx.commit().map_err(|e| {
            tracing::warn!("plugin storage: commit failed: {e}");
            wit_storage::StorageError::Unavailable
        })?;

        Ok(())
    }

    pub(super) fn delete(&self, scope: &str, key: &str) {
        let Ok(conn) = self.pool.get().map_err(|e| tracing::warn!("plugin storage: pool unavailable on delete: {e}")) else {
            return;
        };
        if let Err(e) = conn.execute(
            "DELETE FROM plugin_storage WHERE scope = ?1 AND key = ?2",
            rusqlite::params![scope, key],
        ) {
            tracing::warn!("plugin storage: delete failed: {e}");
        }
    }

    /// Lists keys under `scope` starting with `prefix`. `prefix` is
    /// caller-supplied (guest-controlled) content, not a trusted pattern —
    /// `%`/`_`/`\` are escaped to literals before being used in a SQL
    /// `LIKE ... ESCAPE '\'` clause, so a key containing those characters
    /// can't be used to widen the match beyond a literal prefix.
    pub(super) fn list_keys(&self, scope: &str, prefix: &str) -> Result<Vec<String>, wit_storage::StorageError> {
        let conn = self.pool.get().map_err(|e| {
            tracing::warn!("plugin storage: pool unavailable on list_keys: {e}");
            wit_storage::StorageError::Unavailable
        })?;

        let mut escaped = String::with_capacity(prefix.len());
        for c in prefix.chars() {
            if c == '%' || c == '_' || c == '\\' {
                escaped.push('\\');
            }
            escaped.push(c);
        }
        let pattern = format!("{escaped}%");

        let mut stmt = conn
            .prepare("SELECT key FROM plugin_storage WHERE scope = ?1 AND key LIKE ?2 ESCAPE '\\' ORDER BY key")
            .map_err(|e| {
                tracing::warn!("plugin storage: list_keys prepare failed: {e}");
                wit_storage::StorageError::Unavailable
            })?;
        let rows = stmt
            .query_map(rusqlite::params![scope, pattern], |row| row.get::<_, String>(0))
            .map_err(|e| {
                tracing::warn!("plugin storage: list_keys query failed: {e}");
                wit_storage::StorageError::Unavailable
            })?;

        let mut keys = Vec::new();
        for row in rows {
            keys.push(row.map_err(|e| {
                tracing::warn!("plugin storage: list_keys row read failed: {e}");
                wit_storage::StorageError::Unavailable
            })?);
        }
        Ok(keys)
    }
}

#[cfg(test)]
mod plugin_storage_tests {
    use super::*;

    fn storage() -> PluginStorage {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        // Leak the TempDir so it outlives this test's `PluginStorage` --
        // acceptable in a `#[cfg(test)]`-only helper (mirrors the `NamedTempFile`
        // test fixtures in `loader/tests.rs`, which rely on the
        // guard staying alive for the test's duration rather than cleaning
        // up immediately).
        let path = dir.keep();
        PluginStorage::open(&path).expect("PluginStorage::open failed")
    }

    /// Edge case: the scope quota counts bytes, so multi-byte text cannot
    /// store more than the quota's worth.
    #[test]
    fn scope_quota_counts_bytes_not_characters() {
        let db = storage();
        let value = "€".repeat(STORAGE_MAX_VALUE_BYTES / 3);
        let fits = (STORAGE_MAX_SCOPE_BYTES as usize) / value.len();
        for i in 0..fits {
            db.set("plugin-a", &format!("k{i}"), &value).expect("write within quota failed");
        }
        assert!(matches!(
            db.set("plugin-a", "one-too-many", &value),
            Err(wit_storage::StorageError::QuotaExceeded)
        ));
    }

    /// Normal case: a value written by `set` is returned by `get`, scoped
    /// under the same `scope` it was written under.
    #[test]
    fn set_then_get_round_trips() {
        let db = storage();
        assert_eq!(db.get("plugin-a", "k1"), None, "unset key must read as absent");
        db.set("plugin-a", "k1", "hello").expect("set failed");
        assert_eq!(db.get("plugin-a", "k1"), Some("hello".to_string()));
        // A different scope must not see it.
        assert_eq!(db.get("plugin-b", "k1"), None, "scopes must not leak into each other");
    }

    /// Edge case: oversized keys/values are rejected before ever reaching
    /// SQLite, with the specific error the size limit that was exceeded.
    #[test]
    fn set_rejects_oversized_key_and_value() {
        let db = storage();
        let long_key = "k".repeat(STORAGE_MAX_KEY_BYTES + 1);
        assert!(matches!(db.set("plugin-a", &long_key, "v"), Err(wit_storage::StorageError::KeyTooLong)));

        let long_value = "v".repeat(STORAGE_MAX_VALUE_BYTES + 1);
        assert!(matches!(db.set("plugin-a", "k1", &long_value), Err(wit_storage::StorageError::ValueTooLarge)));
    }

    /// Edge case: the per-scope byte quota is enforced across multiple
    /// keys, and an update to an *existing* key correctly nets out its own
    /// prior size rather than double-counting it.
    #[test]
    fn set_enforces_scope_byte_quota() {
        let db = storage();
        let chunk = "x".repeat(STORAGE_MAX_VALUE_BYTES);
        let mut written = 0i64;
        let mut i = 0;
        loop {
            let key = format!("k{i}");
            match db.set("plugin-a", &key, &chunk) {
                Ok(()) => { written += chunk.len() as i64; i += 1; }
                Err(wit_storage::StorageError::QuotaExceeded) => break,
                Err(e) => panic!("unexpected error: {e:?}"),
            }
            assert!(written <= STORAGE_MAX_SCOPE_BYTES, "wrote past the quota before it was enforced");
        }
        assert!(written > 0, "quota must allow at least one write before rejecting");

        // Overwriting an already-stored key with an equal-sized value must
        // still succeed -- it doesn't add new net bytes to the scope.
        db.set("plugin-a", "k0", &chunk).expect("overwrite of existing key must not double-count its own prior size");
    }

    #[test]
    fn delete_removes_key() {
        let db = storage();
        db.set("plugin-a", "k1", "v").expect("set failed");
        assert_eq!(db.get("plugin-a", "k1"), Some("v".to_string()));
        db.delete("plugin-a", "k1");
        assert_eq!(db.get("plugin-a", "k1"), None);
        // Deleting an already-absent key is a no-op, not an error.
        db.delete("plugin-a", "does-not-exist");
    }

    /// `%`/`_` in a caller-supplied prefix must be treated as literal
    /// characters, not SQL `LIKE` wildcards -- otherwise a plugin could
    /// list keys outside the prefix it actually asked for.
    #[test]
    fn list_keys_escapes_like_metacharacters() {
        let db = storage();
        db.set("plugin-a", "a%b", "v1").expect("set failed");
        db.set("plugin-a", "aXb", "v2").expect("set failed");
        db.set("plugin-a", "a%bc", "v3").expect("set failed");

        let matches = db.list_keys("plugin-a", "a%b").expect("list_keys failed");
        assert_eq!(
            matches,
            vec!["a%b".to_string(), "a%bc".to_string()],
            "prefix 'a%b' must match only keys literally starting with 'a%b', not 'aXb' via wildcard expansion"
        );
    }
}
