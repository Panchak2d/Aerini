use async_trait::async_trait;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};
use crate::nodes::util::scrub_url_in_error;
use tracing::warn;

/// Returns a BLAKE3 hex hash of the connection URL, used as the DashMap pool
/// cache key. This prevents the plaintext URL (which may contain a password)
/// from being stored as a map key in process memory.
fn pool_key(url: &str) -> String {
    blake3::hash(url.as_bytes()).to_hex().to_string()
}

// ── Connection pool registries ────────────────────────────────────────────────

/// Wraps a pool with a last-used timestamp for idle eviction.
struct PoolEntry<P> {
    pool:      P,
    last_used: Instant,
}

/// SQLite pools are excluded from eviction: SQLite is file-local with no remote
/// credential rotation concern, and evicting WAL-mode connections unnecessarily
/// disrupts in-progress transactions.
static SQLITE_POOLS: Lazy<DashMap<String, Pool<SqliteConnectionManager>>> =
    Lazy::new(DashMap::new);

static PG_POOLS:    Lazy<DashMap<String, PoolEntry<sqlx::PgPool>>>                      = Lazy::new(DashMap::new);
static MYSQL_POOLS: Lazy<DashMap<String, PoolEntry<sqlx::MySqlPool>>>                   = Lazy::new(DashMap::new);

/// Redis client objects hold no connection state — excluded from eviction.
static REDIS_CLIENTS: Lazy<DashMap<String, redis::Client>> = Lazy::new(DashMap::new);

static REDIS_CONNS: Lazy<DashMap<String, PoolEntry<redis::aio::MultiplexedConnection>>> =
    Lazy::new(DashMap::new);

// ── SQLite helpers ────────────────────────────────────────────────────────────

/// Validates a db_path before opening. Rejects paths that are:
/// - relative (must be absolute — prevents opening files relative to cwd)
/// - missing a recognised SQLite extension (.db / .sqlite / .sqlite3)
/// - containing path traversal sequences (..)
fn validate_db_path(path: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    if !p.is_absolute() {
        return Err(
            "db_path must be an absolute path. Example: /home/user/myapp.db".to_string()
        );
    }
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !matches!(ext, "db" | "sqlite" | "sqlite3") {
        return Err(format!(
            "db_path must end in .db, .sqlite, or .sqlite3 — got '.{}'",
            ext
        ));
    }
    if path.contains("..") {
        return Err("db_path must not contain '..'.".to_string());
    }
    Ok(())
}

fn get_sqlite_pool(path: &str) -> Result<Pool<SqliteConnectionManager>, String> {
    if let Some(pool) = SQLITE_POOLS.get(path) {
        return Ok(pool.clone());
    }
    let manager = SqliteConnectionManager::file(path)
        .with_init(|conn| conn.execute_batch("PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL;"));
    let pool = Pool::builder()
        .max_size(4)
        .build(manager)
        .map_err(|e| format!("Could not create pool for '{}': {}", path, e))?;
    SQLITE_POOLS.insert(path.to_string(), pool.clone());
    Ok(pool)
}

// ── sqlx pool (Postgres / MySQL) ──────────────────────────────────────────────

async fn get_pg_pool(url: &str) -> Result<sqlx::PgPool, String> {
    let key = pool_key(url);
    PG_POOLS.retain(|_, v| v.last_used.elapsed() < Duration::from_secs(1800));
    if let Some(mut entry) = PG_POOLS.get_mut(&key) {
        entry.last_used = Instant::now();
        return Ok(entry.pool.clone());
    }
    let pool = sqlx::PgPool::connect(url)
        .await
        .map_err(|e| scrub_url_in_error(&format!("Postgres connection failed: {}", e)))?;
    let entry = PG_POOLS.entry(key).or_insert(PoolEntry { pool, last_used: Instant::now() });
    Ok(entry.pool.clone())
}

async fn get_mysql_pool(url: &str) -> Result<sqlx::MySqlPool, String> {
    let key = pool_key(url);
    MYSQL_POOLS.retain(|_, v| v.last_used.elapsed() < Duration::from_secs(1800));
    if let Some(mut entry) = MYSQL_POOLS.get_mut(&key) {
        entry.last_used = Instant::now();
        return Ok(entry.pool.clone());
    }
    let pool = sqlx::MySqlPool::connect(url)
        .await
        .map_err(|e| scrub_url_in_error(&format!("MySQL connection failed: {}", e)))?;
    let entry = MYSQL_POOLS.entry(key).or_insert(PoolEntry { pool, last_used: Instant::now() });
    Ok(entry.pool.clone())
}

// ── Redis client + connection helpers ────────────────────────────────────────

fn get_redis_client(url: &str) -> Result<redis::Client, String> {
    let key = pool_key(url);
    if let Some(c) = REDIS_CLIENTS.get(&key) {
        return Ok(c.clone());
    }
    let client = redis::Client::open(url)
        .map_err(|e| scrub_url_in_error(&format!("Redis URL invalid: {}", e)))?;
    let entry = REDIS_CLIENTS.entry(key).or_insert(client);
    Ok(entry.clone())
}

/// Returns a cached MultiplexedConnection for `url`, creating one if absent.
async fn get_redis_conn(url: &str) -> Result<redis::aio::MultiplexedConnection, String> {
    let key = pool_key(url);
    REDIS_CONNS.retain(|_, v| v.last_used.elapsed() < Duration::from_secs(1800));
    if let Some(mut entry) = REDIS_CONNS.get_mut(&key) {
        entry.last_used = Instant::now();
        return Ok(entry.pool.clone());
    }
    let client = get_redis_client(url)?;
    match client.get_multiplexed_async_connection().await {
        Ok(c) => {
            let entry = REDIS_CONNS.entry(key).or_insert(PoolEntry { pool: c, last_used: Instant::now() });
            Ok(entry.pool.clone())
        }
        Err(e) => Err(scrub_url_in_error(&format!("Redis connection failed: {}", e))),
    }
}

/// Runs `build_cmd()` on a cached connection for `url`.
/// On IoError: evicts the stale connection, reconnects, retries exactly once.
async fn redis_cmd_with_retry<T, F>(url: &str, build_cmd: F) -> Result<T, redis::RedisError>
where
    T: redis::FromRedisValue,
    F: Fn() -> redis::Cmd + Send,
{
    let mut conn = get_redis_conn(url).await.map_err(|e| {
        redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::NotConnected, e))
    })?;

    match build_cmd().query_async::<T>(&mut conn).await {
        Ok(val) => Ok(val),
        Err(e) if e.is_connection_dropped() || e.is_io_error() => {
            REDIS_CONNS.remove(&pool_key(url));
            let mut fresh = get_redis_conn(url).await.map_err(|ce| {
                redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::NotConnected, ce))
            })?;
            build_cmd().query_async::<T>(&mut fresh).await
        }
        Err(e) => Err(e),
    }
}

// ── Node ──────────────────────────────────────────────────────────────────────

pub struct DatabaseNode;

#[async_trait]
impl Node for DatabaseNode {
    fn type_id(&self)      -> &'static str { "database" }
    fn display_name(&self) -> &'static str { "Database" }
    fn node_type(&self)    -> NodeType     { NodeType::Action }
    fn version(&self)      -> &'static str { "2.0.0" }

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
        let db_type = input.input["db_type"].as_str().unwrap_or("sqlite");
        match db_type {
            "postgres" | "mysql" => execute_sqlx(input).await,
            "redis"              => execute_redis(input).await,
            _                   => execute_sqlite(input).await,
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
fn check_query_for_inline_values(query: &str) -> Option<String> {
    // Note: Flowo expression syntax ({{...}}) is resolved by the executor BEFORE
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


async fn execute_sqlite(input: NodeInput) -> NodeOutput {
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
    let params = parse_params(&input.input["params"]);
    let caller_is_admin = input.context.metadata
        .get("__caller_is_admin")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let allow_raw_sql = caller_is_admin
        && input.input["allow_raw_sql"].as_bool().unwrap_or(false);
    let inline_warning_sqlite = check_query_for_inline_values(&query);

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

    if let Err(e) = validate_db_path(&db_path) {
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

    let pool = match get_sqlite_pool(&db_path) {
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

fn sqlite_run_execute(conn: &Connection, query: &str, params: &[Value]) -> Result<Value, String> {
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

fn sqlite_run_query(conn: &Connection, query: &str, params: &[Value]) -> Result<Value, String> {
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
        .filter_map(|r| r.ok())
        .collect();
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

// ── Postgres / MySQL execution ─────────────────────────────────────────────────

async fn execute_sqlx(input: NodeInput) -> NodeOutput {
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
    let params = parse_params(&input.input["params"]);
    let caller_is_admin = input.context.metadata
        .get("__caller_is_admin")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let allow_raw_sql = caller_is_admin
        && input.input["allow_raw_sql"].as_bool().unwrap_or(false);
    let inline_warning = check_query_for_inline_values(&query);

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
        match get_mysql_pool(&url).await {
            Err(e) => NodeOutput::failure(NodeError::unrecoverable("POOL_ERROR", e)),
            Ok(pool) => if is_execute {
                mysql_run_execute(&pool, &query, &params).await
            } else {
                mysql_run_query(&pool, &query, &params).await
            },
        }
    } else {
        if let Err(e) = crate::nodes::util::check_db_url_ssrf(&url).await {
            return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
        }
        match get_pg_pool(&url).await {
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

// ── MySQL ─────────────────────────────────────────────────────────────────────

async fn mysql_run_execute(pool: &sqlx::MySqlPool, query: &str, params: &[Value]) -> NodeOutput {
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

async fn mysql_run_query(pool: &sqlx::MySqlPool, query: &str, params: &[Value]) -> NodeOutput {
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

// ── Redis execution ───────────────────────────────────────────────────────────

async fn execute_redis(input: NodeInput) -> NodeOutput {
    let url = match input.input["connection_url"].as_str() {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_URL",
            "connection_url is required for redis. Example: redis://host:6379")),
    };
    let operation = match input.input["operation"].as_str() {
        Some(op) if !op.is_empty() => op.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_OPERATION",
            "operation is required for redis. One of: get, set, del, lpush, rpush, lpop, rpop, hget, hset")),
    };
    let key = match input.input["key"].as_str() {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_KEY",
            "key is required for all Redis operations")),
    };

    // Use redis::cmd() raw API for all operations — stable across crate versions.
    // redis_cmd_with_retry handles connection caching and evict-and-retry on IoError.
    if let Err(e) = crate::nodes::util::check_db_url_ssrf(&url).await {
        return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
    }
    match operation.as_str() {
        "get" => {
            match redis_cmd_with_retry::<Option<String>, _>(&url, || {
                let mut cmd = redis::cmd("GET");
                cmd.arg(&key);
                cmd
            }).await {
                Err(e)  => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("GET failed: {}", e))),
                Ok(val) => NodeOutput::success(json!({ "value": val })),
            }
        }

        "set" => {
            let value = input.input["value"].as_str().unwrap_or("").to_string();
            let ttl = input.input["expire"].as_u64();
            match redis_cmd_with_retry::<(), _>(&url, || {
                let mut cmd = redis::cmd("SET");
                cmd.arg(&key).arg(&value);
                if let Some(t) = ttl {
                    cmd.arg("EX").arg(t);
                }
                cmd
            }).await {
                Err(e) => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("SET failed: {}", e))),
                Ok(_)  => NodeOutput::success(json!({ "ok": true, "key": key })),
            }
        }

        "del" => {
            match redis_cmd_with_retry::<i64, _>(&url, || {
                let mut cmd = redis::cmd("DEL");
                cmd.arg(&key);
                cmd
            }).await {
                Err(e)    => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("DEL failed: {}", e))),
                Ok(count) => NodeOutput::success(json!({ "deleted": count, "key": key })),
            }
        }

        "lpush" | "rpush" => {
            let value = match input.input["value"].as_str() {
                Some(v) => v.to_string(),
                None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_VALUE",
                    "value is required for lpush and rpush")),
            };
            let cmd_name = if operation == "lpush" { "LPUSH" } else { "RPUSH" };
            match redis_cmd_with_retry::<i64, _>(&url, || {
                let mut cmd = redis::cmd(cmd_name);
                cmd.arg(&key).arg(&value);
                cmd
            }).await {
                Err(e)  => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("{} failed: {}", cmd_name, e))),
                Ok(len) => NodeOutput::success(json!({ "ok": true, "list_length": len })),
            }
        }

        "lpop" | "rpop" => {
            let cmd_name = if operation == "lpop" { "LPOP" } else { "RPOP" };
            match redis_cmd_with_retry::<Option<String>, _>(&url, || {
                let mut cmd = redis::cmd(cmd_name);
                cmd.arg(&key);
                cmd
            }).await {
                Err(e)  => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("{} failed: {}", cmd_name, e))),
                Ok(val) => NodeOutput::success(json!({ "value": val })),
            }
        }

        "hget" => {
            let field = match input.input["field"].as_str() {
                Some(f) if !f.is_empty() => f.to_string(),
                _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_FIELD",
                    "field is required for hget")),
            };
            match redis_cmd_with_retry::<Option<String>, _>(&url, || {
                let mut cmd = redis::cmd("HGET");
                cmd.arg(&key).arg(&field);
                cmd
            }).await {
                Err(e)  => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("HGET failed: {}", e))),
                Ok(val) => NodeOutput::success(json!({ "value": val })),
            }
        }

        "hset" => {
            let field = match input.input["field"].as_str() {
                Some(f) if !f.is_empty() => f.to_string(),
                _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_FIELD",
                    "field is required for hset")),
            };
            let value = input.input["value"].as_str().unwrap_or("").to_string();
            match redis_cmd_with_retry::<(), _>(&url, || {
                let mut cmd = redis::cmd("HSET");
                cmd.arg(&key).arg(&field).arg(&value);
                cmd
            }).await {
                Err(e) => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("HSET failed: {}", e))),
                Ok(_)  => NodeOutput::success(json!({ "ok": true })),
            }
        }

        other => NodeOutput::failure(NodeError::unrecoverable("UNKNOWN_OPERATION", format!(
            "Unknown Redis operation '{}'. Valid: get, set, del, lpush, rpush, lpop, rpop, hget, hset",
            other
        ))),
    }
}

// ── Shared helpers ────────────────────────────────────────────────────────────

fn parse_params(v: &Value) -> Vec<Value> {
    v.as_str()
        .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
        .or_else(|| v.as_array().cloned())
        .unwrap_or_default()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::validate_db_path;

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
