use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

mod pool;
mod sqlite;
mod postgres;
mod mysql;
mod redis;

pub use pool::start_pool_eviction_task;

// ── Node ──────────────────────────────────────────────────────────────────────

pub struct DatabaseNode;

#[async_trait]
impl Node for DatabaseNode {
    fn type_id(&self)      -> &'static str { "database" }
    fn display_name(&self) -> &'static str { "Database" }
    fn node_type(&self)    -> NodeType     { NodeType::Action }
    fn version(&self)      -> &'static str { "2.0.0" }
    fn description(&self)  -> &'static str { "Run SQL queries against a SQLite, PostgreSQL, or MySQL database and return the results." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "db_type": {
                    "type": "string",
                    "enum": ["sqlite", "postgres", "mysql", "redis"],
                    "description": "Database backend. Default: sqlite."
                },
                "db_path": {
                    "type": "string",
                    "description": "SQLite only. Absolute path ending in .db, .sqlite, or .sqlite3."
                },
                "connection_url": {
                    "type": "string",
                    "description": "Postgres / MySQL / Redis. Full connection URL including credentials. Examples: postgres://user:pass@host/db  mysql://user:pass@host/db  redis://host:6379"
                },
                "query": {
                    "type": "string",
                    "description": "SQL query. Use ? for parameters. Not used for Redis — use operation + key/value/field."
                },
                "params": {
                    "type": "string",
                    "description": "JSON array of positional parameters for ? placeholders. Example: [42, \"Alice\"]"
                },
                "operation": {
                    "type": "string",
                    "description": "SQL: query (SELECT) or execute (INSERT/UPDATE/DELETE). Redis: get, set, del, lpush, rpush, lpop, rpop, hget, hset."
                },
                "key": {
                    "type": "string",
                    "description": "Redis: key to operate on."
                },
                "value": {
                    "type": "string",
                    "description": "Redis: value for set, lpush, rpush, hset."
                },
                "field": {
                    "type": "string",
                    "description": "Redis: hash field name for hget and hset."
                },
                "expire": {
                    "type": "number",
                    "description": "Redis set: optional TTL in seconds. Omit for no expiry."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "rows":           { "type": "array",            "description": "SQL SELECT rows." },
                "rows_affected":  { "type": "number",           "description": "SQL execute: rows changed." },
                "last_insert_id": { "type": "number",           "description": "Last inserted row ID. SQLite and MySQL populate this. Postgres returns 0 — use RETURNING instead." },
                "columns":        { "type": "array",            "description": "SQL SELECT: column names in result order." },
                "value":          { "type": ["string", "null"], "description": "Redis get/hget/lpop/rpop: retrieved value or null." },
                "ok":             { "type": "boolean",          "description": "Redis set/hset: true on success." },
                "deleted":        { "type": "number",           "description": "Redis del: number of keys deleted." },
                "list_length":    { "type": "number",           "description": "Redis lpush/rpush: list length after push." }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs:  vec![PortDefinition { id: "input".to_string(),    label: "In".to_string(),    position: PortPosition::Left }],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Out".to_string(),   position: PortPosition::Right },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(), position: PortPosition::Right },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        if input.context.metadata.get("__database_disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return NodeOutput::failure(NodeError::unrecoverable(
                "DATABASE_DISABLED",
                "Database node is disabled in this deployment. \
                 Pass --allow-database to the server to enable it.",
            ));
        }
        let db_type = input.input["db_type"].as_str().unwrap_or("sqlite");
        match db_type {
            "postgres" | "mysql" => postgres::execute_sqlx(input).await,
            "redis"              => redis::execute_redis(input).await,
            _                   => sqlite::execute_sqlite(input).await,
        }
    }
}

// ── SQLite execution ──────────────────────────────────────────────────────────

/// Checks whether a resolved SQL query string contains single-quoted literals that
/// may indicate direct expression interpolation instead of parameterized binding.
/// Returns a warning string when suspicious patterns are found.
///
/// Users must always use `?` placeholders and the `params` array for any value
/// that comes from workflow data or external input. Inline expression substitution
/// bypasses parameterized query protection.
pub(super) fn check_query_for_inline_values(query: &str) -> Option<String> {
    // Note: Aerini expression syntax ({{...}}) is resolved by the executor BEFORE
    // this function is called — checking for "{{" here would be dead code. The
    // correct enforcement point is at the workflow/executor level (pre-resolution).
    //
    // What we CAN check here (post-resolution) is literal SQL injection patterns:
    //   - PostgreSQL dollar-quoting ($$...$$): produces string literals with no
    //     single quotes, bypassing the single-quote heuristic entirely.
    //   - Single-quoted literals: may indicate inline value substitution.
    if query.trim().contains("$$") {
        return Some(
            "SQL INJECTION WARNING: This query contains PostgreSQL dollar-quoting ($$). \
             Dollar-quoted strings bypass the single-quote injection heuristic. \
             Use `?` placeholders and the `params` array for all dynamic values."
            .to_string()
        );
    }
    if query.trim().contains('\'') {
        return Some(
            "SQL INJECTION WARNING: This query contains single-quoted string literals. \
             If any quoted value originates from a workflow expression or external input, \
             use `?` placeholders and the `params` array instead of inline expressions. \
             Inline expression substitution bypasses parameterized query protection."
            .to_string()
        );
    }
    None
}

// ── Shared helpers ────────────────────────────────────────────────────────────

pub(super) fn parse_params(v: &Value) -> Vec<Value> {
    v.as_str()
        .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
        .or_else(|| v.as_array().cloned())
        .unwrap_or_default()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::pool::validate_db_path;

    #[test]
    fn relative_path_rejected() {
        let e = validate_db_path("data/myapp.db").unwrap_err();
        assert!(e.contains("absolute"), "expected absolute path error, got: {}", e);
    }

    #[test]
    fn path_traversal_rejected() {
        let e = validate_db_path("/home/user/../secret.db").unwrap_err();
        assert!(e.contains(".."), "expected traversal error, got: {}", e);
    }

    #[test]
    fn bad_extension_rejected() {
        let e = validate_db_path("/home/user/authorized_keys").unwrap_err();
        assert!(e.contains(".db") || e.contains("sqlite"), "expected extension error, got: {}", e);
    }

    #[test]
    fn txt_extension_rejected() {
        let e = validate_db_path("/home/user/data.txt").unwrap_err();
        assert!(e.contains(".db") || e.contains("sqlite"), "expected extension error, got: {}", e);
    }

    #[test]
    fn valid_db_path_accepted() {
        assert!(validate_db_path("/home/user/myapp.db").is_ok());
    }

    #[test]
    fn valid_sqlite_path_accepted() {
        assert!(validate_db_path("/var/lib/app/store.sqlite").is_ok());
    }

    #[test]
    fn valid_sqlite3_path_accepted() {
        assert!(validate_db_path("/tmp/test.sqlite3").is_ok());
    }

    #[tokio::test]
    async fn private_ip_connection_url_rejected() {
        let urls = [
            "postgres://x:x@127.0.0.1:5432/db",
            "postgres://x:x@10.0.0.1:5432/db",
            "redis://169.254.169.254:6379",
            "mysql://x:x@::1:3306/db",
        ];
        for url in &urls {
            assert!(
                crate::nodes::util::check_db_url_ssrf(url).await.is_err(),
                "Expected SSRF block for: {url}"
            );
        }
    }
}
