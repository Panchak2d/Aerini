use dashmap::DashMap;
use once_cell::sync::Lazy;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::nodes::util::{scrub_url_in_error, Replay, SsrfPolicy, SsrfRedisResolver};

const POOL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REDIS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REDIS_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Returns a BLAKE3 hex hash of the connection URL, used as the DashMap pool
/// cache key. This prevents the plaintext URL (which may contain a password)
/// from being stored as a map key in process memory.
fn pool_key(url: &str) -> String {
    blake3::hash(url.as_bytes()).to_hex().to_string()
}

/// A connection dialed under `AllowLocal` must not be reused by a `Strict` caller.
fn redis_pool_key(url: &str, policy: SsrfPolicy) -> String {
    match policy {
        SsrfPolicy::Strict => pool_key(url),
        SsrfPolicy::AllowLocal => format!("{}:local", pool_key(url)),
    }
}

// ── Connection pool registries ────────────────────────────────────────────────

/// Wraps a pool with a last-used timestamp for idle eviction.
struct PoolEntry<P> {
    pool:      P,
    last_used: Instant,
}

/// SQLite pools are evicted on the same idle-timeout schedule as the other
/// backends (see `start_pool_eviction_task`) rather than kept forever: a
/// workflow that generates a distinct `db_path` per run (e.g. a per-iteration
/// output file inside a loop) would otherwise grow this cache — and its
/// backing r2d2 pools/file handles — without bound.
///
/// Evicting the cache *entry* does not touch a connection already checked
/// out by an in-flight query: r2d2's `Pool` is `Clone` over a shared,
/// internally ref-counted pool, and the clone held inside a running
/// `spawn_blocking` task keeps that pool alive independently of this map.
/// Only a later caller pays the cost of reopening. WAL-mode files are safe
/// to reopen at any time, so no in-progress transaction is disrupted.
static SQLITE_POOLS: Lazy<DashMap<String, PoolEntry<Pool<SqliteConnectionManager>>>> =
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
/// - network (UNC) paths such as \\host\share, which make the OS open a network connection
pub(super) fn validate_db_path(path: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    if !p.is_absolute() {
        return Err(
            "db_path must be an absolute path. Example: /home/user/myapp.db".to_string()
        );
    }
    if path.starts_with("\\\\") || path.starts_with("//") {
        return Err("db_path must be a local path, not a network (UNC) path.".to_string());
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
    if let Some(mut entry) = SQLITE_POOLS.get_mut(path) {
        entry.last_used = Instant::now();
        return Ok(entry.pool.clone());
    }
    let manager = SqliteConnectionManager::file(path)
        .with_init(|conn| conn.execute_batch("PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL;"));
    let pool = Pool::builder()
        .max_size(4)
        .build(manager)
        .map_err(|e| format!("Could not create pool for '{}': {}", path, e))?;
    SQLITE_POOLS.insert(path.to_string(), PoolEntry { pool: pool.clone(), last_used: Instant::now() });
    Ok(pool)
}

// ── sqlx pool (Postgres / MySQL) ──────────────────────────────────────────────

/// Spawns a background task that evicts idle SQLite, sqlx, and Redis pools every 5 minutes.
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
                SQLITE_POOLS.retain(|_, v| v.last_used.elapsed() < evict_after);
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
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(POOL_CONNECT_TIMEOUT)
        .connect(url)
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
    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .acquire_timeout(POOL_CONNECT_TIMEOUT)
        .connect(url)
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
async fn get_redis_conn(url: &str, policy: SsrfPolicy) -> Result<redis::aio::MultiplexedConnection, String> {
    let key = redis_pool_key(url, policy);
    let mut map = REDIS_CONNS.lock().await;
    if let Some(entry) = map.get_mut(&key) {
        entry.last_used = Instant::now();
        return Ok(entry.pool.clone());
    }
    let client = get_redis_client(url)?;
    let config = redis::AsyncConnectionConfig::new().set_dns_resolver(SsrfRedisResolver(policy));
    let conn = tokio::time::timeout(
        REDIS_CONNECT_TIMEOUT,
        client.get_multiplexed_async_connection_with_config(&config),
    )
    .await
    .map_err(|_| "Redis connection failed: timed out".to_string())?
    .map_err(|e| scrub_url_in_error(&format!("Redis connection failed: {}", e)))?;
    map.insert(key, PoolEntry { pool: conn.clone(), last_used: Instant::now() });
    Ok(conn)
}

/// Runs `build_cmd()` on a cached connection for `url`, bounded by a command timeout.
/// On IoError (including a timeout): evicts the stale connection. For
/// `Replay::Safe` commands it then reconnects and retries exactly once; for
/// `Replay::Never` commands (pushes and pops, which a second run would
/// duplicate or repeat) the error is returned, because the first attempt may
/// already have taken effect.
pub(super) async fn redis_cmd_with_retry<T, F>(url: &str, policy: SsrfPolicy, replay: Replay, build_cmd: F) -> Result<T, redis::RedisError>
where
    T: redis::FromRedisValue,
    F: Fn() -> redis::Cmd + Send,
{
    let mut conn = get_redis_conn(url, policy).await.map_err(|e| {
        redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::NotConnected, e))
    })?;

    match run_bounded::<T>(build_cmd(), &mut conn).await {
        Ok(val) => Ok(val),
        Err(e) if e.is_connection_dropped() || e.is_io_error() => {
            REDIS_CONNS.lock().await.remove(&redis_pool_key(url, policy));
            if replay == Replay::Never {
                return Err(e);
            }
            let mut fresh = get_redis_conn(url, policy).await.map_err(|ce| {
                redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::NotConnected, ce))
            })?;
            run_bounded::<T>(build_cmd(), &mut fresh).await
        }
        Err(e) => Err(e),
    }
}

async fn run_bounded<T: redis::FromRedisValue>(
    cmd: redis::Cmd,
    conn: &mut redis::aio::MultiplexedConnection,
) -> Result<T, redis::RedisError> {
    match tokio::time::timeout(REDIS_COMMAND_TIMEOUT, cmd.query_async::<T>(conn)).await {
        Ok(result) => result,
        Err(_) => Err(redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "command timed out",
        ))),
    }
}
