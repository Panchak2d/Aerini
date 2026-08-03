//! Performance-report persistence methods for [`super::WorkflowDb`].


use super::{WorkflowDb, PerformanceReportRecord};
use crate::perf_monitor::{HistorySample, PerfStatus, PerformanceReport};

fn row_to_report(row: &rusqlite::Row) -> rusqlite::Result<PerformanceReportRecord> {
    let status_str: String = row.get(2)?;
    let status = match status_str.as_str() {
        "running" => PerfStatus::Running,
        "success" => PerfStatus::Success,
        // This table is only ever written via `save_performance_report`,
        // which stores `PerfStatus::as_db_str()` verbatim — "failed" is the
        // only remaining variant, so any other value here would mean the
        // row was written by something else. Falls back to `Failed` rather
        // than panicking on a row this code didn't produce.
        _ => PerfStatus::Failed,
    };
    let history_json: String = row.get(15)?;
    // Deliberate fallback, not a silently swallowed error: every other
    // column above still propagates a real parse/type error via `?` and
    // fails the whole read. Only this one nested JSON field degrades to an
    // empty series instead — a report otherwise fully intact but with an
    // unreadable sample history is still useful (status/peak/delta/etc. all
    // still readable); failing the entire row over that one field would
    // throw away more than it protects.
    let history: Vec<HistorySample> = serde_json::from_str(&history_json).unwrap_or_default();

    Ok(PerformanceReportRecord {
        id: row.get(0)?,
        report: PerformanceReport {
            workflow_id:          row.get(1)?,
            status,
            started_at_ms:        row.get::<_, i64>(3)? as u64,
            finished_at_ms:       row.get::<_, i64>(4)? as u64,
            duration_ms:          row.get::<_, i64>(5)? as u64,
            sampling_interval_ms: row.get::<_, i64>(6)? as u64,
            baseline_bytes:       row.get::<_, i64>(7)? as u64,
            final_bytes:          row.get::<_, i64>(8)? as u64,
            peak_bytes:           row.get::<_, i64>(9)? as u64,
            peak_at_ms:           row.get::<_, i64>(10)? as u64,
            minimum_bytes:        row.get::<_, i64>(11)? as u64,
            average_bytes:        row.get::<_, i64>(12)? as u64,
            delta_bytes:          row.get(13)?,
            sample_count:         row.get::<_, i64>(14)? as u64,
            history,
        },
    })
}

impl WorkflowDb {
    /// Persists `report` under `run_id` 
    pub fn save_performance_report(&self, run_id: &str, report: &PerformanceReport) -> Result<(), String> {
        let history_json = serde_json::to_string(&report.history).map_err(|e| e.to_string())?;
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO performance_reports
             (id, workflow_id, status, started_at_ms, finished_at_ms, duration_ms,
              sampling_interval_ms, baseline_bytes, final_bytes, peak_bytes, peak_at_ms,
              minimum_bytes, average_bytes, delta_bytes, sample_count, history_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            rusqlite::params![
                run_id,
                report.workflow_id,
                report.status.as_db_str(),
                report.started_at_ms as i64,
                report.finished_at_ms as i64,
                report.duration_ms as i64,
                report.sampling_interval_ms as i64,
                report.baseline_bytes as i64,
                report.final_bytes as i64,
                report.peak_bytes as i64,
                report.peak_at_ms as i64,
                report.minimum_bytes as i64,
                report.average_bytes as i64,
                report.delta_bytes,
                report.sample_count as i64,
                history_json,
            ],
        ).map_err(|e| e.to_string())?;

        // Same trim-only-when-over-limit shape as `save_run` Reuses
        // `self.history_limit` rather than adding a second, separate
        // setting.
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM performance_reports WHERE workflow_id = ?1",
            rusqlite::params![report.workflow_id],
            |row| row.get(0),
        ).unwrap_or(0);
        if count > self.history_limit {
            conn.execute(
                "DELETE FROM performance_reports WHERE workflow_id = ?1
                 AND id NOT IN (
                     SELECT id FROM performance_reports WHERE workflow_id = ?1
                     ORDER BY started_at_ms DESC LIMIT ?2
                 )",
                rusqlite::params![report.workflow_id, self.history_limit],
            ).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn get_performance_report(&self, run_id: &str) -> Result<Option<PerformanceReportRecord>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let result = conn.query_row(
            "SELECT id, workflow_id, status, started_at_ms, finished_at_ms, duration_ms,
                    sampling_interval_ms, baseline_bytes, final_bytes, peak_bytes, peak_at_ms,
                    minimum_bytes, average_bytes, delta_bytes, sample_count, history_json
             FROM performance_reports WHERE id = ?1",
            rusqlite::params![run_id],
            row_to_report,
        );
        match result {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
            Ok(row) => Ok(Some(row)),
        }
    }

    pub fn list_performance_reports(
        &self,
        workflow_id: &str,
        offset:      i64,
        limit:       i64,
    ) -> Result<Vec<PerformanceReportRecord>, String> {
        let limit  = limit.clamp(1, 1000);
        let offset = offset.max(0);
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, workflow_id, status, started_at_ms, finished_at_ms, duration_ms,
                    sampling_interval_ms, baseline_bytes, final_bytes, peak_bytes, peak_at_ms,
                    minimum_bytes, average_bytes, delta_bytes, sample_count, history_json
             FROM performance_reports WHERE workflow_id = ?1
             ORDER BY started_at_ms DESC LIMIT ?2 OFFSET ?3"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![workflow_id, limit, offset], row_to_report)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    pub fn delete_performance_report(&self, run_id: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM performance_reports WHERE id = ?1", rusqlite::params![run_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn clear_performance_reports(&self, workflow_id: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM performance_reports WHERE workflow_id = ?1", rusqlite::params![workflow_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perf_monitor::HistorySample;
    use std::path::PathBuf;

    fn temp_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aerini_perf_reports_test_{}.db", tag));
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

    fn sample_report(workflow_id: &str, started_at_ms: u64) -> PerformanceReport {
        PerformanceReport {
            workflow_id: workflow_id.to_string(),
            status: PerfStatus::Success,
            started_at_ms,
            finished_at_ms: started_at_ms + 500,
            duration_ms: 500,
            sampling_interval_ms: 150,
            baseline_bytes: 1000,
            final_bytes: 1200,
            peak_bytes: 1500,
            peak_at_ms: started_at_ms + 200,
            minimum_bytes: 900,
            average_bytes: 1100,
            delta_bytes: 200,
            sample_count: 4,
            history: vec![
                HistorySample { at_ms: started_at_ms, bytes: 1000 },
                HistorySample { at_ms: started_at_ms + 150, bytes: 1500 },
            ],
        }
    }

    // Normal case: save then read back by id — every field round-trips,
    // including the signed delta and the JSON-blob history series.
    #[test]
    fn save_and_get_performance_report_round_trips_all_fields() {
        let path = temp_path("roundtrip");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        let report = sample_report("wf-1", 1_000);
        db.save_performance_report("run-1", &report).expect("save failed");

        let row = db.get_performance_report("run-1").expect("get failed").expect("row missing");
        assert_eq!(row.id, "run-1");
        assert_eq!(row.report.workflow_id, "wf-1");
        assert_eq!(row.report.status, PerfStatus::Success);
        assert_eq!(row.report.peak_bytes, 1500);
        assert_eq!(row.report.delta_bytes, 200);
        assert_eq!(row.report.history.len(), 2);
        assert_eq!(row.report.history[1].bytes, 1500);

        assert!(db.get_performance_report("no-such-run").expect("get failed").is_none());

        cleanup(&path);
    }

    // Edge case: retention trims oldest-by-started_at_ms once a workflow's
    // report count exceeds history_limit, same "> not >=" shape as
    // run_history's own test intent.
    #[test]
    fn save_performance_report_trims_to_history_limit_per_workflow() {
        let path = temp_path("retention");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        // history_limit defaults to 500 (no 'run_history_limit' setting
        // row), write past a small threshold to exercise the trim without
        // waiting on the default; verifies the SQL, not the default value.
        // Reuse the same 'settings' row `save_run`'s retention already
        // reads, so no new setting key is introduced.
        {
            let conn = db.pool.get().unwrap();
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('run_history_limit', '3')",
                [],
            ).unwrap();
        }
        drop(db); // matches db/mod.rs's own reopen-to-pick-up-persisted-state convention

        let db = WorkflowDb::open(&path, 8).expect("reopen to pick up limit=3 failed");

        for i in 0..5u64 {
            let report = sample_report("wf-trim", 1_000 + i);
            db.save_performance_report(&format!("run-{i}"), &report).expect("save failed");
        }

        let remaining = db.list_performance_reports("wf-trim", 0, 100).expect("list failed");
        assert_eq!(remaining.len(), 3, "must be trimmed to history_limit=3");
        // Newest-first, oldest two (run-0, run-1) evicted.
        let ids: Vec<&str> = remaining.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["run-4", "run-3", "run-2"]);

        cleanup(&path);
    }

    #[test]
    fn delete_and_clear_performance_reports() {
        let path = temp_path("delete_clear");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        db.save_performance_report("run-a", &sample_report("wf-2", 1_000)).unwrap();
        db.save_performance_report("run-b", &sample_report("wf-2", 2_000)).unwrap();
        db.save_performance_report("run-c", &sample_report("wf-3", 3_000)).unwrap();

        db.delete_performance_report("run-a").unwrap();
        assert!(db.get_performance_report("run-a").unwrap().is_none());
        assert_eq!(db.list_performance_reports("wf-2", 0, 100).unwrap().len(), 1);

        db.clear_performance_reports("wf-2").unwrap();
        assert!(db.list_performance_reports("wf-2", 0, 100).unwrap().is_empty());
        // A different workflow's reports are untouched by clearing wf-2's.
        assert_eq!(db.list_performance_reports("wf-3", 0, 100).unwrap().len(), 1);

        cleanup(&path);
    }
}
