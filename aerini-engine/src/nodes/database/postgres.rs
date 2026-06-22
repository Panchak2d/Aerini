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
            Example: postgres://user:pass@host/dbname")),
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
    let inline_warning = super::check_query_for_inline_values(&query);

    if inline_warning.is_some() {
        if !allow_raw_sql {
            return NodeOutput::failure(NodeError::unrecoverable(
                "SQL_INJECTION_BLOCKED",
                "Query contains single-quoted string literals that may indicate \
                inline expression substitution. Use parameterized placeholders \
                and the `params` array instead. To allow raw SQL (advanced/trusted \
                use only), set `allow_raw_sql: true` in the node config.",
            ));
        }
        warn!(
            workflow_id = %input.workflow_id,
            query = %query,
            "SQL_INJECTION_WARNING: query contains single-quoted literals with allow_raw_sql=true. \
            Ensure no untrusted input is inlined."
        );
    }

    let mut output = if db_type == "mysql" {
        if let Err(e) = crate::nodes::util::check_db_url_ssrf(&url).await {
            return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
        }
        match super::pool::get_mysql_pool(&url).await {
            Err(e) => NodeOutput::failure(NodeError::unrecoverable("POOL_ERROR", e)),
            Ok(pool) => if is_execute {
                super::mysql::mysql_run_execute(&pool, &query, &params).await
            } else {
                super::mysql::mysql_run_query(&pool, &query, &params).await
            },
        }
    } else {
        if let Err(e) = crate::nodes::util::check_db_url_ssrf(&url).await {
            return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
        }
        match super::pool::get_pg_pool(&url).await {
            Err(e) => NodeOutput::failure(NodeError::unrecoverable("POOL_ERROR", e)),
            Ok(pool) => if is_execute {
                pg_run_execute(&pool, &query, &params).await
            } else {
                pg_run_query(&pool, &query, &params).await
            },
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

async fn pg_run_execute(pool: &sqlx::PgPool, query: &str, params: &[Value]) -> NodeOutput {
    let rewritten = rewrite_pg_placeholders(query);
    let mut q = sqlx::query(&rewritten);
    for p in params {
        q = pg_bind_one(q, p);
    }
    match q.execute(pool).await {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable("DB_ERROR",
            format!("Execute failed: {}", e))),
        Ok(result) => {
            let rows_affected = result.rows_affected();
            NodeOutput::success_with_logs(
                json!({
                    "rows": [],
                    "rows_affected": rows_affected,
                    "last_insert_id": 0,
                    "columns": []
                }),
                vec![format!("{} row(s) affected", rows_affected)],
            )
        }
    }
}

async fn pg_run_query(pool: &sqlx::PgPool, query: &str, params: &[Value]) -> NodeOutput {
    let rewritten = rewrite_pg_placeholders(query);
    let mut q = sqlx::query(&rewritten);
    for p in params {
        q = pg_bind_one(q, p);
    }
    match q.fetch_all(pool).await {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable("DB_ERROR",
            format!("Query failed: {}", e))),
        Ok(rows) => {
            use sqlx::{Column, Row};
            let column_names: Vec<String> = rows.first()
                .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                .unwrap_or_default();
            let json_rows: Vec<Value> = rows.iter().map(|row| {
                let mut obj = serde_json::Map::new();
                for (i, col) in row.columns().iter().enumerate() {
                    obj.insert(col.name().to_string(), decode_pg_value(row, i));
                }
                Value::Object(obj)
            }).collect();
            let count = json_rows.len();
            NodeOutput::success_with_logs(
                json!({
                    "rows": json_rows,
                    "rows_affected": 0,
                    "last_insert_id": 0,
                    "columns": column_names
                }),
                vec![format!("Query returned {} row(s)", count)],
            )
        }
    }
}

// ── Placeholder rewriting ──────────────────────────────────────────────────────

/// Rewrite `?` positional placeholders to PostgreSQL's `$1, $2, ...` syntax.
///
/// sqlx does not translate placeholders client-side — Postgres processes `$N`
/// natively and does not recognise `?`. MySQL/MariaDB use `?` natively and are
/// handled by a separate code path, so this function is Postgres-only.
///
/// Skips `?` characters that appear inside single-quoted string literals so that
/// literal question marks in user data (e.g. `WHERE note = 'anything?'`) are not
/// misidentified as parameter markers. Escaped single-quotes inside string literals
/// are represented as `''` in standard SQL and handled correctly.
fn rewrite_pg_placeholders(query: &str) -> String {
    let mut out = String::with_capacity(query.len() + 16);
    let mut param_idx: u32 = 1;
    let mut in_string = false;
    let mut chars = query.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if ch == '\'' {
                // `''` is an escaped quote inside a string literal — stay in string.
                if chars.peek() == Some(&'\'') {
                    out.push(chars.next().unwrap());
                } else {
                    in_string = false;
                }
            }
        } else {
            match ch {
                '\'' => {
                    in_string = true;
                    out.push(ch);
                }
                '?' => {
                    out.push('$');
                    out.push_str(&param_idx.to_string());
                    param_idx += 1;
                }
                _ => out.push(ch),
            }
        }
    }
    out
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

fn decode_pg_value(row: &sqlx::postgres::PgRow, i: usize) -> Value {
    use sqlx::Row;
    if let Ok(opt) = row.try_get::<Option<i64>, _>(i) {
        return opt.map(|v| json!(v)).unwrap_or(Value::Null);
    }
    if let Ok(opt) = row.try_get::<Option<f64>, _>(i) {
        return opt.map(|v| json!(v)).unwrap_or(Value::Null);
    }
    if let Ok(opt) = row.try_get::<Option<bool>, _>(i) {
        return opt.map(|v| json!(v)).unwrap_or(Value::Null);
    }
    if let Ok(opt) = row.try_get::<Option<String>, _>(i) {
        return opt.map(|v| json!(v)).unwrap_or(Value::Null);
    }
    Value::Null
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
    fn placeholder_immediately_after_closing_quote() {
        assert_eq!(
            rewrite_pg_placeholders("SELECT * FROM t WHERE a = 'lit'?"),
            "SELECT * FROM t WHERE a = 'lit'$1"
        );
    }
}
