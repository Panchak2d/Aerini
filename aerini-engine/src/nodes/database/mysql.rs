use serde_json::{json, Value};
use tokio::time::Duration;

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::util::OnDrop;

// ── MySQL ─────────────────────────────────────────────────────────────────────

pub(super) async fn mysql_run_execute(
    pool: &sqlx::MySqlPool,
    query: &str,
    params: &[Value],
    kill_on_cancel: bool,
) -> NodeOutput {
    let mut q = sqlx::query(query);
    for p in params {
        q = mysql_bind_one(q, p);
    }

    let mut kill_mitigation_unavailable = false;
    let exec_result = if kill_on_cancel {
        let mut conn = match pool.acquire().await {
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("DB_ERROR",
                format!("Execute failed: {}", e))),
            Ok(c) => c,
        };
        let connection_id = mysql_capture_connection_id(&mut conn).await;
        kill_mitigation_unavailable = connection_id.is_none();
        let kill_on_drop = connection_id.map(|id| {
            let kill_pool = pool.clone();
            OnDrop::new(move || {
                super::spawn_detached(async move { mysql_kill_query(&kill_pool, id).await })
            })
        });
        let result = q.execute(&mut *conn).await;
        if let Some(guard) = kill_on_drop {
            guard.disarm();
        }
        result
    } else {
        q.execute(pool).await
    };

    match exec_result {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable("DB_ERROR",
            format!("Execute failed: {}", e))),
        Ok(result) => {
            let rows_affected = result.rows_affected();
            // last_insert_id() is available on MySqlQueryResult via sqlx::mysql::MySqlQueryResult
            let last_id = result.last_insert_id();
            let mut output = NodeOutput::success_with_logs(
                json!({
                    "rows": [],
                    "rows_affected": rows_affected,
                    "last_insert_id": last_id,
                    "columns": []
                }),
                vec![format!("{} row(s) affected", rows_affected)],
            );
            if kill_mitigation_unavailable {
                output.logs.push(
                    "cancel-safety: could not capture this query's MySQL connection id; \
                     a cancellation mid-query cannot be killed server-side this time".to_string(),
                );
            }
            output
        }
    }
}

/// Reads the current session's connection id, for `mysql_kill_query` to target later.
/// `None` on any read failure — the caller falls back to running without kill-safety
/// rather than failing the query over a diagnostic read.
async fn mysql_capture_connection_id(conn: &mut sqlx::pool::PoolConnection<sqlx::MySql>) -> Option<u64> {
    sqlx::query_scalar("SELECT CONNECTION_ID()")
        .fetch_one(&mut **conn)
        .await
        .ok()
}

const KILL_QUERY_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Best-effort: acquires a second pooled connection and issues `KILL QUERY` for
/// `connection_id`, then gives up silently on acquire timeout or error. Not a
/// guaranteed kill — MySQL's own KILL QUERY has a known race (upstream bug
/// #79838) where a kill landing just after the target query already finished
/// can hit that connection's next statement instead of the intended one.
async fn mysql_kill_query(pool: &sqlx::MySqlPool, connection_id: u64) {
    let mut conn = match tokio::time::timeout(KILL_QUERY_ACQUIRE_TIMEOUT, pool.acquire()).await {
        Ok(Ok(c)) => c,
        _ => return,
    };
    let _ = sqlx::query(&format!("KILL QUERY {}", connection_id))
        .execute(&mut *conn)
        .await;
}

pub(super) async fn mysql_run_query(pool: &sqlx::MySqlPool, query: &str, params: &[Value]) -> NodeOutput {
    if let Err(e) = super::enforce_read_only_query(query) {
        return NodeOutput::failure(NodeError::unrecoverable("QUERY_NOT_READ_ONLY", e));
    }
    let mut q = sqlx::query(query);
    for p in params {
        q = mysql_bind_one(q, p);
    }
    use futures_util::StreamExt;
    use sqlx::{Column, Row};
    let mut stream = q.fetch(pool);
    let mut column_names: Vec<String> = Vec::new();
    let mut json_rows: Vec<Value> = Vec::new();
    let mut unsupported: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    while let Some(item) = stream.next().await {
        let row = match item {
            Ok(r) => r,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("DB_ERROR",
                format!("Query failed: {}", e))),
        };
        if json_rows.len() >= super::MAX_QUERY_ROWS {
            return NodeOutput::failure(NodeError::unrecoverable("ROW_LIMIT_EXCEEDED",
                super::row_limit_message()));
        }
        if column_names.is_empty() {
            column_names = row.columns().iter().map(|c| c.name().to_string()).collect();
        }
        let mut obj = serde_json::Map::new();
        for (i, col) in row.columns().iter().enumerate() {
            let value = match decode_mysql_value(&row, i) {
                Ok(v) => v,
                Err(type_name) => {
                    let placeholder = Value::String(super::undecodable_placeholder(&type_name));
                    unsupported.entry(col.name().to_string()).or_insert(type_name);
                    placeholder
                }
            };
            obj.insert(col.name().to_string(), value);
        }
        json_rows.push(Value::Object(obj));
    }
    let count = json_rows.len();
    let mut logs = vec![format!("Query returned {} row(s)", count)];
    for (column, type_name) in &unsupported {
        logs.push(super::unsupported_type_log(column, type_name));
    }
    NodeOutput::success_with_logs(
        json!({
            "rows": json_rows,
            "rows_affected": 0,
            "last_insert_id": 0,
            "columns": column_names
        }),
        logs,
    )
}

fn mysql_bind_one<'q>(
    q: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    p: &'q Value,
) -> sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    match p {
        Value::Null      => q.bind(Option::<String>::None),
        Value::Bool(b)   => q.bind(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() { q.bind(i) }
            else { q.bind(n.as_f64().unwrap_or(0.0)) }
        }
        Value::String(s) => q.bind(s.as_str()),
        other            => q.bind(other.to_string()),
    }
}

/// `Err(type_name)` when the column holds a non-NULL value of a type this node
/// does not decode (DECIMAL, dates and times, JSON, binary, ...).
fn decode_mysql_value(row: &sqlx::mysql::MySqlRow, i: usize) -> Result<Value, String> {
    use sqlx::{Column, Row, TypeInfo, ValueRef};
    if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
        return Ok(v.map_or(Value::Null, |n| json!(n)));
    }
    if let Ok(v) = row.try_get::<Option<u64>, _>(i) {
        return Ok(v.map_or(Value::Null, |n| json!(n)));
    }
    if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
        return Ok(v.map_or(Value::Null, |n| json!(n)));
    }
    if let Ok(v) = row.try_get::<Option<f32>, _>(i) {
        return Ok(v.map_or(Value::Null, super::f32_json));
    }
    if let Ok(v) = row.try_get::<Option<bool>, _>(i) {
        return Ok(v.map_or(Value::Null, |b| json!(b)));
    }
    if let Ok(v) = row.try_get::<Option<String>, _>(i) {
        return Ok(v.map_or(Value::Null, |s| json!(s)));
    }
    if row.try_get_raw(i).map(|r| r.is_null()).unwrap_or(false) {
        return Ok(Value::Null);
    }
    Err(row.columns()[i].type_info().name().to_string())
}

#[cfg(test)]
mod multi_statement_smuggling_tests {
    use super::mysql_run_query;

    #[tokio::test]
    #[ignore = "requires a live, disposable MySQL instance — set DATABASE_URL_MYSQL and run with `cargo test -- --ignored`"]
    async fn semicolon_stacked_statement_does_not_execute_second_command() {
        let url = std::env::var("DATABASE_URL_MYSQL")
            .expect("set DATABASE_URL_MYSQL to a disposable MySQL instance to run this test");
        let pool = sqlx::MySqlPool::connect(&url)
            .await
            .expect("failed to connect to DATABASE_URL_MYSQL");

        sqlx::query("DROP TABLE IF EXISTS __aerini_smuggle_proof")
            .execute(&pool)
            .await
            .ok();

        let attack = "SELECT 1; CREATE TABLE __aerini_smuggle_proof (id INT); \
                       INSERT INTO __aerini_smuggle_proof VALUES (1);";
        let result = mysql_run_query(&pool, attack, &[]).await;

        // Expected: hard failure — MySQL's own COM_STMT_PREPARE rejects a
        // multi-statement string outright (see comment above); it does not
        // silently execute just the first statement and ignore the rest.
        assert!(
            !result.success,
            "expected the stacked statement to be rejected by MySQL's prepared-statement protocol, \
             but the call reported success — multi-statement smuggling is exploitable"
        );

        let leaked = sqlx::query(
            "SELECT 1 FROM information_schema.tables WHERE table_name = '__aerini_smuggle_proof'",
        )
        .fetch_all(&pool)
        .await
        .expect("existence check itself failed");
        assert!(
            leaked.is_empty(),
            "smuggled CREATE TABLE executed — multi-statement smuggling IS exploitable, escalate immediately"
        );

        sqlx::query("DROP TABLE IF EXISTS __aerini_smuggle_proof")
            .execute(&pool)
            .await
            .ok();
    }
}

#[cfg(test)]
mod cancellation_kill_tests {
    use super::{mysql_kill_query, mysql_run_execute};

    #[tokio::test]
    #[ignore = "requires a live, disposable MySQL instance — set DATABASE_URL_MYSQL and run with `cargo test -- --ignored`"]
    async fn mysql_run_execute_with_kill_on_cancel_completes_normally_when_not_cancelled() {
        let url = std::env::var("DATABASE_URL_MYSQL")
            .expect("set DATABASE_URL_MYSQL to a disposable MySQL instance to run this test");
        let pool = sqlx::MySqlPool::connect(&url)
            .await
            .expect("failed to connect to DATABASE_URL_MYSQL");

        let result = mysql_run_execute(&pool, "SELECT 1", &[], true).await;

        assert!(
            result.success,
            "expected a fast query to succeed normally when kill-on-cancel is armed but never fires, got: {:?}",
            result.error
        );
    }

    #[tokio::test]
    #[ignore = "requires a live, disposable MySQL instance — set DATABASE_URL_MYSQL and run with `cargo test -- --ignored`"]
    async fn mysql_kill_query_interrupts_in_flight_query() {
        let url = std::env::var("DATABASE_URL_MYSQL")
            .expect("set DATABASE_URL_MYSQL to a disposable MySQL instance to run this test");
        let pool = sqlx::MySqlPool::connect(&url)
            .await
            .expect("failed to connect to DATABASE_URL_MYSQL");

        let mut conn = pool.acquire().await.expect("failed to acquire connection");
        let connection_id: u64 = sqlx::query_scalar("SELECT CONNECTION_ID()")
            .fetch_one(&mut *conn)
            .await
            .expect("failed to read CONNECTION_ID()");

        let sleeper = tokio::spawn(async move {
            let start = std::time::Instant::now();
            let result = sqlx::query("SELECT SLEEP(5)").execute(&mut *conn).await;
            (result, start.elapsed())
        });

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        mysql_kill_query(&pool, connection_id).await;

        let (result, elapsed) = sleeper.await.expect("sleeper task panicked");
        assert!(
            result.is_err(),
            "expected KILL QUERY to interrupt the in-flight SLEEP(5) with an error, got Ok"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "expected the kill to interrupt SLEEP(5) well before its natural 5s completion, took {:?}",
            elapsed
        );
    }
}
