//! Workflow CRUD, versions, variables, and settings methods for [`super::WorkflowDb`].

use uuid::Uuid;
use super::{WorkflowDb, WorkflowSummary, VersionRow};
use crate::model::Workflow;

impl WorkflowDb {
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
