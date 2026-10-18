use serde_json::{json, Value};
use tokio::time::Duration;

use crate::error::NodeError;
use crate::model::NodeOutput;

// ── MySQL ─────────────────────────────────────────────────────────────────────

pub(super) async fn mysql_run(
    pool: &sqlx::MySqlPool,
    query: &str,
    params: &[Value],
    operation: super::Operation,
    kill_on_cancel: bool,
) -> NodeOutput {
    if operation == super::Operation::Query {
        if let Err(e) = super::enforce_read_only_query(query) {
            return NodeOutput::failure(NodeError::unrecoverable("QUERY_NOT_READ_ONLY", e));
        }
    }
    let mut q = sqlx::query(query);
    for p in params {
        q = mysql_bind_one(q, p);
    }
    let summarize: fn(&sqlx::mysql::MySqlQueryResult) -> (u64, u64) =
        |r| (r.rows_affected(), r.last_insert_id());

    if !kill_on_cancel {
        // `fetch_many` is the only call that also reports the row count and last
        // insert id; the deprecation concerns multi-statement strings, which
        // MySQL rejects for a prepared statement.
        #[allow(deprecated)]
        let steps = q.fetch_many(pool);
        return super::collect_steps(steps, operation, summarize, decode_mysql_value).await.output;
    }
    let mut guard = match mysql_acquire_cancellable(pool).await {
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable("DB_ERROR",
            format!("{} failed: {}", operation.label(), e))),
        Ok(g) => g,
    };
    #[allow(deprecated)]
    let steps = q.fetch_many(&mut **guard.conn());
    let mut collected = super::collect_steps(steps, operation, summarize, decode_mysql_value).await;
    if !guard.can_kill() {
        collected.output.logs.push(MYSQL_CANCEL_UNAVAILABLE_LOG.to_string());
    }
    // Still armed when reading stopped with the statement running (row limit):
    // dropping the guard below kills it and closes the connection.
    if collected.statement_finished {
        guard.release();
    }
    collected.output
}

const MYSQL_CANCEL_UNAVAILABLE_LOG: &str = "cancel-safety: could not capture this query's MySQL \
     connection id; a cancellation mid-query cannot be killed server-side this time";

/// Takes a pooled connection for one statement that must stop if its future is
/// dropped. When the session's connection id cannot be read the guard still
/// closes the connection on drop, but has no kill to send ([`super::CancelOnDrop::can_kill`]).
async fn mysql_acquire_cancellable(
    pool: &sqlx::MySqlPool,
) -> Result<super::CancelOnDrop<sqlx::MySql>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    let kill = mysql_capture_connection_id(&mut conn).await.map(|id| {
        let kill_pool = pool.clone();
        Box::new(move || {
            super::spawn_detached(async move { mysql_kill_query(&kill_pool, id).await })
        }) as Box<dyn FnOnce() + Send>
    });
    Ok(super::CancelOnDrop::new(conn, kill))
}

/// Reads the current session's connection id, for `mysql_kill_query` to target later.
/// `None` on any read failure: the caller runs without a kill rather than
/// failing the query over a diagnostic read.
async fn mysql_capture_connection_id(conn: &mut sqlx::pool::PoolConnection<sqlx::MySql>) -> Option<u64> {
    sqlx::query_scalar("SELECT CONNECTION_ID()")
        .fetch_one(&mut **conn)
        .await
        .ok()
}

const KILL_QUERY_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Best-effort: acquires a second pooled connection and issues `KILL QUERY` for
/// `connection_id`. The cancelled connection is closed by
/// [`super::CancelOnDrop`], so a kill that lands after the statement finished
/// reaches a connection that is gone ("Unknown thread id") and cannot
/// interrupt another statement. If no second connection can be had within the
/// timeout (the pool is exhausted), the statement keeps running on the server
/// until it finishes.
async fn mysql_kill_query(pool: &sqlx::MySqlPool, connection_id: u64) {
    let mut conn = match tokio::time::timeout(KILL_QUERY_ACQUIRE_TIMEOUT, pool.acquire()).await {
        Ok(Ok(c)) => c,
        _ => {
            tracing::warn!(connection_id = connection_id, "could not get a connection to kill a MySQL statement; it keeps running on the server");
            return;
        }
    };
    if let Err(e) = sqlx::query(&format!("KILL QUERY {}", connection_id))
        .execute(&mut *conn)
        .await
    {
        tracing::warn!(connection_id = connection_id, error = %e, "MySQL KILL QUERY failed; the statement keeps running on the server unless it had already ended");
    }
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
        return Ok(v.map_or(Value::Null, super::f64_json));
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
    let type_name = row.columns()[i].type_info().name();
    if let Some(value) = decode_mysql_raw(row, i, type_name) {
        return Ok(value);
    }
    Err(type_name.to_string())
}

/// Types read from the raw value: DECIMAL (an exact string), DATE, DATETIME,
/// TIMESTAMP and TIME (ISO 8601 strings), and JSON (the parsed value). The
/// driver leaves the length byte on the date and time types.
fn decode_mysql_raw(row: &sqlx::mysql::MySqlRow, i: usize, type_name: &str) -> Option<Value> {
    use super::types;
    use sqlx::Row;
    if !matches!(type_name, "DECIMAL" | "DATE" | "DATETIME" | "TIMESTAMP" | "TIME" | "JSON") {
        return None;
    }
    let bytes = row.try_get_unchecked::<Vec<u8>, _>(i).ok()?;
    let text = match type_name {
        "DECIMAL" => String::from_utf8(bytes).ok()?,
        "DATE" => types::mysql_datetime(&bytes, true)?,
        "DATETIME" | "TIMESTAMP" => types::mysql_datetime(&bytes, false)?,
        "TIME" => types::mysql_time(&bytes)?,
        _ => return serde_json::from_slice(&bytes).ok(),
    };
    Some(Value::String(text))
}

#[cfg(test)]
mod multi_statement_smuggling_tests {
    use super::mysql_run;
    use crate::nodes::database::Operation;

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
        let result = mysql_run(&pool, attack, &[], Operation::Query, false).await;

        // MySQL's COM_STMT_PREPARE rejects a multi-statement string outright.
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
    use super::{mysql_kill_query, mysql_run};
    use crate::nodes::database::Operation;

    #[tokio::test]
    #[ignore = "requires a live, disposable MySQL instance — set DATABASE_URL_MYSQL and run with `cargo test -- --ignored`"]
    async fn mysql_run_execute_with_kill_on_cancel_completes_normally_when_not_cancelled() {
        let url = std::env::var("DATABASE_URL_MYSQL")
            .expect("set DATABASE_URL_MYSQL to a disposable MySQL instance to run this test");
        let pool = sqlx::MySqlPool::connect(&url)
            .await
            .expect("failed to connect to DATABASE_URL_MYSQL");

        let result = mysql_run(&pool, "SELECT 1", &[], Operation::Execute, true).await;

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

    #[tokio::test]
    #[ignore = "requires a live, disposable MySQL instance — set DATABASE_URL_MYSQL and run with `cargo test -- --ignored`"]
    async fn mysql_run_query_with_kill_on_cancel_completes_normally_when_not_cancelled() {
        let url = std::env::var("DATABASE_URL_MYSQL")
            .expect("set DATABASE_URL_MYSQL to a disposable MySQL instance to run this test");
        let pool = sqlx::MySqlPool::connect(&url)
            .await
            .expect("failed to connect to DATABASE_URL_MYSQL");

        let result = mysql_run(&pool, "SELECT 1 AS n", &[], Operation::Query, true).await;

        assert!(result.success, "got: {:?}", result.error);
        assert_eq!(result.output.expect("rows")["rows"][0]["n"], serde_json::json!(1));
    }

    #[tokio::test]
    #[ignore = "requires a live, disposable MySQL instance — set DATABASE_URL_MYSQL and run with `cargo test -- --ignored`"]
    async fn dropping_a_running_query_kills_it() {
        let url = std::env::var("DATABASE_URL_MYSQL")
            .expect("set DATABASE_URL_MYSQL to a disposable MySQL instance to run this test");
        let pool = sqlx::MySqlPool::connect(&url)
            .await
            .expect("failed to connect to DATABASE_URL_MYSQL");
        let marker = "aerini_cancel_probe_my";
        let query = format!("SELECT SLEEP(30) AS {marker}");

        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            mysql_run(&pool, &query, &[], Operation::Query, true),
        )
        .await;
        assert!(outcome.is_err(), "the 30s statement should still be running when the timeout drops it");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let running: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM information_schema.processlist \
                 WHERE id <> CONNECTION_ID() AND info LIKE CONCAT('%', ?, '%')",
            )
            .bind(marker)
            .fetch_one(&pool)
            .await
            .expect("processlist probe failed");
            if running == 0 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the dropped query is still running on the server after 5s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}

#[cfg(test)]
mod type_decoding_tests {
    use super::mysql_run;
    use crate::nodes::database::Operation;
    use serde_json::json;

    #[tokio::test]
    #[ignore = "requires a live, disposable MySQL instance — set DATABASE_URL_MYSQL and run with `cargo test -- --ignored`"]
    async fn decimal_dates_times_and_json_columns_decode() {
        let url = std::env::var("DATABASE_URL_MYSQL")
            .expect("set DATABASE_URL_MYSQL to a disposable MySQL instance to run this test");
        let pool = sqlx::MySqlPool::connect(&url).await.expect("failed to connect to DATABASE_URL_MYSQL");
        let sql = "SELECT CAST(12.50 AS DECIMAL(10,2)) AS n, DATE '2024-02-29' AS d, \
                   CAST('2024-02-29 13:14:15.5' AS DATETIME(6)) AS dt, TIME '-01:02:03' AS t, \
                   CAST('{\"a\": 1}' AS JSON) AS j";
        let out = mysql_run(&pool, sql, &[], Operation::Query, false).await;
        assert!(out.success, "got: {:?}", out.error);
        let row = &out.output.expect("rows")["rows"][0];
        assert_eq!(row["n"], json!("12.50"));
        assert_eq!(row["d"], json!("2024-02-29"));
        assert_eq!(row["dt"], json!("2024-02-29T13:14:15.5"));
        assert_eq!(row["t"], json!("-01:02:03"));
        assert_eq!(row["j"], json!({"a": 1}));
    }
}
