//! Workflow and run-history database.
//!
//! Moved from commands/workflow.rs so the server binary can use it without
//! depending on Tauri. The Tauri app imports this from flowo_engine::db.

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use crate::model::Workflow;
use crate::scheduler::{ScheduledJobRow, SchedulerDb};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id:            String,
    pub workflow_id:   String,
    pub workflow_name: String,
    pub ran_at:        String,
    pub success:       bool,
    pub duration_ms:   i64,
    pub result_json:   String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionRow {
    pub id:          String,
    pub workflow_id: String,
    pub message:     Option<String>,
    pub created_at:  String,
}

fn row_to_run(row: &rusqlite::Row) -> rusqlite::Result<RunRecord> {
    Ok(RunRecord {
        id:            row.get(0)?,
        workflow_id:   row.get(1)?,
        workflow_name: row.get(2)?,
        ran_at:        row.get(3)?,
        success:       row.get::<_, i64>(4)? != 0,
        duration_ms:   row.get(5)?,
        result_json:   row.get(6)?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSummary {
    pub id:         String,
    pub name:       String,
    pub updated_at: String,
}

pub struct WorkflowDb {
    pool: Pool<SqliteConnectionManager>,
    history_limit: i64,
}

impl WorkflowDb {
    /// Current schema version. Increment this and add a `migrate_vN` block
    /// in `run_migrations` for every schema change.
    const SCHEMA_VERSION: i64 = 1;

    pub fn open(path: &PathBuf) -> Result<Self, String> {
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

        let pool = Pool::builder()
            .max_size(8)
            .build(manager)
            .map_err(|e| e.to_string())?;

        {
            let conn = pool.get().map_err(|e| e.to_string())?;
            Self::run_migrations(&conn)?;
        }

        let history_limit = {
            let conn = pool.get().map_err(|e| e.to_string())?;
            let result = conn.query_row(
                "SELECT value FROM settings WHERE key = 'run_history_limit'",
                [],
                |row| row.get::<_, String>(0),
            );
            match result {
                Ok(v) => v.parse::<i64>().unwrap_or(500),
                Err(rusqlite::Error::QueryReturnedNoRows) => 500,
                Err(e) => return Err(e.to_string()),
            }
        };

        Ok(Self { pool, history_limit })
    }

    pub fn save(&self, workflow: &Workflow) -> Result<(), String> {
        let json = workflow.to_json_pretty().map_err(|e| e.to_string())?;
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO workflows (id, name, json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                workflow.id,
                workflow.name,
                json,
                workflow.metadata.created_at.to_rfc3339(),
                chrono::Utc::now().to_rfc3339(),
            ],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<Option<Workflow>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let result = conn.query_row(
            "SELECT json FROM workflows WHERE id = ?1",
            rusqlite::params![id],
            |row| row.get::<_, String>(0),
        );
        match result {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
            Ok(json) => Ok(Some(Workflow::from_json(&json).map_err(|e| e.to_string())?)),
        }
    }

    pub fn list(&self) -> Result<Vec<WorkflowSummary>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, name, updated_at FROM workflows ORDER BY updated_at DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |row| {
            Ok(WorkflowSummary {
                id:         row.get(0)?,
                name:       row.get(1)?,
                updated_at: row.get(2)?,
            })
        }).map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// Returns only workflow IDs — cheaper than `list()` for containment checks.
    /// Used by exec_locks pruning in the server binary.
    pub fn list_ids(&self) -> Result<std::collections::HashSet<String>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id FROM workflows")
            .map_err(|e| e.to_string())?;
        let ids = stmt.query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<std::collections::HashSet<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(ids)
    }

    /// Paginated list with server-side LIMIT/OFFSET. Returns (items, total_count).
    /// Preferred over `list()` for API endpoints to avoid loading all rows into memory.
    pub fn list_paginated(&self, limit: usize, offset: usize) -> Result<(Vec<WorkflowSummary>, usize), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let total: usize = conn
            .query_row("SELECT COUNT(*) FROM workflows", [], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())? as usize;
        let mut stmt = conn
            .prepare("SELECT id, name, updated_at FROM workflows ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map(
            rusqlite::params![limit as i64, offset as i64],
            |row| Ok(WorkflowSummary {
                id:         row.get(0)?,
                name:       row.get(1)?,
                updated_at: row.get(2)?,
            }),
        ).map_err(|e| e.to_string())?;
        let items = rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
        Ok((items, total))
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut conn = self.pool.get().map_err(|e| e.to_string())?;
        // Wrap all three DELETEs in one transaction so a process kill between
        // any two cannot leave orphan rows in workflow_variables or
        // workflow_versions.
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM workflow_variables WHERE workflow_id = ?1", rusqlite::params![id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM workflow_versions WHERE workflow_id = ?1", rusqlite::params![id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM workflows WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn delete_scheduled_job(&self, workflow_id: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM scheduled_jobs WHERE workflow_id = ?1",
            rusqlite::params![workflow_id],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete_runs_for_workflow(&self, workflow_id: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM run_history WHERE workflow_id = ?1",
            rusqlite::params![workflow_id],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_variable(&self, workflow_id: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let result = conn.query_row(
            "SELECT value_json FROM workflow_variables WHERE workflow_id = ?1 AND variable_name = ?2",
            rusqlite::params![workflow_id, name],
            |row| row.get::<_, String>(0),
        );
        match result {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
            Ok(json) => serde_json::from_str(&json).map(Some).map_err(|e| e.to_string()),
        }
    }

    pub fn set_variable(&self, workflow_id: &str, name: &str, value: &serde_json::Value) -> Result<(), String> {
        let json = serde_json::to_string(value).map_err(|e| e.to_string())?;
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO workflow_variables (workflow_id, variable_name, value_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![workflow_id, name, json, chrono::Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete_variables_for_workflow(&self, workflow_id: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM workflow_variables WHERE workflow_id = ?1",
            rusqlite::params![workflow_id],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn save_version(&self, workflow_id: &str, snapshot_json: &str, message: Option<&str>) -> Result<(), String> {
        let id  = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO workflow_versions (id, workflow_id, snapshot, message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, workflow_id, snapshot_json, message, now],
        ).map_err(|e| e.to_string())?;
        // Cap at 50 versions per workflow — keep newest
        conn.execute(
            "DELETE FROM workflow_versions WHERE workflow_id = ?1
             AND id NOT IN (
                 SELECT id FROM workflow_versions WHERE workflow_id = ?1
                 ORDER BY created_at DESC LIMIT 50
             )",
            rusqlite::params![workflow_id, workflow_id],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn list_versions(&self, workflow_id: &str) -> Result<Vec<VersionRow>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, workflow_id, message, created_at FROM workflow_versions
             WHERE workflow_id = ?1 ORDER BY created_at DESC"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![workflow_id], |row| {
            Ok(VersionRow {
                id:          row.get(0)?,
                workflow_id: row.get(1)?,
                message:     row.get(2)?,
                created_at:  row.get(3)?,
            })
        }).map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    pub fn get_version(&self, id: &str) -> Result<Option<String>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let result = conn.query_row(
            "SELECT snapshot FROM workflow_versions WHERE id = ?1",
            rusqlite::params![id],
            |row| row.get::<_, String>(0),
        );
        match result {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
            Ok(snap) => Ok(Some(snap)),
        }
    }

    pub fn delete_version(&self, id: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM workflow_versions WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn save_run(&self, record: &RunRecord) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO run_history
             (id, workflow_id, workflow_name, ran_at, success, duration_ms, result_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                record.id,
                record.workflow_id,
                record.workflow_name,
                record.ran_at,
                record.success as i64,
                record.duration_ms,
                record.result_json,
            ],
        ).map_err(|e| e.to_string())?;

        // Keep only the most recent runs per workflow (limit from settings, default 500)
        conn.execute(
            "DELETE FROM run_history WHERE workflow_id = ?1
             AND id NOT IN (
                 SELECT id FROM run_history WHERE workflow_id = ?1
                 ORDER BY ran_at DESC LIMIT ?2
             )",
            rusqlite::params![record.workflow_id, self.history_limit],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn list_runs(
        &self,
        workflow_id: &str,
        offset:      i64,
        limit:       i64,
        filter:      &str,
    ) -> Result<Vec<RunRecord>, String> {
        let limit  = limit.clamp(1, 1000);
        let offset = offset.max(0);
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let success_filter = match filter {
            "success" => Some(1i64),
            "failed"  => Some(0i64),
            _         => None,
        };
        let rows: Vec<RunRecord> = if let Some(sf) = success_filter {
            let mut stmt = conn.prepare(
                "SELECT id, workflow_id, workflow_name, ran_at, success, duration_ms, result_json
                 FROM run_history WHERE workflow_id = ?1 AND success = ?2
                 ORDER BY ran_at DESC LIMIT ?3 OFFSET ?4"
            ).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params![workflow_id, sf, limit, offset], row_to_run)
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, workflow_id, workflow_name, ran_at, success, duration_ms, result_json
                 FROM run_history WHERE workflow_id = ?1
                 ORDER BY ran_at DESC LIMIT ?2 OFFSET ?3"
            ).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params![workflow_id, limit, offset], row_to_run)
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        Ok(rows)
    }

    pub fn get_run(&self, id: &str) -> Result<Option<RunRecord>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let result = conn.query_row(
            "SELECT id, workflow_id, workflow_name, ran_at, success, duration_ms, result_json
             FROM run_history WHERE id = ?1",
            rusqlite::params![id],
            row_to_run,
        );
        match result {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
            Ok(row) => Ok(Some(row)),
        }
    }

    pub fn delete_run(&self, id: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM run_history WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn clear_runs(&self, workflow_id: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM run_history WHERE workflow_id = ?1", rusqlite::params![workflow_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let result = conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            rusqlite::params![key],
            |row| row.get::<_, String>(0),
        );
        match result {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
            Ok(v) => Ok(Some(v)),
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Applies all pending schema migrations in order.
    ///
    /// To add a future migration:
    ///   1. Increment `SCHEMA_VERSION`.
    ///   2. Add `if current_version < N { Self::migrate_vN(conn)?; }` below.
    ///   3. Write `migrate_vN` using only additive DDL (ADD COLUMN, CREATE TABLE,
    ///      CREATE INDEX). Never DROP or RENAME — those require data migration logic.
    ///   4. Wrap DDL + `PRAGMA user_version = N` in a single transaction so that
    ///      a mid-migration crash leaves user_version unchanged and the next launch
    ///      retries cleanly.
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

        Ok(())
    }

    /// Version 1 — initial release baseline.
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
}

impl SchedulerDb for WorkflowDb {
    fn scheduler_list_all(&self) -> Result<Vec<ScheduledJobRow>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT workflow_id, workflow_name, trigger_kind, status, always_on,
                    run_count, last_run_at, next_run_at, last_error, created_at
             FROM scheduled_jobs ORDER BY created_at DESC"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |row| {
            Ok(ScheduledJobRow {
                workflow_id:   row.get(0)?,
                workflow_name: row.get(1)?,
                trigger_kind:  row.get(2)?,
                status:        row.get(3)?,
                always_on:     row.get::<_, i64>(4)? != 0,
                run_count:     row.get(5)?,
                last_run_at:   row.get(6)?,
                next_run_at:   row.get(7)?,
                last_error:    row.get(8)?,
                created_at:    row.get(9)?,
            })
        }).map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    fn scheduler_list_active(&self) -> Result<Vec<ScheduledJobRow>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT workflow_id, workflow_name, trigger_kind, status, always_on,
                    run_count, last_run_at, next_run_at, last_error, created_at
             FROM scheduled_jobs WHERE status = 'active'"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |row| {
            Ok(ScheduledJobRow {
                workflow_id:   row.get(0)?,
                workflow_name: row.get(1)?,
                trigger_kind:  row.get(2)?,
                status:        row.get(3)?,
                always_on:     row.get::<_, i64>(4)? != 0,
                run_count:     row.get(5)?,
                last_run_at:   row.get(6)?,
                next_run_at:   row.get(7)?,
                last_error:    row.get(8)?,
                created_at:    row.get(9)?,
            })
        }).map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    fn scheduler_upsert(&self, row: &ScheduledJobRow) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO scheduled_jobs
             (workflow_id, workflow_name, trigger_kind, status, always_on,
              run_count, last_run_at, next_run_at, last_error, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                row.workflow_id, row.workflow_name, row.trigger_kind, row.status,
                row.always_on as i64, row.run_count, row.last_run_at, row.next_run_at,
                row.last_error, row.created_at,
            ],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn scheduler_update_run(
        &self, workflow_id: &str, success: bool, error_msg: Option<&str>, next_run_at: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE scheduled_jobs
             SET run_count   = run_count + 1,
                 last_run_at = ?2,
                 last_error  = ?3,
                 next_run_at = COALESCE(?4, next_run_at)
             WHERE workflow_id = ?1",
            rusqlite::params![
                workflow_id,
                chrono::Utc::now().to_rfc3339(),
                if success { None } else { error_msg },
                next_run_at,
            ],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn scheduler_set_status(&self, workflow_id: &str, status: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE scheduled_jobs SET status = ?2 WHERE workflow_id = ?1",
            rusqlite::params![workflow_id, status],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn scheduler_clear_next_run(&self, workflow_id: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE scheduled_jobs SET next_run_at = NULL WHERE workflow_id = ?1",
            rusqlite::params![workflow_id],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn scheduler_update_next_run_at(&self, workflow_id: &str, next_run_at: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE scheduled_jobs SET next_run_at = ?2 WHERE workflow_id = ?1",
            rusqlite::params![workflow_id, next_run_at],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn scheduler_get(&self, workflow_id: &str) -> Result<Option<ScheduledJobRow>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let result = conn.query_row(
            "SELECT workflow_id, workflow_name, trigger_kind, status, always_on,
                    run_count, last_run_at, next_run_at, last_error, created_at
             FROM scheduled_jobs WHERE workflow_id = ?1",
            rusqlite::params![workflow_id],
            |row| Ok(ScheduledJobRow {
                workflow_id:   row.get(0)?,
                workflow_name: row.get(1)?,
                trigger_kind:  row.get(2)?,
                status:        row.get(3)?,
                always_on:     row.get::<_, i64>(4)? != 0,
                run_count:     row.get(5)?,
                last_run_at:   row.get(6)?,
                next_run_at:   row.get(7)?,
                last_error:    row.get(8)?,
                created_at:    row.get(9)?,
            }),
        );
        match result {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
            Ok(row) => Ok(Some(row)),
        }
    }

    fn load_workflow_json(&self, workflow_id: &str) -> Result<Option<String>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let result = conn.query_row(
            "SELECT json FROM workflows WHERE id = ?1",
            rusqlite::params![workflow_id],
            |row| row.get::<_, String>(0),
        );
        match result {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
            Ok(json) => Ok(Some(json)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("flowo_db_test_{}.db", tag));
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

        let db = WorkflowDb::open(&path).expect("open failed");
        let conn = db.pool.get().unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 1);

        cleanup(&path);
    }

    #[test]
    fn test_existing_db_migrates_and_preserves_data() {
        let path = temp_path("existing");
        cleanup(&path);

        // Simulate a pre-migration DB: create tables manually, leave user_version = 0.
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
            // user_version intentionally left at 0
        }

        let db = WorkflowDb::open(&path).expect("migration failed");

        let conn = db.pool.get().unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 1, "must be migrated to v1");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM workflows", [], |r| r.get::<_, i64>(0))
            .unwrap();
        assert_eq!(count, 1, "pre-existing row must survive migration");

        cleanup(&path);
    }

    #[test]
    fn test_open_twice_is_idempotent() {
        let path = temp_path("idempotent");
        cleanup(&path);

        WorkflowDb::open(&path).expect("first open failed");
        WorkflowDb::open(&path).expect("second open must not fail");

        cleanup(&path);
    }
}
