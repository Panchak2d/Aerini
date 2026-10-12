use rusqlite::{Connection, InterruptHandle};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tracing::warn;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput};
use crate::nodes::fs_sandbox;
use crate::nodes::util::OnDrop;

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
                array instead. Raw SQL can only be enabled by an admin caller (a server \
                run with admin rights); it is not available in the desktop app.",
            ));
        }
        warn!(
            workflow_id = %input.workflow_id,
            query_len = query.len(),
            "SQL_INJECTION_WARNING: query contains single-quoted literals with allow_raw_sql=true. \
            Ensure no untrusted input is inlined."
        );
    }

    if let Err(e) = super::pool::validate_db_path(&db_path) {
        return NodeOutput::failure(NodeError::unrecoverable("INVALID_PATH", e));
    }

    // Open the resolved path on Unix so the path that was checked is the path SQLite
    // opens. Windows keeps the caller's path: SQLite's handling of the verbatim
    // `\\?\` prefix on canonical Windows paths is unverified.
    let db_path = match fs_sandbox::root_from_metadata(&input.context.metadata) {
        Err(e) => return e.into_output("Database path", None),
        Ok(None) => db_path,
        Ok(Some(root)) => match fs_sandbox::resolve(&root, std::path::Path::new(&db_path)) {
            Ok(resolved) if cfg!(unix) => resolved.to_string_lossy().into_owned(),
            Ok(_) => db_path,
            Err(e) => return e.into_output("Database path", Some(&root)),
        },
    };

    let pool = match super::pool::get_sqlite_pool(&db_path) {
        Ok(p)  => p,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable("POOL_ERROR", e)),
    };

    let interrupt_slot: Arc<Mutex<Option<InterruptHandle>>> = Arc::new(Mutex::new(None));
    let interrupt_on_drop = {
        let slot = Arc::clone(&interrupt_slot);
        OnDrop::new(move || {
            if let Ok(guard) = slot.lock() {
                if let Some(handle) = guard.as_ref() {
                    handle.interrupt();
                }
            }
        })
    };
    let blocking_slot = Arc::clone(&interrupt_slot);
    let blocking_result = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let conn = pool.get()
            .map_err(|e| format!("Could not acquire connection for '{}': {}", db_path, e))?;
        if let Ok(mut slot) = blocking_slot.lock() {
            *slot = Some(conn.get_interrupt_handle());
        }
        let result = if is_execute {
            sqlite_run_execute(&conn, &query, &params)
        } else {
            sqlite_run_query(&conn, &query, &params)
        };
        if let Ok(mut slot) = blocking_slot.lock() {
            *slot = None;
        }
        result
    }).await;
    interrupt_on_drop.disarm();

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
    super::reject_path_escaping_statement(query)?;
    let sql_params = sqlite_bind_params(params)?;
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
    super::enforce_read_only_query(query)?;
    let sql_params = sqlite_bind_params(params)?;
    let refs: Vec<&dyn rusqlite::ToSql> = sql_params.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(query)
        .map_err(|e| format!("Query failed: {}. Check your SQL syntax.", e))?;
    let column_names: Vec<String> = stmt.column_names()
        .iter().map(|s| s.to_string()).collect();
    let cols = column_names.clone();
    let mapped = stmt
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
        .map_err(|e| format!("Query execution failed: {}", e))?;
    let mut rows: Vec<Value> = Vec::new();
    for row in mapped {
        if rows.len() >= super::MAX_QUERY_ROWS {
            return Err(super::row_limit_message());
        }
        rows.push(row.map_err(|e| format!("Row read error: {}", e))?);
    }
    Ok(json!({
        "rows": rows,
        "rows_affected": 0,
        "last_insert_id": 0,
        "columns": column_names
    }))
}

fn sqlite_bind_params(params: &[Value]) -> Result<Vec<Box<dyn rusqlite::ToSql>>, String> {
    params.iter()
        .enumerate()
        .map(|(i, v)| -> Result<Box<dyn rusqlite::ToSql>, String> {
            match v {
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() { Ok(Box::new(i)) }
                    else { Ok(Box::new(n.as_f64().unwrap_or(0.0))) }
                }
                Value::Bool(b)   => Ok(Box::new(if *b { 1i64 } else { 0i64 })),
                Value::String(s) => Ok(Box::new(s.clone())),
                Value::Null      => Ok(Box::new(Option::<String>::None)),
                Value::Array(_) | Value::Object(_) => Err(format!(
                    "params[{}] is a {} value — SQLite parameters must be scalars \
                     (string, number, bool, or null). Serialise it to a JSON string first \
                     if you need to store structured data.",
                    i,
                    if v.is_array() { "Array" } else { "Object" }
                )),
            }
        })
        .collect()
}

#[cfg(test)]
mod sqlite_run_query_tests {
    use super::sqlite_run_query;
    use rusqlite::Connection;

    #[test]
    fn select_accepted() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (x INTEGER)", []).unwrap();
        assert!(sqlite_run_query(&conn, "SELECT * FROM t", &[]).is_ok());
    }

    #[test]
    fn delete_rejected_by_query_operation() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (x INTEGER)", []).unwrap();
        let err = sqlite_run_query(&conn, "DELETE FROM t", &[]).unwrap_err();
        assert!(err.contains("SELECT"), "expected read-only rejection, got: {}", err);
    }

    // a WITH-prefixed DELETE must still be rejected once wired into sqlite_run_query itself.
    #[test]
    fn with_prefixed_delete_rejected_by_query_operation() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE users (name TEXT)", []).unwrap();
        let err = sqlite_run_query(&conn, "WITH x AS (SELECT 1) DELETE FROM users", &[])
            .unwrap_err();
        assert!(err.contains("SELECT"), "expected read-only rejection, got: {}", err);
    }
}

#[cfg(test)]
mod sqlite_bind_tests {
    use super::sqlite_bind_params;
    use serde_json::json;

    #[test]
    fn scalars_accepted() {
        let params = vec![json!(42), json!("hello"), json!(true), json!(null), json!(1.5)];
        assert!(sqlite_bind_params(&params).is_ok());
    }

    #[test]
    fn array_param_rejected_with_index_in_message() {
        let params = vec![json!("ok"), json!([1, 2, 3])];
        let err = match sqlite_bind_params(&params) {
            Err(e) => e,
            Ok(_) => panic!("expected array param to be rejected"),
        };
        assert!(err.contains("params[1]"), "expected index 1 in error, got: {}", err);
        assert!(err.contains("Array"), "expected 'Array' in error, got: {}", err);
    }

    #[test]
    fn object_param_rejected_with_index_in_message() {
        let params = vec![json!({"a": 1})];
        let err = match sqlite_bind_params(&params) {
            Err(e) => e,
            Ok(_) => panic!("expected object param to be rejected"),
        };
        assert!(err.contains("params[0]"), "expected index 0 in error, got: {}", err);
        assert!(err.contains("Object"), "expected 'Object' in error, got: {}", err);
    }

    #[test]
    fn empty_params_ok() {
        assert!(sqlite_bind_params(&[]).is_ok());
    }
}

#[cfg(test)]
mod interrupt_tests {
    use super::execute_sqlite;
    use crate::model::{ExecutionContext, NodeInput};
    use serde_json::json;

    #[tokio::test]
    async fn dropping_sqlite_query_future_interrupts_running_statement() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("runaway.db").to_string_lossy().to_string();
        let input = NodeInput {
            node_id: "n".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({
                "db_path": db_path,
                "operation": "query",
                "query": "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 500000000) SELECT count(*) FROM c"
            }),
            resolved_credentials: std::collections::HashMap::new(),
            context: ExecutionContext::default(),
            cancel_token: None,
        };

        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            execute_sqlite(input),
        )
        .await;
        assert!(outcome.is_err(), "statement should still be running when the future is dropped");

        let pool = super::super::pool::get_sqlite_pool(&db_path).unwrap();
        let mut released = false;
        for _ in 0..60 {
            let state = pool.state();
            if state.connections > 0 && state.idle_connections == state.connections {
                released = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(released, "connection was never returned to the pool after the future was dropped");
    }
}

#[cfg(all(test, unix))]
mod sandbox_tests {
    use super::execute_sqlite;
    use crate::model::{ExecutionContext, NodeInput, NodeOutput};
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::path::Path;

    async fn run(sandbox: Value, db_path: &Path) -> NodeOutput {
        let mut metadata: HashMap<String, Value> = HashMap::new();
        metadata.insert("__file_sandbox_dir".to_string(), sandbox);
        execute_sqlite(NodeInput {
            node_id: "n".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({
                "db_path": db_path.to_string_lossy(),
                "operation": "execute",
                "query": "CREATE TABLE IF NOT EXISTS t (x INTEGER)"
            }),
            resolved_credentials: HashMap::new(),
            context: ExecutionContext { metadata, ..Default::default() },
            cancel_token: None,
        })
        .await
    }

    fn root_value(root: &Path) -> Value {
        Value::String(root.to_str().unwrap().to_string())
    }

    #[tokio::test]
    async fn dangling_symlink_cannot_create_a_database_outside_the_sandbox() {
        let sandbox = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("created_by_attacker.db");
        let trap = sandbox.path().join("trap.db");
        std::os::unix::fs::symlink(&victim, &trap).unwrap();

        let out = run(root_value(sandbox.path()), &trap).await;

        assert_eq!(out.error.expect("must be refused").code, "INVALID_PATH");
        assert!(!victim.exists(), "nothing may be created outside the sandbox");
    }

    #[tokio::test]
    async fn database_inside_the_sandbox_still_opens_and_is_created() {
        let sandbox = tempfile::tempdir().unwrap();
        let db = sandbox.path().join("ok.db");

        let out = run(root_value(sandbox.path()), &db).await;

        assert!(out.error.is_none(), "{:?}", out.error);
        assert!(db.exists());
    }

    #[tokio::test]
    async fn database_outside_the_sandbox_is_refused() {
        let sandbox = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let db = outside.path().join("elsewhere.db");

        let out = run(root_value(sandbox.path()), &db).await;

        assert_eq!(out.error.expect("must be refused").code, "PATH_OUTSIDE_SANDBOX");
        assert!(!db.exists());
    }

    #[tokio::test]
    async fn a_non_string_sandbox_setting_fails_closed() {
        let outside = tempfile::tempdir().unwrap();
        let db = outside.path().join("any.db");

        let out = run(json!(true), &db).await;

        assert_eq!(out.error.expect("must be refused").code, "INVALID_PATH");
        assert!(!db.exists());
    }
}
