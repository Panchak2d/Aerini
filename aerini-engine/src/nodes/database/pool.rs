use dashmap::DashMap;
use once_cell::sync::Lazy;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::nodes::util::scrub_url_in_error;

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

/// PG and MySQL pool maps use `Mutex<HashMap>` rather than DashMap so that
/// pool creation is atomic: the lock is held across the async `connect()` call,
/// preventing two concurrent callers from both racing to open a new connection
/// to the same URL. The lock is released immediately after insertion, so it is
/// only contended during first-connect to a new URL.
static PG_POOLS:    Lazy<Mutex<HashMap<String, PoolEntry<sqlx::PgPool>>>>    = Lazy::new(|| Mutex::new(HashMap::new()));
static MYSQL_POOLS: Lazy<Mutex<HashMap<String, PoolEntry<sqlx::MySqlPool>>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Redis client objects hold no connection state — cheap to create, no async op.
static REDIS_CLIENTS: Lazy<DashMap<String, redis::Client>> = Lazy::new(DashMap::new);

/// Redis multiplexed connections use `Mutex<HashMap>` for the same reason as
/// PG/MySQL: creation requires an async connect that must not be raced.
static REDIS_CONNS: Lazy<Mutex<HashMap<String, PoolEntry<redis::aio::MultiplexedConnection>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// ── SQLite helpers ────────────────────────────────────────────────────────────

/// Validates a db_path before opening. Rejects paths that are:
/// - relative (must be absolute — prevents opening files relative to cwd)
/// - missing a recognised SQLite extension (.db / .sqlite / .sqlite3)
/// - containing path traversal sequences (..)
pub(super) fn validate_db_path(path: &str) -> Result<(), String> {
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

pub(super) fn get_sqlite_pool(path: &str) -> Result<Pool<SqliteConnectionManager>, String> {
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

/// Spawns a background task that evicts idle sqlx and Redis pools every 5 minutes.
/// Call once at application startup (e.g. in main() or plugin setup).
/// Safe to call multiple times — extra calls are no-ops after the first spawn.
///
/// `rt` must be a handle to a running Tokio runtime. Pass
/// `tokio::runtime::Handle::current()` when inside `#[tokio::main]`, or
/// `tauri::async_runtime::handle().inner()` inside Tauri's `setup()` callback.
pub fn start_pool_eviction_task(rt: &tokio::runtime::Handle) {
    use std::sync::Once;
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        rt.spawn(async {
            let evict_after = Duration::from_secs(1800);
            loop {
                tokio::time::sleep(Duration::from_secs(300)).await;
                PG_POOLS.lock().await.retain(|_, v| v.last_used.elapsed() < evict_after);
                MYSQL_POOLS.lock().await.retain(|_, v| v.last_used.elapsed() < evict_after);
                REDIS_CONNS.lock().await.retain(|_, v| v.last_used.elapsed() < evict_after);
            }
        });
    });
}

pub(super) async fn get_pg_pool(url: &str) -> Result<sqlx::PgPool, String> {
    let key = pool_key(url);
    let mut map = PG_POOLS.lock().await;
    if let Some(entry) = map.get_mut(&key) {
        entry.last_used = Instant::now();
        return Ok(entry.pool.clone());
    }
    let pool = sqlx::PgPool::connect(url)
        .await
        .map_err(|e| scrub_url_in_error(&format!("Postgres connection failed: {}", e)))?;
    map.insert(key, PoolEntry { pool: pool.clone(), last_used: Instant::now() });
    Ok(pool)
}

pub(super) async fn get_mysql_pool(url: &str) -> Result<sqlx::MySqlPool, String> {
    let key = pool_key(url);
    let mut map = MYSQL_POOLS.lock().await;
    if let Some(entry) = map.get_mut(&key) {
        entry.last_used = Instant::now();
        return Ok(entry.pool.clone());
    }
    let pool = sqlx::MySqlPool::connect(url)
        .await
        .map_err(|e| scrub_url_in_error(&format!("MySQL connection failed: {}", e)))?;
    map.insert(key, PoolEntry { pool: pool.clone(), last_used: Instant::now() });
    Ok(pool)
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
    let mut map = REDIS_CONNS.lock().await;
    if let Some(entry) = map.get_mut(&key) {
        entry.last_used = Instant::now();
        return Ok(entry.pool.clone());
    }
    let client = get_redis_client(url)?;
    let conn = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| scrub_url_in_error(&format!("Redis connection failed: {}", e)))?;
    map.insert(key, PoolEntry { pool: conn.clone(), last_used: Instant::now() });
    Ok(conn)
}

/// Runs `build_cmd()` on a cached connection for `url`.
/// On IoError: evicts the stale connection, reconnects, retries exactly once.
pub(super) async fn redis_cmd_with_retry<T, F>(url: &str, build_cmd: F) -> Result<T, redis::RedisError>
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
            REDIS_CONNS.lock().await.remove(&pool_key(url));
            let mut fresh = get_redis_conn(url).await.map_err(|ce| {
                redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::NotConnected, ce))
            })?;
            build_cmd().query_async::<T>(&mut fresh).await
        }
        Err(e) => Err(e),
    }
}
