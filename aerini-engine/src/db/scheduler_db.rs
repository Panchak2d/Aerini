//! `SchedulerDb` trait implementation for [`super::WorkflowDb`].

use super::WorkflowDb;
use crate::scheduler::{ScheduledJobRow, SchedulerDb};

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

    fn scheduler_list_paginated(&self, offset: usize, limit: usize) -> Result<(Vec<ScheduledJobRow>, usize), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let total: usize = conn.query_row(
            "SELECT COUNT(*) FROM scheduled_jobs",
            [],
            |r| r.get::<_, i64>(0),
        ).map_err(|e| e.to_string())? as usize;
        let mut stmt = conn.prepare(
            "SELECT workflow_id, workflow_name, trigger_kind, status, always_on,
                    run_count, last_run_at, next_run_at, last_error, created_at
             FROM scheduled_jobs ORDER BY created_at DESC LIMIT ?1 OFFSET ?2"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(
            rusqlite::params![limit as i64, offset as i64],
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
        ).map_err(|e| e.to_string())?;
        let items = rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
        Ok((items, total))
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
