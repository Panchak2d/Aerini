use serde_json::{json, Value};
use tracing::warn;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput};

// ── Postgres / MySQL execution ─────────────────────────────────────────────────

pub(super) async fn execute_sqlx(input: NodeInput) -> NodeOutput {
    let db_type = input.input["db_type"].as_str().unwrap_or("sqlite");
    let url = match input.input["connection_url"].as_str() {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_URL",
            "connection_url is required for postgres and mysql. \
            Example: postgres://user@host/dbname")),
    };
    let url = match super::url_with_password(&url, &input.input["password"]) {
        Ok(u) => u,
        Err(out) => return out,
    };
    let query = match input.input["query"].as_str() {
        Some(q) if !q.is_empty() => q.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_QUERY",
            "query is required")),
    };
    let is_execute = input.input["operation"].as_str()
        .map(|o| o == "execute")
        .unwrap_or(false);
    let params = match super::parse_params(&input.input["params"]) {
        Ok(p) => p,
        Err(out) => return out,
    };
    let caller_is_admin = input.context.metadata
        .get("__caller_is_admin")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let allow_raw_sql = caller_is_admin
        && input.input["allow_raw_sql"].as_bool().unwrap_or(false);
    let inline_warning = super::check_query_for_inline_values(&query).or_else(|| {
        if db_type == "mysql" { super::check_double_quoted_literal(&query) } else { None }
    });
    // Not gated behind __caller_is_admin the way allow_raw_sql is: that gate is
    // never set on desktop (see util.rs::check_db_url_ssrf).
    let ssrf_policy = if input.input["allow_local"].as_bool().unwrap_or(false) {
        crate::nodes::util::SsrfPolicy::AllowLocal
    } else {
        crate::nodes::util::SsrfPolicy::Strict
    };

    if inline_warning.is_some() {
        if !allow_raw_sql {
            return NodeOutput::failure(NodeError::unrecoverable(
                "SQL_INJECTION_BLOCKED",
                "Query contains single-quoted string literals that may indicate \
                inline expression substitution. Use parameterized placeholders \
                and the `params` array instead. Raw SQL can only be enabled by an \
                admin caller (a server run with admin rights); it is not available \
                in the desktop app.",
            ));
        }
        warn!(
            workflow_id = %input.workflow_id,
            query_len = query.len(),
            "SQL_INJECTION_WARNING: query contains single-quoted literals with allow_raw_sql=true. \
            Ensure no untrusted input is inlined."
        );
    }

    let mysql = db_type == "mysql";
    if let Err(e) = super::pool::check_sqlx_target(&url, mysql, ssrf_policy).await {
        return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
    }
    let cancellable = input.cancel_token.is_some();
    let operation = if is_execute { super::Operation::Execute } else { super::Operation::Query };
    let mut output = if mysql {
        match super::pool::get_mysql_pool(&url).await {
            Err(e) => NodeOutput::failure(NodeError::unrecoverable("POOL_ERROR", e)),
            Ok(pool) => super::mysql::mysql_run(&pool, &query, &params, operation, cancellable).await,
        }
    } else {
        match super::pool::get_pg_pool(&url).await {
            Err(e) => NodeOutput::failure(NodeError::unrecoverable("POOL_ERROR", e)),
            Ok(pool) => pg_run(&pool, &query, &params, operation, cancellable).await,
        }
    };
    if let Some(w) = inline_warning {
        output.logs.push(format!("[SQL_INJECTION_WARNING] {}", w));
        if let Some(ref mut data) = output.output {
            data["_sql_injection_warning"] = serde_json::Value::String(w);
        }
    }
    output
}

// ── Postgres ──────────────────────────────────────────────────────────────────

async fn pg_run(
    pool: &sqlx::PgPool,
    query: &str,
    params: &[Value],
    operation: super::Operation,
    cancel_on_drop: bool,
) -> NodeOutput {
    if operation == super::Operation::Query {
        if let Err(e) = super::enforce_read_only_query(query) {
            return NodeOutput::failure(NodeError::unrecoverable("QUERY_NOT_READ_ONLY", e));
        }
    }
    let rewritten = rewrite_pg_placeholders(query);
    let mut q = sqlx::query(&rewritten);
    for p in params {
        q = pg_bind_one(q, p);
    }
    let summarize: fn(&sqlx::postgres::PgQueryResult) -> (u64, u64) = |r| (r.rows_affected(), 0);

    if !cancel_on_drop {
        // `fetch_many` is the only call that also reports the row count; the
        // deprecation concerns multi-statement strings, which Postgres rejects
        // for a prepared statement.
        #[allow(deprecated)]
        let steps = q.fetch_many(pool);
        return super::collect_steps(steps, operation, summarize, decode_pg_value).await.output;
    }
    let mut guard = match pg_acquire_cancellable(pool).await {
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable("DB_ERROR",
            format!("{} failed: {}", operation.label(), e))),
        Ok(g) => g,
    };
    #[allow(deprecated)]
    let steps = q.fetch_many(&mut **guard.conn());
    let mut collected = super::collect_steps(steps, operation, summarize, decode_pg_value).await;
    if !guard.can_kill() {
        collected.output.logs.push(PG_CANCEL_UNAVAILABLE_LOG.to_string());
    }
    // Still armed when reading stopped with the statement running (row limit):
    // dropping the guard below cancels it and closes the connection.
    if collected.statement_finished {
        guard.release();
    }
    collected.output
}

const PG_CANCEL_UNAVAILABLE_LOG: &str = "cancel-safety: could not capture this query's Postgres \
     backend pid; a cancellation mid-query cannot be signalled server-side this time";

/// Takes a pooled connection for one statement that must stop if its future is
/// dropped. When the session's backend pid cannot be read the guard still
/// closes the connection on drop, but has no kill to send ([`super::CancelOnDrop::can_kill`]).
async fn pg_acquire_cancellable(
    pool: &sqlx::PgPool,
) -> Result<super::CancelOnDrop<sqlx::Postgres>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    let kill = pg_capture_backend_pid(&mut conn).await.map(|pid| {
        let cancel_pool = pool.clone();
        Box::new(move || {
            super::spawn_detached(async move { pg_cancel_backend(&cancel_pool, pid).await })
        }) as Box<dyn FnOnce() + Send>
    });
    Ok(super::CancelOnDrop::new(conn, kill))
}

/// Reads the current session's backend pid, for `pg_cancel_backend` to target later.
/// `None` on any read failure: the caller runs without a kill instead of
/// failing the query over a diagnostic read.
async fn pg_capture_backend_pid(conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>) -> Option<i32> {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **conn)
        .await
        .ok()
}

const CANCEL_BACKEND_ACQUIRE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Best-effort: acquires a second pooled connection and asks Postgres to cancel
/// the statement running on `backend_pid`. The cancelled connection is closed
/// by [`super::CancelOnDrop`], so a request that lands late reaches a backend
/// that is gone and cannot interrupt anyone else's statement. If no second
/// connection can be had within the timeout (the pool is exhausted), the
/// statement keeps running on the server until it finishes.
async fn pg_cancel_backend(pool: &sqlx::PgPool, backend_pid: i32) {
    let mut conn = match tokio::time::timeout(CANCEL_BACKEND_ACQUIRE_TIMEOUT, pool.acquire()).await {
        Ok(Ok(c)) => c,
        _ => {
            warn!(backend_pid = backend_pid, "could not get a connection to cancel a Postgres statement; it keeps running on the server");
            return;
        }
    };
    if let Err(e) = sqlx::query("SELECT pg_cancel_backend($1)")
        .bind(backend_pid)
        .execute(&mut *conn)
        .await
    {
        warn!(backend_pid = backend_pid, error = %e, "Postgres cancel request failed; the statement keeps running on the server");
    }
}

// ── Placeholder rewriting ──────────────────────────────────────────────────────

/// Rewrite `?` positional placeholders to PostgreSQL's `$1, $2, ...` syntax.
///
/// sqlx does not translate placeholders client-side — Postgres processes `$N`
/// natively and does not recognise `?`. MySQL/MariaDB use `?` natively and are
/// handled by a separate code path, so this function is Postgres-only.
///
/// A `?` is left alone inside single-quoted strings (including `E'...'` strings
/// with backslash escapes), double-quoted identifiers, `--` and nested `/* */`
/// comments, and dollar-quoted strings, so literal question marks in user data
/// (e.g. `WHERE note = 'anything?'`) are not misidentified as parameter markers
/// and do not shift the numbering of the real ones.
fn rewrite_pg_placeholders(query: &str) -> String {
    let chars: Vec<char> = query.chars().collect();
    let mut out = String::with_capacity(query.len() + 16);
    let mut param_idx: u32 = 1;
    let mut i = 0;
    while i < chars.len() {
        if let Some(end) = verbatim_region_end(&chars, i) {
            out.extend(&chars[i..end]);
            i = end;
        } else if chars[i] == '?' {
            out.push('$');
            out.push_str(&param_idx.to_string());
            param_idx += 1;
            i += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// If a quoted string, quoted identifier, comment or dollar-quoted string
/// starts at `chars[i]`, the index just past its end (the end of input when it
/// is unterminated).
fn verbatim_region_end(chars: &[char], i: usize) -> Option<usize> {
    match chars[i] {
        '\'' => {
            let escape_string = i > 0
                && matches!(chars[i - 1], 'e' | 'E')
                && (i < 2 || !is_word_char(chars[i - 2]));
            Some(quoted_end(chars, i, '\'', escape_string))
        }
        '"' => Some(quoted_end(chars, i, '"', false)),
        '-' if chars.get(i + 1) == Some(&'-') => {
            Some(chars[i..].iter().position(|&c| c == '\n').map_or(chars.len(), |p| i + p))
        }
        '/' if chars.get(i + 1) == Some(&'*') => Some(block_comment_end(chars, i)),
        '$' => dollar_quote_end(chars, i),
        _ => None,
    }
}

/// End of the string or identifier opened by `chars[start]`; a doubled quote
/// stays inside it, and so does a backslash-escaped character when `backslash`.
fn quoted_end(chars: &[char], start: usize, quote: char, backslash: bool) -> usize {
    let mut j = start + 1;
    while j < chars.len() {
        let c = chars[j];
        if backslash && c == '\\' {
            j += 2;
        } else if c == quote {
            if chars.get(j + 1) == Some(&quote) {
                j += 2;
            } else {
                return j + 1;
            }
        } else {
            j += 1;
        }
    }
    chars.len()
}

/// Postgres block comments nest.
fn block_comment_end(chars: &[char], start: usize) -> usize {
    let mut depth = 0usize;
    let mut j = start;
    while j < chars.len() {
        if chars[j] == '/' && chars.get(j + 1) == Some(&'*') {
            depth += 1;
            j += 2;
        } else if chars[j] == '*' && chars.get(j + 1) == Some(&'/') {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return j;
            }
        } else {
            j += 1;
        }
    }
    chars.len()
}

/// `$tag$ ... $tag$` (the tag may be empty). `$1` is a parameter, and a `$`
/// that continues an identifier (`price$`) opens nothing.
fn dollar_quote_end(chars: &[char], start: usize) -> Option<usize> {
    if start > 0 && (is_word_char(chars[start - 1]) || chars[start - 1] == '$') {
        return None;
    }
    let mut j = start + 1;
    while j < chars.len() && is_word_char(chars[j]) {
        j += 1;
    }
    if chars.get(j) != Some(&'$') || chars.get(start + 1).is_some_and(char::is_ascii_digit) {
        return None;
    }
    let delimiter = &chars[start..=j];
    let mut k = j + 1;
    while k + delimiter.len() <= chars.len() {
        if chars[k..k + delimiter.len()] == *delimiter {
            return Some(k + delimiter.len());
        }
        k += 1;
    }
    Some(chars.len())
}

fn pg_bind_one<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    p: &'q Value,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
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
/// does not decode (numeric, timestamps, uuid, json, bytea, arrays, ...).
fn decode_pg_value(row: &sqlx::postgres::PgRow, i: usize) -> Result<Value, String> {
    use sqlx::{Column, Row, TypeInfo, ValueRef};
    if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
        return Ok(v.map_or(Value::Null, |n| json!(n)));
    }
    if let Ok(v) = row.try_get::<Option<i32>, _>(i) {
        return Ok(v.map_or(Value::Null, |n| json!(n)));
    }
    if let Ok(v) = row.try_get::<Option<i16>, _>(i) {
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
    let raw = row.try_get_raw(i).ok();
    if raw.as_ref().is_some_and(|r| r.is_null()) {
        return Ok(Value::Null);
    }
    let type_info = row.columns()[i].type_info();
    if let Some(value) = raw.and_then(|r| decode_pg_raw(&r, type_info)) {
        return Ok(value);
    }
    Err(type_info.name().to_string())
}

/// Types read from the raw binary value: NUMERIC (an exact string), the date and
/// time types (ISO 8601 strings), UUID (a string), JSON and JSONB (the parsed
/// value) and enums (the label).
fn decode_pg_raw(raw: &sqlx::postgres::PgValueRef<'_>, type_info: &sqlx::postgres::PgTypeInfo) -> Option<Value> {
    use super::types;
    use sqlx::postgres::{PgTypeKind, PgValueFormat};
    use sqlx::TypeInfo;
    if raw.format() != PgValueFormat::Binary {
        return None;
    }
    let bytes = raw.as_bytes().ok()?;
    if matches!(type_info.kind(), PgTypeKind::Enum(_)) {
        return std::str::from_utf8(bytes).ok().map(|label| json!(label));
    }
    let text = match type_info.name() {
        "NUMERIC" => types::pg_numeric(bytes)?,
        "DATE" => types::pg_date(bytes)?,
        "TIMESTAMP" => types::pg_timestamp(bytes, false)?,
        "TIMESTAMPTZ" => types::pg_timestamp(bytes, true)?,
        "TIME" => types::pg_time(bytes)?,
        "UUID" => types::pg_uuid(bytes)?,
        "JSON" => return types::pg_json(bytes, false),
        "JSONB" => return types::pg_json(bytes, true),
        _ => return None,
    };
    Some(Value::String(text))
}

#[cfg(test)]
mod tests {
    use super::rewrite_pg_placeholders;

    #[test]
    fn single_placeholder_rewritten() {
        assert_eq!(
            rewrite_pg_placeholders("SELECT * FROM users WHERE id = ?"),
            "SELECT * FROM users WHERE id = $1"
        );
    }

    #[test]
    fn multiple_placeholders_numbered_in_order() {
        assert_eq!(
            rewrite_pg_placeholders("INSERT INTO t (a, b, c) VALUES (?, ?, ?)"),
            "INSERT INTO t (a, b, c) VALUES ($1, $2, $3)"
        );
    }

    #[test]
    fn no_placeholders_unchanged() {
        let q = "SELECT * FROM users";
        assert_eq!(rewrite_pg_placeholders(q), q);
    }

    #[test]
    fn question_mark_inside_string_literal_preserved() {
        assert_eq!(
            rewrite_pg_placeholders("SELECT * FROM notes WHERE body = 'anything?' AND id = ?"),
            "SELECT * FROM notes WHERE body = 'anything?' AND id = $1"
        );
    }

    #[test]
    fn escaped_quote_inside_literal_does_not_exit_string_early() {
        // 'it''s a test?' contains an escaped quote — the `?` immediately
        // after must stay inside the string, not become $1.
        assert_eq!(
            rewrite_pg_placeholders("SELECT * FROM t WHERE x = 'it''s a test?' AND id = ?"),
            "SELECT * FROM t WHERE x = 'it''s a test?' AND id = $1"
        );
    }

    #[test]
    fn multiple_string_literals_with_placeholders_between() {
        assert_eq!(
            rewrite_pg_placeholders("SELECT ? FROM t WHERE a = 'x?y' AND b = ? AND c = 'z?'"),
            "SELECT $1 FROM t WHERE a = 'x?y' AND b = $2 AND c = 'z?'"
        );
    }

    #[test]
    fn question_mark_inside_comment_or_quoted_identifier_does_not_shift_numbering() {
        assert_eq!(
            rewrite_pg_placeholders(
                "SELECT \"why?\" FROM t -- really?\nWHERE a = ? /* ok? /* nested? */ still? */ AND b = ?"
            ),
            "SELECT \"why?\" FROM t -- really?\nWHERE a = $1 /* ok? /* nested? */ still? */ AND b = $2"
        );
    }

    #[test]
    fn question_mark_inside_escape_and_dollar_quoted_strings_is_preserved() {
        assert_eq!(
            rewrite_pg_placeholders("SELECT E'it\\'s ?', $$a?$$, $t$b?$t$, price$ FROM t WHERE id = ?"),
            "SELECT E'it\\'s ?', $$a?$$, $t$b?$t$, price$ FROM t WHERE id = $1"
        );
    }

    #[test]
    fn unterminated_literal_or_comment_is_copied_through() {
        for q in ["SELECT 'a?", "SELECT \"a?", "SELECT 1 /* a?", "SELECT 1 -- a?", "SELECT $$ a?", "SELECT E'a\\"] {
            assert_eq!(rewrite_pg_placeholders(q), q);
        }
    }

    #[test]
    fn placeholder_immediately_after_closing_quote() {
        assert_eq!(
            rewrite_pg_placeholders("SELECT * FROM t WHERE a = 'lit'?"),
            "SELECT * FROM t WHERE a = 'lit'$1"
        );
    }
}

#[cfg(test)]
mod multi_statement_smuggling_tests {
    use super::pg_run;
    use crate::nodes::database::Operation;

    #[tokio::test]
    #[ignore = "requires a live, disposable Postgres instance — set DATABASE_URL_PG and run with `cargo test -- --ignored`"]
    async fn semicolon_stacked_statement_does_not_execute_second_command() {
        let url = std::env::var("DATABASE_URL_PG")
            .expect("set DATABASE_URL_PG to a disposable Postgres instance to run this test");
        let pool = sqlx::PgPool::connect(&url)
            .await
            .expect("failed to connect to DATABASE_URL_PG");

        // Belt-and-braces cleanup in case a prior failed run left this behind.
        sqlx::query("DROP TABLE IF EXISTS __aerini_smuggle_proof")
            .execute(&pool)
            .await
            .ok();

        let attack = "SELECT 1; CREATE TABLE __aerini_smuggle_proof (id INT); \
                       INSERT INTO __aerini_smuggle_proof VALUES (1);";
        let result = pg_run(&pool, attack, &[], Operation::Query, false).await;

        // Postgres's extended protocol rejects a multi-statement string outright.
        assert!(
            !result.success,
            "expected the stacked statement to be rejected by Postgres's extended protocol, \
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
mod cancellation_tests {
    use super::pg_run;
    use crate::nodes::database::Operation;
    use serde_json::json;
    use std::time::{Duration, Instant};

    async fn pool() -> sqlx::PgPool {
        let url = std::env::var("DATABASE_URL_PG")
            .expect("set DATABASE_URL_PG to a disposable Postgres instance to run this test");
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("failed to connect to DATABASE_URL_PG")
    }

    /// Waits until no other backend, running or idle, last ran a statement
    /// containing `marker`. An idle one means its connection was recycled
    /// instead of closed.
    async fn wait_until_no_backend_has(pool: &sqlx::PgPool, marker: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity \
                 WHERE pid <> pg_backend_pid() AND query LIKE '%' || $1 || '%'",
            )
            .bind(marker)
            .fetch_one(pool)
            .await
            .expect("pg_stat_activity probe failed");
            if remaining == 0 {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{remaining} backend(s) still hold a statement with `{marker}` after 5s: \
                 it was not cancelled, or its connection went back to the pool"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    #[tokio::test]
    #[ignore = "requires a live, disposable Postgres instance — set DATABASE_URL_PG and run with `cargo test -- --ignored`"]
    async fn query_with_cancel_guard_completes_normally_when_not_cancelled() {
        let pool = pool().await;
        let result = pg_run(&pool, "SELECT 1::int4 AS n", &[], Operation::Query, true).await;
        assert!(result.success, "got: {:?}", result.error);
        assert_eq!(result.output.expect("rows")["rows"][0]["n"], json!(1));
    }

    #[tokio::test]
    #[ignore = "requires a live, disposable Postgres instance — set DATABASE_URL_PG and run with `cargo test -- --ignored`"]
    async fn dropping_a_running_query_cancels_it_and_closes_its_connection() {
        let pool = pool().await;
        let marker = "aerini_cancel_probe_pg";
        let query = format!("SELECT pg_sleep(30) AS {marker}");

        let outcome =
            tokio::time::timeout(Duration::from_millis(500), pg_run(&pool, &query, &[], Operation::Query, true)).await;
        assert!(outcome.is_err(), "the 30s statement should still be running when the timeout drops it");

        wait_until_no_backend_has(&pool, marker).await;
    }

    #[tokio::test]
    #[ignore = "requires a live, disposable Postgres instance — set DATABASE_URL_PG and run with `cargo test -- --ignored`"]
    async fn query_past_the_row_limit_fails_and_closes_its_connection() {
        let pool = pool().await;
        let marker = "aerini_rowlimit_probe_pg";
        let query = format!(
            "SELECT g AS {marker} FROM generate_series(1, {}) AS g",
            super::super::MAX_QUERY_ROWS + 1
        );

        let result = pg_run(&pool, &query, &[], Operation::Query, true).await;
        assert!(!result.success);
        assert_eq!(result.error.expect("error").code, "ROW_LIMIT_EXCEEDED");

        wait_until_no_backend_has(&pool, marker).await;
        let after = pg_run(&pool, "SELECT 1::int4 AS n", &[], Operation::Query, true).await;
        assert!(after.success, "the pool must stay usable after an abandoned query, got: {:?}", after.error);
    }
}

#[cfg(test)]
mod type_decoding_tests {
    use super::pg_run;
    use crate::nodes::database::Operation;
    use serde_json::json;

    #[tokio::test]
    #[ignore = "requires a live, disposable Postgres instance — set DATABASE_URL_PG and run with `cargo test -- --ignored`"]
    async fn numeric_dates_uuid_and_json_columns_decode() {
        let url = std::env::var("DATABASE_URL_PG")
            .expect("set DATABASE_URL_PG to a disposable Postgres instance to run this test");
        let pool = sqlx::PgPool::connect(&url).await.expect("failed to connect to DATABASE_URL_PG");
        let sql = "SELECT 12345.6700::numeric(10,4) AS n, DATE '2024-02-29' AS d, \
                   TIMESTAMP '2024-02-29 13:14:15.5' AS ts, TIMESTAMPTZ '2024-02-29 13:14:15+00' AS tz, \
                   TIME '01:02:03' AS t, '123e4567-e89b-12d3-a456-426614174000'::uuid AS u, \
                   '{\"a\":[1]}'::jsonb AS jb, '{\"a\":1}'::json AS j, \
                   (-0.0001)::numeric AS neg, 123456789012345678901234567890.123456789::numeric AS big, \
                   0::numeric(5,2) AS zero, 'NaN'::numeric AS nan, 'infinity'::timestamp AS inf, \
                   '1999-12-31 23:59:59.000001'::timestamp AS before_epoch, 'ok'::__aerini_probe_mood AS mood";
        sqlx::query("DROP TYPE IF EXISTS __aerini_probe_mood").execute(&pool).await.ok();
        sqlx::query("CREATE TYPE __aerini_probe_mood AS ENUM ('ok')").execute(&pool).await.expect("create type failed");
        let out = pg_run(&pool, sql, &[], Operation::Query, false).await;
        sqlx::query("DROP TYPE IF EXISTS __aerini_probe_mood").execute(&pool).await.ok();
        assert!(out.success, "got: {:?}", out.error);
        let row = &out.output.expect("rows")["rows"][0];
        assert_eq!(row["n"], json!("12345.6700"));
        assert_eq!(row["d"], json!("2024-02-29"));
        assert_eq!(row["ts"], json!("2024-02-29T13:14:15.5"));
        assert_eq!(row["tz"], json!("2024-02-29T13:14:15Z"));
        assert_eq!(row["t"], json!("01:02:03"));
        assert_eq!(row["u"], json!("123e4567-e89b-12d3-a456-426614174000"));
        assert_eq!(row["jb"], json!({"a": [1]}));
        assert_eq!(row["j"], json!({"a": 1}));
        assert_eq!(row["neg"], json!("-0.0001"));
        assert_eq!(row["big"], json!("123456789012345678901234567890.123456789"));
        assert_eq!(row["zero"], json!("0.00"));
        assert_eq!(row["nan"], json!("NaN"));
        assert_eq!(row["inf"], json!("infinity"));
        assert_eq!(row["before_epoch"], json!("1999-12-31T23:59:59.000001"));
        assert_eq!(row["mood"], json!("ok"));
    }

    #[tokio::test]
    #[ignore = "requires a live, disposable Postgres instance — set DATABASE_URL_PG and run with `cargo test -- --ignored`"]
    async fn execute_returns_returning_rows() {
        let url = std::env::var("DATABASE_URL_PG")
            .expect("set DATABASE_URL_PG to a disposable Postgres instance to run this test");
        let pool = sqlx::PgPool::connect(&url).await.expect("failed to connect to DATABASE_URL_PG");
        sqlx::query("DROP TABLE IF EXISTS __aerini_returning_probe").execute(&pool).await.ok();
        sqlx::query("CREATE TABLE __aerini_returning_probe (id bigserial PRIMARY KEY, x bigint)")
            .execute(&pool)
            .await
            .expect("create table failed");

        let out = pg_run(
            &pool,
            "INSERT INTO __aerini_returning_probe (x) VALUES (?), (?) RETURNING id, x",
            &[json!(5), json!(6)],
            Operation::Execute,
            true,
        )
        .await;
        sqlx::query("DROP TABLE IF EXISTS __aerini_returning_probe").execute(&pool).await.ok();

        assert!(out.success, "got: {:?}", out.error);
        let data = out.output.expect("output");
        assert_eq!(data["rows"], json!([{ "id": 1, "x": 5 }, { "id": 2, "x": 6 }]));
        assert_eq!(data["rows_affected"], json!(2));
    }
}
