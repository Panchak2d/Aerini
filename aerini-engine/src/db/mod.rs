//! Workflow and run-history database.
//!
//! Moved from commands/workflow.rs so the server binary can use it without
//! depending on Tauri. The Tauri app imports this from aerini_engine::db.

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::perf_monitor::PerformanceReport;

mod workflow_db;
mod run_history;
mod scheduler_db;
mod performance_reports;
mod chat_db;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatImageFile {
    pub filename:  String,
    pub data:      String,
    pub mime_type: String,
}

/// A file the user attached to a sent chat message — same shape as
/// [`ChatImageFile`], but not limited to images.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatAttachment {
    pub filename:  String,
    pub data:      String,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessageRecord {
    pub id:          String,
    pub role:        String,
    pub text:        Option<String>,
    pub images:      Option<Vec<ChatImageFile>>,
    pub attachments: Option<Vec<ChatAttachment>>,
    pub timestamp:   i64,
}

/// A chat session without its messages — what the session list and the
/// per-message append command carry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSessionMeta {
    pub id:          String,
    pub workflow_id: String,
    pub name:        String,
    pub created_at:  i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSessionRecord {
    pub id:          String,
    pub workflow_id: String,
    pub name:        String,
    pub created_at:  i64,
    pub messages:    Vec<ChatMessageRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id:            String,
    pub workflow_id:   String,
    pub workflow_name: String,
    pub ran_at:        String,
    pub success:       bool,
    pub duration_ms:   i64,
    pub result_json:   String,
    /// One of "running", "success", "failed", "interrupted".
    pub status:        String,
}

/// A persisted [`PerformanceReport`], linked to its `run_history` row by 'id'

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceReportRecord {
    pub id: String,
    #[serde(flatten)]
    pub report: PerformanceReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionRow {
    pub id:          String,
    pub workflow_id: String,
    pub message:     Option<String>,
    pub created_at:  String,
}

pub(super) fn row_to_run(row: &rusqlite::Row) -> rusqlite::Result<RunRecord> {
    Ok(RunRecord {
        id:            row.get(0)?,
        workflow_id:   row.get(1)?,
        workflow_name: row.get(2)?,
        ran_at:        row.get(3)?,
        success:       row.get::<_, i64>(4)? != 0,
        duration_ms:   row.get(5)?,
        result_json:   run_history::expand_result_json(row.get(6)?),
        status:        row.get(7)?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSummary {
    pub id:            String,
    pub name:          String,
    pub updated_at:    String,
    pub tags:          Vec<String>,
    pub collection_id: Option<String>,
}

/// Outcome of a conditional save via [`WorkflowDb::save_checked`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    /// No row existed for this id yet — inserted fresh at `row_version` 1.
    Created { row_version: i64 },
    /// A prior row existed and, if the caller supplied an expected version,
    /// it matched — updated, `row_version` incremented by one.
    Updated { row_version: i64 },
    /// A prior row existed but at a different `row_version` than the caller
    /// expected — nothing was written.
    Conflict { current_row_version: i64 },
    /// The caller supplied an expected version but no row exists for this
    /// id — an If-Match precondition can't be satisfied against nothing.
    NotFound,
}

pub struct WorkflowDb {
    pub(super) pool:                    Pool<SqliteConnectionManager>,
    pub(super) history_limit:           i64,
    pub(super) version_retention_limit: i64,
}

impl WorkflowDb {
    /// Current schema version. Increment this and add a `migrate_vN` block
    /// in `run_migrations` for every schema change.
    pub(super) const SCHEMA_VERSION: i64 = 9;

    pub fn open(path: &PathBuf, pool_size: usize) -> Result<Self, String> {
        let manager = SqliteConnectionManager::file(path)
            .with_init(|conn| {
                conn.execute_batch(
                    "PRAGMA busy_timeout=5000;
                     PRAGMA journal_mode=WAL;
                     PRAGMA synchronous=NORMAL;
                     PRAGMA cache_size=-8000;
                     PRAGMA temp_store=MEMORY;
                     PRAGMA foreign_keys=ON;"
                )
            });

        // r2d2 re-enforces min_idle as a floor on every checkout, not just at
        // pool construction: establish_idle_connections runs inside
        // try_get_inner right after popping an idle connection, before that
        // connection is returned to the caller. With min_idle(Some(1)),
        // checking out the sole warmed connection below immediately schedules
        // a background replacement connection; if that finishes before this
        // connection is dropped — e.g. while the migrations below run — the
        // pool ends up with two connections instead of one. min_idle(Some(0))
        // keeps that floor at zero, so the replenishment call is always a
        // no-op: a connection is only ever created in direct response to real
        // demand. Later connections still join an already-WAL database
        // cleanly, however many and however concurrent, so this does not
        // limit the pool's real capacity under load.
        let pool = Pool::builder()
            .max_size(pool_size.max(1) as u32)
            .min_idle(Some(0))
            .build(manager)
            .map_err(|e| e.to_string())?;

        let (history_limit, version_retention_limit) = {
            let conn = pool.get().map_err(|e| e.to_string())?;
            Self::run_migrations(&conn)?;
            conn.execute(
                "UPDATE run_history SET status = 'interrupted' WHERE status = 'running'",
                [],
            ).map_err(|e| e.to_string())?;

            let history_limit = match conn.query_row(
                "SELECT value FROM settings WHERE key = 'run_history_limit'",
                [],
                |row| row.get::<_, String>(0),
            ) {
                Ok(v) => v.parse::<i64>().unwrap_or(500).max(1),
                Err(rusqlite::Error::QueryReturnedNoRows) => 500,
                Err(e) => return Err(e.to_string()),
            };

            // Separate from history_limit (unlike performance_reports, which
            // deliberately reuses it): versions default to 50, distinct from
            // history_limit's default of 500 — reusing history_limit's default
            // here would silently change established retention behavior for
            // existing users.
            let version_retention_limit = match conn.query_row(
                "SELECT value FROM settings WHERE key = 'version_retention_limit'",
                [],
                |row| row.get::<_, String>(0),
            ) {
                Ok(v) => v.parse::<i64>().unwrap_or(50).max(1),
                Err(rusqlite::Error::QueryReturnedNoRows) => 50,
                Err(e) => return Err(e.to_string()),
            };

            (history_limit, version_retention_limit)
        };

        Ok(Self { pool, history_limit, version_retention_limit })
    }

    fn run_migrations(conn: &rusqlite::Connection) -> Result<(), String> {
        let current_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;

        if current_version >= Self::SCHEMA_VERSION {
            return Ok(());
        }

        if current_version < 1 {
            Self::migrate_v1(conn)?;
        }
        if current_version < 2 {
            Self::migrate_v2(conn)?;
        }
        if current_version < 3 {
            Self::migrate_v3(conn)?;
        }
        if current_version < 4 {
            Self::migrate_v4(conn)?;
        }
        if current_version < 5 {
            Self::migrate_v5(conn)?;
        }
        if current_version < 6 {
            Self::migrate_v6(conn)?;
        }
        if current_version < 7 {
            Self::migrate_v7(conn)?;
        }
        if current_version < 8 {
            Self::migrate_v8(conn)?;
        }
        if current_version < 9 {
            Self::migrate_v9(conn)?;
        }

        Ok(())
    }

    /// All tables created with IF NOT EXISTS so this is safe to run against a
    /// pre-migration database that already has some or all tables present.
    fn migrate_v1(conn: &rusqlite::Connection) -> Result<(), String> {
        conn.execute_batch("
            BEGIN;
            CREATE TABLE IF NOT EXISTS workflows (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                json        TEXT NOT NULL,
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS scheduled_jobs (
                workflow_id    TEXT PRIMARY KEY,
                workflow_name  TEXT NOT NULL,
                trigger_kind   TEXT NOT NULL,
                status         TEXT NOT NULL DEFAULT 'active',
                always_on      INTEGER NOT NULL DEFAULT 0,
                run_count      INTEGER NOT NULL DEFAULT 0,
                last_run_at    TEXT,
                next_run_at    TEXT,
                last_error     TEXT,
                created_at     TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS run_history (
                id              TEXT PRIMARY KEY,
                workflow_id     TEXT NOT NULL,
                workflow_name   TEXT NOT NULL,
                ran_at          TEXT NOT NULL,
                success         INTEGER NOT NULL,
                duration_ms     INTEGER NOT NULL,
                result_json     TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS workflow_variables (
                workflow_id   TEXT NOT NULL,
                variable_name TEXT NOT NULL,
                value_json    TEXT NOT NULL,
                updated_at    TEXT NOT NULL,
                PRIMARY KEY (workflow_id, variable_name)
            );
            CREATE TABLE IF NOT EXISTS workflow_versions (
                id          TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL,
                snapshot    TEXT NOT NULL,
                message     TEXT,
                created_at  TEXT NOT NULL,
                FOREIGN KEY (workflow_id) REFERENCES workflows(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_run_history_workflow
                ON run_history(workflow_id, ran_at DESC);
            PRAGMA user_version = 1;
            COMMIT;
        ").map_err(|e| e.to_string())
    }

    /// Version 2 — adds `status` to `run_history` so a "running" record can be
    /// written before execution starts, distinguishing crashed/interrupted runs
    /// from completed ones. Existing rows default to 'complete'.
    fn migrate_v2(conn: &rusqlite::Connection) -> Result<(), String> {
        conn.execute_batch("
            BEGIN;
            ALTER TABLE run_history ADD COLUMN status TEXT NOT NULL DEFAULT 'complete';
            PRAGMA user_version = 2;
            COMMIT;
        ").map_err(|e| e.to_string())
    }


    fn migrate_v3(conn: &rusqlite::Connection) -> Result<(), String> {
        conn.execute_batch("
            BEGIN;
            CREATE TABLE IF NOT EXISTS performance_reports (
                id                   TEXT PRIMARY KEY,
                workflow_id          TEXT NOT NULL,
                status               TEXT NOT NULL,
                started_at_ms        INTEGER NOT NULL,
                finished_at_ms       INTEGER NOT NULL,
                duration_ms          INTEGER NOT NULL,
                sampling_interval_ms INTEGER NOT NULL,
                baseline_bytes       INTEGER NOT NULL,
                final_bytes          INTEGER NOT NULL,
                peak_bytes           INTEGER NOT NULL,
                peak_at_ms           INTEGER NOT NULL,
                minimum_bytes        INTEGER NOT NULL,
                average_bytes        INTEGER NOT NULL,
                delta_bytes          INTEGER NOT NULL,
                sample_count         INTEGER NOT NULL,
                history_json         TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_performance_reports_workflow
                ON performance_reports(workflow_id, started_at_ms DESC);
            PRAGMA user_version = 3;
            COMMIT;
        ").map_err(|e| e.to_string())
    }

    fn migrate_v4(conn: &rusqlite::Connection) -> Result<(), String> {
        conn.execute_batch("
            BEGIN;
            CREATE TABLE IF NOT EXISTS chat_sessions (
                id          TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL,
                name        TEXT NOT NULL,
                created_at  INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chat_messages (
                id          TEXT PRIMARY KEY,
                session_id  TEXT NOT NULL,
                role        TEXT NOT NULL,
                text        TEXT,
                images_json TEXT,
                timestamp   INTEGER NOT NULL,
                FOREIGN KEY (session_id) REFERENCES chat_sessions(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_chat_messages_session
                ON chat_messages(session_id, timestamp);
            CREATE INDEX IF NOT EXISTS idx_chat_sessions_workflow
                ON chat_sessions(workflow_id, created_at DESC);
            PRAGMA user_version = 4;
            COMMIT;
        ").map_err(|e| e.to_string())
    }

    fn migrate_v5(conn: &rusqlite::Connection) -> Result<(), String> {
        conn.execute_batch("
            BEGIN;
            CREATE INDEX IF NOT EXISTS idx_workflow_versions_workflow
                ON workflow_versions(workflow_id, created_at DESC);
            PRAGMA user_version = 5;
            COMMIT;
        ").map_err(|e| e.to_string())
    }

    /// Version 6 — adds `tags` to `workflows` as a JSON-encoded string array,
    /// mirroring the `run_history.status` pattern from migrate_v2. Populated
    /// from `WorkflowMetadata.tags` at save time (already in memory there —
    /// no JSON-extraction from the `json` column needed). Existing rows
    /// default to an empty array.
    fn migrate_v6(conn: &rusqlite::Connection) -> Result<(), String> {
        conn.execute_batch("
            BEGIN;
            ALTER TABLE workflows ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
            PRAGMA user_version = 6;
            COMMIT;
        ").map_err(|e| e.to_string())
    }

    /// Version 7 — adds `row_version` to `workflows`: an optimistic-concurrency
    /// counter for the API server's `POST /api/workflows` (`If-Match`/`ETag`).
    /// Existing rows default to 1; every subsequent `save()`/`save_checked()`
    /// increments it, so it stays a reliable "have I got the latest?" token.
    fn migrate_v7(conn: &rusqlite::Connection) -> Result<(), String> {
        conn.execute_batch("
            BEGIN;
            ALTER TABLE workflows ADD COLUMN row_version INTEGER NOT NULL DEFAULT 1;
            PRAGMA user_version = 7;
            COMMIT;
        ").map_err(|e| e.to_string())
    }

    /// Version 8 — adds `collection_id` to `workflows`. Unlike `tags`
    /// (migrate_v6), this has no `DEFAULT` — a workflow with no collection
    /// is `NULL`, not an empty-array stand-in, so existing rows and the
    /// column's absence-state are the same value. Populated from
    /// `WorkflowMetadata.collection_id` at save time, mirroring the `tags`
    /// pattern otherwise.
    fn migrate_v8(conn: &rusqlite::Connection) -> Result<(), String> {
        conn.execute_batch("
            BEGIN;
            ALTER TABLE workflows ADD COLUMN collection_id TEXT;
            PRAGMA user_version = 8;
            COMMIT;
        ").map_err(|e| e.to_string())
    }

    /// Version 9 — adds `attachments_json` to `chat_messages`: the
    /// JSON-encoded list of files the user attached to a sent message,
    /// stored the same way as `images_json`. No `DEFAULT` — a message with
    /// no attachments, including every row that predates this column, is
    /// `NULL`.
    fn migrate_v9(conn: &rusqlite::Connection) -> Result<(), String> {
        conn.execute_batch("
            BEGIN;
            ALTER TABLE chat_messages ADD COLUMN attachments_json TEXT;
            PRAGMA user_version = 9;
            COMMIT;
        ").map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aerini_db_test_{}.db", tag));
        p
    }

    fn cleanup(path: &PathBuf) {
        let _ = std::fs::remove_file(path);
        let mut wal = path.clone();
        wal.set_extension("db-wal");
        let _ = std::fs::remove_file(&wal);
        let mut shm = path.clone();
        shm.set_extension("db-shm");
        let _ = std::fs::remove_file(&shm);
    }

    #[test]
    fn test_fresh_db_schema_version() {
        let path = temp_path("fresh");
        cleanup(&path);

        let db = WorkflowDb::open(&path, 8).expect("open failed");
        let conn = db.pool.get().unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, WorkflowDb::SCHEMA_VERSION);

        cleanup(&path);
    }

    #[test]
    fn test_existing_db_migrates_and_preserves_data() {
        let path = temp_path("existing");
        cleanup(&path);

        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch("
                CREATE TABLE IF NOT EXISTS workflows (
                    id TEXT PRIMARY KEY, name TEXT NOT NULL,
                    json TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                );
                INSERT INTO workflows (id, name, json, created_at, updated_at)
                VALUES ('wf-1', 'My Workflow', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z');
            ").unwrap();
        }

        let db = WorkflowDb::open(&path, 8).expect("migration failed");

        let conn = db.pool.get().unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, WorkflowDb::SCHEMA_VERSION, "must be migrated to the current schema version");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM workflows", [], |r| r.get::<_, i64>(0))
            .unwrap();
        assert_eq!(count, 1, "pre-existing row must survive migration");

        let tags: String = conn
            .query_row("SELECT tags FROM workflows WHERE id = 'wf-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tags, "[]", "legacy row predating the tags column must default to an empty array");

        cleanup(&path);
    }

    #[test]
    fn test_open_twice_is_idempotent() {
        let path = temp_path("idempotent");
        cleanup(&path);

        WorkflowDb::open(&path, 8).expect("first open failed");
        WorkflowDb::open(&path, 8).expect("second open must not fail");

        cleanup(&path);
    }

    // Edge case: a chat message written while the schema was still at v8 — no
    // attachments_json column yet — survives the v9 upgrade and reads back
    // with no attachments.
    #[test]
    fn chat_message_from_v8_database_upgrades_with_no_attachments() {
        let path = temp_path("chat_v8_upgrade");
        cleanup(&path);

        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            WorkflowDb::migrate_v1(&conn).unwrap();
            WorkflowDb::migrate_v2(&conn).unwrap();
            WorkflowDb::migrate_v3(&conn).unwrap();
            WorkflowDb::migrate_v4(&conn).unwrap();
            WorkflowDb::migrate_v5(&conn).unwrap();
            WorkflowDb::migrate_v6(&conn).unwrap();
            WorkflowDb::migrate_v7(&conn).unwrap();
            WorkflowDb::migrate_v8(&conn).unwrap();
            conn.execute_batch("
                INSERT INTO chat_sessions (id, workflow_id, name, created_at)
                VALUES ('s1', 'wf-1', 'Old', 1000);
                INSERT INTO chat_messages (id, session_id, role, text, images_json, timestamp)
                VALUES ('m1', 's1', 'user', 'hello', NULL, 1001);
            ").unwrap();
            let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, 8, "fixture must start at the pre-upgrade schema version");
        }

        let db = WorkflowDb::open(&path, 8).expect("upgrade failed");

        let metas = db.list_chat_session_meta("wf-1").expect("list failed");
        let messages = db.load_chat_messages(&metas[0].id).expect("load failed");
        assert_eq!(messages[0].text.as_deref(), Some("hello"));
        assert!(messages[0].attachments.is_none());

        let conn = db.pool.get().unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, WorkflowDb::SCHEMA_VERSION);

        cleanup(&path);
    }

    fn run_with_result(id: &str, result: &serde_json::Value) -> RunRecord {
        RunRecord {
            id: id.to_string(),
            workflow_id: "wf-1".to_string(),
            workflow_name: "Chat".to_string(),
            ran_at: "2024-01-01T00:00:00Z".to_string(),
            success: true,
            duration_ms: 1,
            result_json: result.to_string(),
            status: "success".to_string(),
        }
    }

    fn stored_result_json(db: &WorkflowDb, id: &str) -> String {
        let conn = db.pool.get().unwrap();
        conn.query_row("SELECT result_json FROM run_history WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    }

    // Normal case: attachments duplicated under body.attachments and files are
    // stored once, and every read path hands back the original document.
    #[test]
    fn run_history_stores_duplicated_attachments_once_and_reads_them_back_intact() {
        let path = temp_path("run_dedupe");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        let blob = "A".repeat(4096);
        let files = serde_json::json!([{ "filename": "a.pdf", "data": blob, "mime_type": "application/pdf" }]);
        let result = serde_json::json!({
            "success": true,
            "node_outputs": { "trigger": { "body": { "message": "hi", "attachments": files }, "files": files } },
        });
        db.save_run(&run_with_result("r1", &result)).expect("save failed");

        let raw = stored_result_json(&db, "r1");
        assert!(raw.len() < result.to_string().len() * 6 / 10, "attachment payload must be stored once");
        assert!(raw.contains("$same_as"));

        let got = db.get_run("r1").unwrap().unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&got.result_json).unwrap(), result);
        let listed = db.list_runs("wf-1", 0, 10, "all").unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&listed[0].result_json).unwrap(), result);

        cleanup(&path);
    }

    // Edge case: anything that is not an exact duplicate — differing arrays, a
    // missing `files`, a body that merely looks like the marker, non-JSON — is
    // stored byte-for-byte as received and read back unchanged.
    #[test]
    fn run_history_leaves_non_duplicate_results_untouched() {
        let path = temp_path("run_dedupe_edge");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        let differing = serde_json::json!({ "node_outputs": { "t": {
            "body": { "attachments": [{ "filename": "a" }] }, "files": [{ "filename": "b" }] } } });
        let no_files = serde_json::json!({ "node_outputs": { "t": { "body": { "attachments": [{ "filename": "a" }] } } } });
        for (id, r) in [("r-diff", &differing), ("r-nofiles", &no_files)] {
            db.save_run(&run_with_result(id, r)).unwrap();
            assert_eq!(stored_result_json(&db, id), r.to_string());
        }

        // A caller-supplied body that happens to look like the marker is data,
        // not something to expand.
        let literal = serde_json::json!({ "node_outputs": { "t": {
            "body": { "attachments": { "$same_as": "files" } }, "files": [{ "filename": "b" }] } } });
        db.save_run(&run_with_result("r-literal", &literal)).unwrap();
        assert_eq!(stored_result_json(&db, "r-literal"), literal.to_string());
        let read = db.get_run("r-literal").unwrap().unwrap().result_json;
        assert_eq!(serde_json::from_str::<serde_json::Value>(&read).unwrap(), literal);

        let mut junk = run_with_result("r-junk", &differing);
        junk.result_json = "not json \"attachments\"".to_string();
        db.save_run(&junk).unwrap();
        assert_eq!(db.get_run("r-junk").unwrap().unwrap().result_json, "not json \"attachments\"");

        cleanup(&path);
    }

    #[test]
    fn test_status_column_default_and_interrupted_sweep() {
        let path = temp_path("status_col");
        cleanup(&path);

        let db = WorkflowDb::open(&path, 8).expect("open failed");

        // save_run_started writes status = 'running'.
        db.save_run_started("run-1", "wf-1", "My Workflow", "2024-01-01T00:00:00Z")
            .expect("save_run_started failed");
        let row = db.get_run("run-1").expect("get_run failed").expect("row missing");
        assert_eq!(row.status, "running");
        assert!(!row.success);
        assert_eq!(row.duration_ms, 0);

        // save_run upserts the same id, overwriting status to 'success'/'failed'.
        db.save_run(&RunRecord {
            id: "run-1".to_string(),
            workflow_id: "wf-1".to_string(),
            workflow_name: "My Workflow".to_string(),
            ran_at: "2024-01-01T00:00:05Z".to_string(),
            success: true,
            duration_ms: 5000,
            result_json: "{}".to_string(),
            status: "success".to_string(),
        }).expect("save_run failed");
        let row = db.get_run("run-1").expect("get_run failed").expect("row missing");
        assert_eq!(row.status, "success");
        assert!(row.success);

        // A row left 'running' (simulating a crash) is swept to 'interrupted'
        // the next time the database is opened.
        db.save_run_started("run-2", "wf-1", "My Workflow", "2024-01-01T00:01:00Z")
            .expect("save_run_started failed");
        drop(db);

        let db2 = WorkflowDb::open(&path, 8).expect("reopen failed");
        let row1 = db2.get_run("run-1").expect("get_run failed").expect("row missing");
        let row2 = db2.get_run("run-2").expect("get_run failed").expect("row missing");
        assert_eq!(row1.status, "success", "completed run must be untouched");
        assert_eq!(row2.status, "interrupted", "orphaned running row must become interrupted");

        cleanup(&path);
    }

    #[test]
    fn fresh_open_only_warms_one_connection() {
        let path = temp_path("warm_one");
        cleanup(&path);

        let db = WorkflowDb::open(&path, 16).expect("open failed");
        let state = db.pool.state();
        assert_eq!(state.connections, 1, "only one connection should be eagerly warmed on open");

        cleanup(&path);
    }

    #[test]
    fn pool_still_grows_to_max_size_under_concurrent_load() {
        let path = temp_path("grows_under_load");
        cleanup(&path);

        let db = std::sync::Arc::new(WorkflowDb::open(&path, 8).expect("open failed"));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8).map(|_| {
            let db = std::sync::Arc::clone(&db);
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                let conn = db.pool.get().expect("pool.get failed under concurrent load");
                barrier.wait();
                let _: i64 = conn.query_row("SELECT 1", [], |r| r.get(0)).expect("query failed");
            })
        }).collect();
        for h in handles {
            h.join().expect("worker thread panicked");
        }

        assert_eq!(db.pool.state().connections, 8, "pool must still reach max_size once demand requires it");

        cleanup(&path);
    }
}
