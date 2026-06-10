//! Run history persistence methods for [`super::WorkflowDb`].

use super::{WorkflowDb, RunRecord, row_to_run};

impl WorkflowDb {
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

        // Trim to history_limit only when over the limit (> not >=).
        // Using >= would fire on every insert at steady state (count == limit after insert)
        // causing a wasted DELETE + subquery on each run.
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM run_history WHERE workflow_id = ?1",
            rusqlite::params![record.workflow_id],
            |row| row.get(0),
        ).unwrap_or(0);
        if count > self.history_limit {
            conn.execute(
                "DELETE FROM run_history WHERE workflow_id = ?1
                 AND id NOT IN (
                     SELECT id FROM run_history WHERE workflow_id = ?1
                     ORDER BY ran_at DESC LIMIT ?2
                 )",
                rusqlite::params![record.workflow_id, self.history_limit],
            ).map_err(|e| e.to_string())?;
        }
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
        if let Some(sf) = success_filter {
            let mut stmt = conn.prepare(
                "SELECT id, workflow_id, workflow_name, ran_at, success, duration_ms, result_json
                 FROM run_history WHERE workflow_id = ?1 AND success = ?2
                 ORDER BY ran_at DESC LIMIT ?3 OFFSET ?4"
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map(rusqlite::params![workflow_id, sf, limit, offset], row_to_run)
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string());
            rows
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, workflow_id, workflow_name, ran_at, success, duration_ms, result_json
                 FROM run_history WHERE workflow_id = ?1
                 ORDER BY ran_at DESC LIMIT ?2 OFFSET ?3"
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map(rusqlite::params![workflow_id, limit, offset], row_to_run)
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string());
            rows
        }
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
}
