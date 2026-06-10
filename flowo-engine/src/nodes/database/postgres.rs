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
    let mut q = sqlx::query(query);
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
    let mut q = sqlx::query(query);
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
