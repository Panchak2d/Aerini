//! Run history persistence methods for [`super::WorkflowDb`].

use super::{WorkflowDb, RunRecord, RunSummary, row_to_run};
use serde_json::{json, Value};

/// Key of the marker object that stands in for `body.attachments` when it is
/// byte-for-byte the sibling `files` array of the same node output.
const SAME_AS_KEY: &str = "$same_as";

/// Root-level key listing the node ids whose `body.attachments` was replaced
/// by the marker. The root of a stored result is engine-built, so unlike the
/// node bodies (which carry arbitrary webhook input) it cannot already hold
/// this key; expansion only touches listed nodes, so a caller-supplied
/// `{"$same_as": "files"}` inside a body is never rewritten.
const COMPACTED_KEY: &str = "$compacted";

/// A Webhook trigger fed chat attachments emits them twice: under
/// `body.attachments` and under `files`. Both are base64, so every stored run
/// would carry the payload twice. Swaps the `body.attachments` copy for a
/// marker; [`expand_result_json`] restores it on read, so callers never see
/// the marker. Anything that doesn't match exactly is stored untouched.
pub(super) fn compact_result_json(raw: &str) -> String {
    if !raw.contains("\"attachments\"") {
        return raw.to_string();
    }
    let Ok(mut root) = serde_json::from_str::<Value>(raw) else {
        return raw.to_string();
    };
    if root.get(COMPACTED_KEY).is_some() {
        return raw.to_string();
    }
    let mut compacted: Vec<Value> = Vec::new();
    if let Some(outputs) = root.get_mut("node_outputs").and_then(Value::as_object_mut) {
        for (node_id, out) in outputs.iter_mut() {
            let Some(obj) = out.as_object_mut() else { continue };
            let duplicated = match (obj.get("files"), obj.get("body").and_then(|b| b.get("attachments"))) {
                (Some(files), Some(attachments)) => files.is_array() && files == attachments,
                _ => false,
            };
            if !duplicated {
                continue;
            }
            if let Some(body) = obj.get_mut("body").and_then(Value::as_object_mut) {
                body.insert("attachments".to_string(), json!({ SAME_AS_KEY: "files" }));
                compacted.push(Value::String(node_id.clone()));
            }
        }
    }
    if compacted.is_empty() {
        return raw.to_string();
    }
    if let Some(map) = root.as_object_mut() {
        map.insert(COMPACTED_KEY.to_string(), Value::Array(compacted));
    }
    serde_json::to_string(&root).unwrap_or_else(|_| raw.to_string())
}

/// Inverse of [`compact_result_json`]. A listed node whose marker or sibling
/// `files` is missing is left as-is rather than guessed at.
pub(super) fn expand_result_json(raw: String) -> String {
    if !raw.contains(COMPACTED_KEY) {
        return raw;
    }
    let Ok(mut root) = serde_json::from_str::<Value>(&raw) else {
        return raw;
    };
    let Some(Value::Array(compacted)) = root.as_object_mut().and_then(|m| m.remove(COMPACTED_KEY)) else {
        return raw;
    };
    if let Some(outputs) = root.get_mut("node_outputs").and_then(Value::as_object_mut) {
        for node_id in compacted.iter().filter_map(Value::as_str) {
            let Some(obj) = outputs.get_mut(node_id).and_then(Value::as_object_mut) else { continue };
            let is_marker = obj
                .get("body")
                .and_then(|b| b.get("attachments"))
                .and_then(|a| a.get(SAME_AS_KEY))
                .and_then(Value::as_str)
                == Some("files");
            if !is_marker {
                continue;
            }
            let Some(files) = obj.get("files").filter(|f| f.is_array()).cloned() else { continue };
            if let Some(body) = obj.get_mut("body").and_then(Value::as_object_mut) {
                body.insert("attachments".to_string(), files);
            }
        }
    }
    serde_json::to_string(&root).unwrap_or(raw)
}

/// Largest result document kept for one run. Matches the 32 MiB bound on
/// Database query results: a stored run is read back whole by every history
/// listing and by the desktop app's IPC, so one run must stay a bounded cost.
pub(super) const MAX_STORED_RESULT_BYTES: usize = 32 * 1024 * 1024;

/// Key of the marker that stands in for a node output dropped to fit
/// [`MAX_STORED_RESULT_BYTES`]. Also set at the root of a result that had to be
/// reduced to its summary.
const TRUNCATED_KEY: &str = "$truncated";

/// Log entries kept when a result still exceeds the limit after its node
/// outputs were dropped.
const KEPT_LOG_ENTRIES: usize = 1_000;

fn truncated_marker(original_bytes: usize) -> Value {
    json!({ TRUNCATED_KEY: { "original_bytes": original_bytes } })
}

/// Result bytes one `list_runs` call hands back. A page of runs near the
/// per-run limit would otherwise load, serialize and ship gigabytes at once.
pub(super) const MAX_LISTED_RESULT_BYTES: usize = 64 * 1024 * 1024;

/// Counts `run`'s result against a listing's `budget`. The first row is always
/// kept whole. A later row that would push `used` past the budget keeps its
/// identity and outcome but carries a [`TRUNCATED_KEY`] marker instead of the
/// result; `get_run` still returns it whole.
pub(super) fn within_listing_budget(used: &mut usize, budget: usize, mut run: RunRecord) -> RunRecord {
    let size = run.result_json.len();
    if *used > 0 && used.saturating_add(size) > budget {
        run.result_json = truncated_marker(size).to_string();
    } else {
        *used += size;
    }
    run
}

/// Keeps a stored result within `cap` bytes. Drops the largest node outputs
/// first, replacing each with a [`TRUNCATED_KEY`] marker; if the document is
/// still too large, keeps only the last [`KEPT_LOG_ENTRIES`] log entries, and
/// as a last resort reduces it to its identifiers and success flag.
pub(super) fn bound_result_json(raw: String, cap: usize) -> String {
    if raw.len() <= cap {
        return raw;
    }
    let original_bytes = raw.len();
    let Ok(mut root) = serde_json::from_str::<Value>(&raw) else {
        return truncated_marker(original_bytes).to_string();
    };
    if !root.is_object() {
        return truncated_marker(original_bytes).to_string();
    }
    let mut total = root.to_string().len();

    if let Some(outputs) = root.get_mut("node_outputs").and_then(Value::as_object_mut) {
        let mut sizes: Vec<(String, usize)> = outputs
            .iter()
            .map(|(id, out)| (id.clone(), out.to_string().len()))
            .collect();
        sizes.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        for (id, size) in sizes {
            if total <= cap {
                break;
            }
            let marker = truncated_marker(size);
            let marker_len = marker.to_string().len();
            if size <= marker_len {
                break;
            }
            outputs.insert(id, marker);
            total = total - size + marker_len;
        }
    }
    if total <= cap {
        return root.to_string();
    }

    if let Some(logs) = root.get_mut("logs").and_then(Value::as_array_mut) {
        let excess = logs.len().saturating_sub(KEPT_LOG_ENTRIES);
        logs.drain(..excess);
    }
    let kept = root.to_string();
    if kept.len() <= cap {
        return kept;
    }

    let field = |name: &str| root.get(name).cloned().unwrap_or(Value::Null);
    json!({
        "execution_id": field("execution_id"),
        "workflow_id":  field("workflow_id"),
        "success":      field("success"),
        "node_outputs": {},
        "logs":         [],
        "error":        Value::Null,
        TRUNCATED_KEY:  { "original_bytes": original_bytes },
    })
    .to_string()
}

impl WorkflowDb {
    pub fn save_run(&self, record: &RunRecord) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let compacted = compact_result_json(&record.result_json);
        let oversized = compacted.len() > MAX_STORED_RESULT_BYTES;
        let result_json = bound_result_json(compacted, MAX_STORED_RESULT_BYTES);
        if oversized {
            tracing::warn!(
                run_id = %record.id,
                bytes = record.result_json.len(),
                "run result exceeds the history size limit; large node outputs were not kept"
            );
        }
        conn.execute(
            "INSERT OR REPLACE INTO run_history
             (id, workflow_id, workflow_name, ran_at, success, duration_ms, result_json, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                record.id,
                record.workflow_id,
                record.workflow_name,
                record.ran_at,
                record.success as i64,
                record.duration_ms,
                result_json,
                record.status,
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

    /// Writes a placeholder "running" record before execution begins, so a
    /// crash mid-run still leaves a trace (swept to 'interrupted' on next
    /// startup by `WorkflowDb::open`). `save_run` later overwrites this row
    /// via `INSERT OR REPLACE` using the same `id`.
    pub fn save_run_started(&self, id: &str, workflow_id: &str, workflow_name: &str, ran_at: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO run_history
             (id, workflow_id, workflow_name, ran_at, success, duration_ms, result_json, status)
             VALUES (?1, ?2, ?3, ?4, 0, 0, '', 'running')",
            rusqlite::params![id, workflow_id, workflow_name, ran_at],
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
        if let Some(sf) = success_filter {
            let mut stmt = conn.prepare(
                "SELECT id, workflow_id, workflow_name, ran_at, success, duration_ms, result_json, status
                 FROM run_history WHERE workflow_id = ?1 AND success = ?2
                 ORDER BY ran_at DESC LIMIT ?3 OFFSET ?4"
            ).map_err(|e| e.to_string())?;
            let mut used = 0;
            let rows = stmt.query_map(rusqlite::params![workflow_id, sf, limit, offset], |row| {
                row_to_run(row).map(|run| within_listing_budget(&mut used, MAX_LISTED_RESULT_BYTES, run))
            })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string());
            rows
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, workflow_id, workflow_name, ran_at, success, duration_ms, result_json, status
                 FROM run_history WHERE workflow_id = ?1
                 ORDER BY ran_at DESC LIMIT ?2 OFFSET ?3"
            ).map_err(|e| e.to_string())?;
            let mut used = 0;
            let rows = stmt.query_map(rusqlite::params![workflow_id, limit, offset], |row| {
                row_to_run(row).map(|run| within_listing_budget(&mut used, MAX_LISTED_RESULT_BYTES, run))
            })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string());
            rows
        }
    }

    /// Newest-first start time, outcome and duration of a workflow's runs,
    /// without the stored result documents.
    pub fn recent_run_summaries(&self, workflow_id: &str, limit: i64) -> Result<Vec<RunSummary>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT ran_at, success, duration_ms
             FROM run_history WHERE workflow_id = ?1
             ORDER BY ran_at DESC LIMIT ?2"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![workflow_id, limit.clamp(1, 1000)], |row| {
            Ok(RunSummary {
                ran_at:      row.get(0)?,
                success:     row.get::<_, i64>(1)? != 0,
                duration_ms: row.get(2)?,
            })
        }).map_err(|e| e.to_string())?
          .collect::<Result<Vec<_>, _>>()
          .map_err(|e| e.to_string());
        rows
    }

    pub fn get_run(&self, id: &str) -> Result<Option<RunRecord>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let result = conn.query_row(
            "SELECT id, workflow_id, workflow_name, ran_at, success, duration_ms, result_json, status
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
