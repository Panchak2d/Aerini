use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;

// ── MySQL ─────────────────────────────────────────────────────────────────────

pub(super) async fn mysql_run_execute(pool: &sqlx::MySqlPool, query: &str, params: &[Value]) -> NodeOutput {
    let mut q = sqlx::query(query);
    for p in params {
        q = mysql_bind_one(q, p);
    }
    match q.execute(pool).await {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable("DB_ERROR",
            format!("Execute failed: {}", e))),
        Ok(result) => {
            let rows_affected = result.rows_affected();
            // last_insert_id() is available on MySqlQueryResult via sqlx::mysql::MySqlQueryResult
            let last_id = result.last_insert_id();
            NodeOutput::success_with_logs(
                json!({
                    "rows": [],
                    "rows_affected": rows_affected,
                    "last_insert_id": last_id,
                    "columns": []
                }),
                vec![format!("{} row(s) affected", rows_affected)],
            )
        }
    }
}

pub(super) async fn mysql_run_query(pool: &sqlx::MySqlPool, query: &str, params: &[Value]) -> NodeOutput {
    if let Err(e) = super::enforce_read_only_query(query) {
        return NodeOutput::failure(NodeError::unrecoverable("QUERY_NOT_READ_ONLY", e));
    }
    let mut q = sqlx::query(query);
    for p in params {
        q = mysql_bind_one(q, p);
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
                    obj.insert(col.name().to_string(), decode_mysql_value(row, i));
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

fn decode_mysql_value(row: &sqlx::mysql::MySqlRow, i: usize) -> Value {
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
