use rusqlite::Connection;
use serde_json::{json, Value};
use tracing::warn;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput};

pub(super) async fn execute_sqlite(input: NodeInput) -> NodeOutput {
    let db_path = match input.input["db_path"].as_str() {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_PATH",
            "db_path is required for SQLite — provide an absolute path to your SQLite file.")),
    };
    let query = match input.input["query"].as_str() {
        Some(q) if !q.is_empty() => q.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_QUERY",
            "query is required")),
    };
    let is_execute = input.input["operation"].as_str()
        .map(|o| o == "execute")
        .unwrap_or(false);
    let params = super::parse_params(&input.input["params"]);
    let caller_is_admin = input.context.metadata
        .get("__caller_is_admin")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let allow_raw_sql = caller_is_admin
        && input.input["allow_raw_sql"].as_bool().unwrap_or(false);
    let inline_warning_sqlite = super::check_query_for_inline_values(&query);

    if inline_warning_sqlite.is_some() {
        if !allow_raw_sql {
            return NodeOutput::failure(NodeError::unrecoverable(
                "SQL_INJECTION_BLOCKED",
                "Query contains single-quoted string literals that may indicate \
                inline expression substitution. Use `?` placeholders and the `params` \
                array instead. To allow raw SQL (advanced/trusted use only), set \
                `allow_raw_sql: true` in the node config.",
            ));
        }
        warn!(
            workflow_id = %input.workflow_id,
            query = %query,
            "SQL_INJECTION_WARNING: query contains single-quoted literals with allow_raw_sql=true. \
            Ensure no untrusted input is inlined."
        );
    }

    if let Err(e) = super::pool::validate_db_path(&db_path) {
        return NodeOutput::failure(NodeError::unrecoverable("INVALID_PATH", e));
    }

    if let Some(sandbox_val) = input.context.metadata.get("__file_sandbox_dir") {
        if let Some(sandbox_str) = sandbox_val.as_str() {
            // Canonicalize the sandbox root so symlinks in the configured path
            // don't defeat the containment check.
            let sandbox = match std::fs::canonicalize(sandbox_str) {
                Ok(p) => p,
                Err(_) => return NodeOutput::failure(NodeError::unrecoverable(
                    "INVALID_PATH",
                    "Configured sandbox directory does not exist or cannot be resolved",
                )),
            };
            // Resolve the canonical db path. SQLite creates the file on first open,
            // so the file may not exist yet. Strategy mirrors file.rs write mode:
            // 1. Try full canonicalize — handles existing files and dereferences symlinks
            //    (catches sandbox/escape.db -> /external.db).
            // 2. On failure (file not yet on disk), canonicalize the parent directory
            //    and rejoin the filename — the parent must exist.
            let db_path_buf = std::path::PathBuf::from(&db_path);
            let canonical_db = match std::fs::canonicalize(&db_path_buf) {
                Ok(p) => p,
                Err(_) => {
                    let parent = db_path_buf.parent().unwrap_or_else(|| std::path::Path::new("."));
                    let fname  = db_path_buf.file_name().unwrap_or_default();
                    match std::fs::canonicalize(parent) {
                        Ok(cp) => cp.join(fname),
                        Err(_) => return NodeOutput::failure(NodeError::unrecoverable(
                            "INVALID_PATH",
                            "Database file's parent directory does not exist or cannot be resolved",
                        )),
                    }
                }
            };
            if !canonical_db.starts_with(&sandbox) {
                return NodeOutput::failure(NodeError::unrecoverable(
                    "PATH_OUTSIDE_SANDBOX",
                    format!("Database access is restricted to '{}'", sandbox_str),
                ));
            }
        }
    }

    let pool = match super::pool::get_sqlite_pool(&db_path) {
        Ok(p)  => p,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable("POOL_ERROR", e)),
    };

    let blocking_result = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let conn = pool.get()
            .map_err(|e| format!("Could not acquire connection for '{}': {}", db_path, e))?;
        if is_execute {
            sqlite_run_execute(&conn, &query, &params)
        } else {
            sqlite_run_query(&conn, &query, &params)
        }
    }).await;

    match blocking_result {
        Err(e)       => NodeOutput::failure(NodeError::unrecoverable("TASK_PANIC",
            format!("Database task panicked: {}", e))),
        Ok(Err(e))   => NodeOutput::failure(NodeError::unrecoverable("DB_ERROR", e)),
        Ok(Ok(data)) => {
            let row_count = data["rows"].as_array().map(|a| a.len()).unwrap_or(0);
            let mut logs = vec![format!("Query returned {} row(s)", row_count)];
            let mut out_data = data;
            if let Some(w) = inline_warning_sqlite {
                logs.push(format!("[SQL_INJECTION_WARNING] {}", w));
                out_data["_sql_injection_warning"] = serde_json::Value::String(w);
            }
            NodeOutput::success_with_logs(out_data, logs)
        }
    }
}

pub(super) fn sqlite_run_execute(conn: &Connection, query: &str, params: &[Value]) -> Result<Value, String> {
    let sql_params = sqlite_bind_params(params);
    let refs: Vec<&dyn rusqlite::ToSql> = sql_params.iter().map(|b| b.as_ref()).collect();
    let rows_affected = conn.execute(query, refs.as_slice())
        .map_err(|e| format!("Execute failed: {}", e))?;
    let last_id = conn.last_insert_rowid();
    Ok(json!({
        "rows": [],
        "rows_affected": rows_affected,
        "last_insert_id": last_id,
        "columns": []
    }))
}

pub(super) fn sqlite_run_query(conn: &Connection, query: &str, params: &[Value]) -> Result<Value, String> {
    let first_word = query.split_whitespace().next().unwrap_or("").to_uppercase();
    if first_word != "SELECT" && first_word != "WITH" {
        return Err("The Query operation only accepts SELECT statements. \
            Use the Execute operation for INSERT, UPDATE, DELETE, or other write operations.".to_string());
    }
    let sql_params = sqlite_bind_params(params);
    let refs: Vec<&dyn rusqlite::ToSql> = sql_params.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(query)
        .map_err(|e| format!("Query failed: {}. Check your SQL syntax.", e))?;
    let column_names: Vec<String> = stmt.column_names()
        .iter().map(|s| s.to_string()).collect();
    let cols = column_names.clone();
    // Collect rows into Result — errors surface instead of being silently dropped.
    let rows: Vec<Value> = stmt
        .query_map(refs.as_slice(), move |row| {
            let mut obj = serde_json::Map::new();
            for (i, col) in cols.iter().enumerate() {
                let val: Value = match row.get_ref(i) {
                    Ok(rusqlite::types::ValueRef::Null)       => Value::Null,
                    Ok(rusqlite::types::ValueRef::Integer(n)) => json!(n),
                    Ok(rusqlite::types::ValueRef::Real(f))    => json!(f),
                    Ok(rusqlite::types::ValueRef::Text(s))    => {
                        json!(String::from_utf8_lossy(s).to_string())
                    }
                    Ok(rusqlite::types::ValueRef::Blob(b))    => {
                        json!(format!("<blob {} bytes>", b.len()))
                    }
                    Err(_) => Value::Null,
                };
                obj.insert(col.clone(), val);
            }
            Ok(Value::Object(obj))
        })
        .map_err(|e| format!("Query execution failed: {}", e))?
        .collect::<Result<Vec<Value>, rusqlite::Error>>()
        .map_err(|e| format!("Row read error: {}", e))?;
    Ok(json!({
        "rows": rows,
        "rows_affected": 0,
        "last_insert_id": 0,
        "columns": column_names
    }))
}

fn sqlite_bind_params(params: &[Value]) -> Vec<Box<dyn rusqlite::ToSql>> {
    params.iter()
        .map(|v| -> Box<dyn rusqlite::ToSql> {
            match v {
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() { Box::new(i) }
                    else { Box::new(n.as_f64().unwrap_or(0.0)) }
                }
                Value::Bool(b)   => Box::new(if *b { 1i64 } else { 0i64 }),
                Value::String(s) => Box::new(s.clone()),
                _                => Box::new(Option::<String>::None),
            }
        })
        .collect()
}
