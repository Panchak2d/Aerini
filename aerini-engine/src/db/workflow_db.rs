//! Workflow CRUD, versions, variables, and settings methods for [`super::WorkflowDb`].

use uuid::Uuid;
use super::{WorkflowDb, WorkflowSummary, VersionRow};
use crate::model::Workflow;

impl WorkflowDb {
    pub fn save(&self, workflow: &Workflow) -> Result<(), String> {
        let json = workflow.to_json_pretty().map_err(|e| e.to_string())?;
        let tags = serde_json::to_string(&workflow.metadata.tags).map_err(|e| e.to_string())?;
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO workflows (id, name, json, created_at, updated_at, tags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                workflow.id,
                workflow.name,
                json,
                workflow.metadata.created_at.to_rfc3339(),
                chrono::Utc::now().to_rfc3339(),
                tags,
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
            .prepare("SELECT id, name, updated_at, tags FROM workflows ORDER BY updated_at DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |row| {
            let tags_json: String = row.get(3)?;
            Ok(WorkflowSummary {
                id:         row.get(0)?,
                name:       row.get(1)?,
                updated_at: row.get(2)?,
                tags:       serde_json::from_str(&tags_json).unwrap_or_default(),
            })
        }).map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// Returns only workflow IDs — cheaper than `list()` for containment checks.
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
    pub fn list_paginated(&self, limit: usize, offset: usize) -> Result<(Vec<WorkflowSummary>, usize), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let total: usize = conn
            .query_row("SELECT COUNT(*) FROM workflows", [], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())? as usize;
        let mut stmt = conn
            .prepare("SELECT id, name, updated_at, tags FROM workflows ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map(
            rusqlite::params![limit as i64, offset as i64],
            |row| {
                let tags_json: String = row.get(3)?;
                Ok(WorkflowSummary {
                    id:         row.get(0)?,
                    name:       row.get(1)?,
                    updated_at: row.get(2)?,
                    tags:       serde_json::from_str(&tags_json).unwrap_or_default(),
                })
            },
        ).map_err(|e| e.to_string())?;
        let items = rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
        Ok((items, total))
    }

    /// Deletes a workflow and ALL associated data atomically.
    ///
    /// Covers: workflow_variables, run_history, scheduled_jobs, and the workflows row.
    /// workflow_versions are handled by the FK ON DELETE CASCADE on the workflows row.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut conn = self.pool.get().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        // workflow_variables: no FK cascade — must delete explicitly.
        tx.execute("DELETE FROM workflow_variables WHERE workflow_id = ?1", rusqlite::params![id])
            .map_err(|e| e.to_string())?;
        // run_history and scheduled_jobs: no FK cascade — must delete explicitly.
        tx.execute("DELETE FROM run_history WHERE workflow_id = ?1", rusqlite::params![id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM scheduled_jobs WHERE workflow_id = ?1", rusqlite::params![id])
            .map_err(|e| e.to_string())?;
        // workflow_versions: FK ON DELETE CASCADE fires when the workflows row is deleted below.
        tx.execute("DELETE FROM workflows WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// Deletes only the scheduled_job row for a workflow.
    ///
    /// Prefer `delete()` when removing a workflow entirely — it covers all associated
    /// data in a single transaction. Use this only when unscheduling without deleting
    /// the workflow itself.
    pub fn delete_scheduled_job(&self, workflow_id: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM scheduled_jobs WHERE workflow_id = ?1",
            rusqlite::params![workflow_id],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Deletes all run history for a workflow.
    ///
    /// Prefer `delete()` when removing a workflow entirely. Use this only when
    /// clearing history while keeping the workflow itself.
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
        let conn = self.pool.get().map_err(|e| e.to_string())?;

        // Skip if identical to the most recent version for this workflow —
        // avoids piling up no-op rows from duplicate/repeated saves.
        let last_snapshot: Option<String> = match conn.query_row(
            "SELECT snapshot FROM workflow_versions WHERE workflow_id = ?1
             ORDER BY created_at DESC LIMIT 1",
            rusqlite::params![workflow_id],
            |row| row.get::<_, String>(0),
        ) {
            Ok(s) => Some(s),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.to_string()),
        };
        if last_snapshot.as_deref() == Some(snapshot_json) {
            return Ok(());
        }

        let id  = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO workflow_versions (id, workflow_id, snapshot, message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, workflow_id, snapshot_json, message, now],
        ).map_err(|e| e.to_string())?;

        // Trim to version_retention_limit only when over the limit (> not >=),
        // same shape as run_history.rs / performance_reports.rs.
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM workflow_versions WHERE workflow_id = ?1",
            rusqlite::params![workflow_id],
            |row| row.get(0),
        ).unwrap_or(0);
        if count > self.version_retention_limit {
            conn.execute(
                "DELETE FROM workflow_versions WHERE workflow_id = ?1
                 AND id NOT IN (
                     SELECT id FROM workflow_versions WHERE workflow_id = ?1
                     ORDER BY created_at DESC LIMIT ?2
                 )",
                rusqlite::params![workflow_id, self.version_retention_limit],
            ).map_err(|e| e.to_string())?;
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Workflow;
    use std::path::PathBuf;

    fn temp_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aerini_workflow_versions_test_{}.db", tag));
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

    // workflow_versions has a real FK (ON DELETE CASCADE) to workflows(id),
    // and foreign_keys=ON is set on every pooled connection (see
    // WorkflowDb::open's with_init) — unlike performance_reports/chat_sessions,
    // which have no such constraint, a version row can't be inserted for a
    // workflow_id that isn't actually present in the workflows table.
    fn seed_workflow(db: &WorkflowDb, id: &str, name: &str) {
        db.save(&Workflow::new(id, name)).expect("seed workflow save failed");
    }

    // Normal case: save then list/get — message and snapshot round-trip,
    // and a lookup by an unknown id comes back None rather than erroring.
    #[test]
    fn save_list_and_get_version_round_trips_message_and_snapshot() {
        let path = temp_path("roundtrip");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        seed_workflow(&db, "wf-1", "My Workflow");

        db.save_version("wf-1", "{\"nodes\":[]}", Some("Checkpoint")).expect("save failed");

        let versions = db.list_versions("wf-1").expect("list failed");
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].workflow_id, "wf-1");
        assert_eq!(versions[0].message.as_deref(), Some("Checkpoint"));

        let snapshot = db.get_version(&versions[0].id).expect("get failed").expect("row missing");
        assert_eq!(snapshot, "{\"nodes\":[]}");

        assert!(db.get_version("no-such-id").expect("get failed").is_none());

        cleanup(&path);
    }

    #[test]
    fn save_and_list_round_trip_tags() {
        let path = temp_path("tags_roundtrip");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        let mut wf = Workflow::new("wf-1", "My Workflow");
        wf.metadata.tags = vec!["prod".to_string(), "webhook".to_string()];
        db.save(&wf).expect("save failed");

        let summaries = db.list().expect("list failed");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].tags, vec!["prod".to_string(), "webhook".to_string()]);

        cleanup(&path);
    }

    #[test]
    fn list_defaults_empty_tags_for_workflow_saved_with_none() {
        let path = temp_path("tags_default");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        seed_workflow(&db, "wf-1", "My Workflow");

        let summaries = db.list().expect("list failed");
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].tags.is_empty());

        cleanup(&path);
    }

    #[test]
    fn save_version_stores_null_message_when_none_given() {
        let path = temp_path("null_message");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        seed_workflow(&db, "wf-1", "My Workflow");

        db.save_version("wf-1", "{\"nodes\":[]}", None).expect("save failed");

        let versions = db.list_versions("wf-1").expect("list failed");
        assert_eq!(versions[0].message, None);

        cleanup(&path);
    }

    #[test]
    fn list_versions_orders_newest_first_and_scopes_by_workflow() {
        let path = temp_path("ordering");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        seed_workflow(&db, "wf-1", "A");
        seed_workflow(&db, "wf-2", "B");

        db.save_version("wf-1", "{\"v\":1}", Some("first")).unwrap();
        db.save_version("wf-1", "{\"v\":2}", Some("second")).unwrap();
        db.save_version("wf-2", "{\"v\":1}", Some("other workflow")).unwrap();

        let wf1 = db.list_versions("wf-1").expect("list failed");
        assert_eq!(wf1.len(), 2);
        assert_eq!(wf1[0].message.as_deref(), Some("second"), "must be newest-first");
        assert_eq!(wf1[1].message.as_deref(), Some("first"));
        assert_eq!(db.list_versions("wf-2").expect("list failed").len(), 1);

        cleanup(&path);
    }

    // Dedup: an identical snapshot saved twice in a row is skipped, not
    // inserted as a second row.
    #[test]
    fn save_version_skips_an_identical_consecutive_snapshot() {
        let path = temp_path("dedup");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        seed_workflow(&db, "wf-1", "A");

        db.save_version("wf-1", "{\"v\":1}", Some("first")).unwrap();
        db.save_version("wf-1", "{\"v\":1}", Some("first")).unwrap();

        assert_eq!(db.list_versions("wf-1").expect("list failed").len(), 1, "identical repeat must be skipped");

        cleanup(&path);
    }

    // Edge case: dedup only compares against the single most recent row, not
    // the whole history — re-saving an earlier snapshot after a different one
    // in between must insert, not be treated as a repeat.
    #[test]
    fn save_version_does_not_dedupe_against_snapshot_before_the_most_recent() {
        let path = temp_path("dedup_not_transitive");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        seed_workflow(&db, "wf-1", "A");

        db.save_version("wf-1", "{\"v\":1}", None).unwrap(); // A
        db.save_version("wf-1", "{\"v\":2}", None).unwrap(); // B (different from A)
        db.save_version("wf-1", "{\"v\":1}", None).unwrap(); // A again — most recent is B, not A

        assert_eq!(db.list_versions("wf-1").expect("list failed").len(), 3);

        cleanup(&path);
    }

    // Retention: trims to version_retention_limit only once the count exceeds
    // it, keeping the newest N — same "> not >=" shape as
    // performance_reports.rs's own retention test. Pre-populate the setting
    // and reopen (version_retention_limit is read once at open()), rather
    // than saving 51 real rows to exercise the default.
    #[test]
    fn save_version_trims_to_retention_limit_keeping_newest() {
        let path = temp_path("retention");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        seed_workflow(&db, "wf-trim", "A");
        {
            let conn = db.pool.get().unwrap();
            conn.execute("INSERT INTO settings (key, value) VALUES ('version_retention_limit', '3')", [])
                .unwrap();
        }
        drop(db);
        let db = WorkflowDb::open(&path, 8).expect("reopen to pick up limit=3 failed");

        for i in 0..5 {
            db.save_version("wf-trim", &format!("{{\"v\":{i}}}"), Some(&format!("snap-{i}"))).unwrap();
        }

        let remaining = db.list_versions("wf-trim").expect("list failed");
        assert_eq!(remaining.len(), 3, "must be trimmed to version_retention_limit=3");
        let messages: Vec<Option<&str>> = remaining.iter().map(|v| v.message.as_deref()).collect();
        assert_eq!(messages, vec![Some("snap-4"), Some("snap-3"), Some("snap-2")], "oldest two must be evicted");

        cleanup(&path);
    }

    #[test]
    fn delete_version_removes_only_that_row() {
        let path = temp_path("delete");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        seed_workflow(&db, "wf-1", "A");

        db.save_version("wf-1", "{\"v\":1}", Some("keep")).unwrap();
        db.save_version("wf-1", "{\"v\":2}", Some("delete-me")).unwrap();
        let versions = db.list_versions("wf-1").unwrap();
        let to_delete = versions.iter().find(|v| v.message.as_deref() == Some("delete-me")).unwrap().id.clone();

        db.delete_version(&to_delete).expect("delete failed");

        let remaining = db.list_versions("wf-1").expect("list failed");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].message.as_deref(), Some("keep"));

        cleanup(&path);
    }

    // Cascade: deleting the workflow row removes its versions via the FK's
    // ON DELETE CASCADE — delete() itself never touches workflow_versions
    // directly (see its own doc comment), so this is exercising the schema's
    // cascade, not application code.
    #[test]
    fn deleting_workflow_cascades_its_versions() {
        let path = temp_path("cascade");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        seed_workflow(&db, "wf-1", "A");
        seed_workflow(&db, "wf-2", "B");

        db.save_version("wf-1", "{\"v\":1}", None).unwrap();
        db.save_version("wf-2", "{\"v\":1}", None).unwrap();

        db.delete("wf-1").expect("delete failed");

        assert!(db.list_versions("wf-1").expect("list failed").is_empty(), "versions must cascade-delete with their workflow");
        assert_eq!(db.list_versions("wf-2").expect("list failed").len(), 1, "a different workflow's versions must be untouched");

        cleanup(&path);
    }
}
