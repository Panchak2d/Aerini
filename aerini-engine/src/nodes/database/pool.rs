use dashmap::DashMap;
use once_cell::sync::Lazy;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::nodes::util::{scrub_url_in_error, Replay, SsrfPolicy, SsrfRedisResolver};

const POOL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REDIS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REDIS_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Most pools or connections cached per backend. A workflow that dials a
/// different URL or file each run replaces the least recently used entry
/// instead of growing the cache until the idle timeout.
const MAX_CACHED_POOLS: usize = 64;

fn make_room<P>(map: &mut HashMap<String, PoolEntry<P>>) {
    if map.len() < MAX_CACHED_POOLS {
        return;
    }
    let oldest = map.iter().min_by_key(|(_, e)| e.last_used).map(|(k, _)| k.clone());
    if let Some(key) = oldest {
        map.remove(&key);
    }
}

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

/// Pools that need an async connect to create. Each key is dialled by one task
/// at a time, so concurrent callers for the same URL share one connection
/// attempt, while a slow or unreachable host never delays a different one.
struct PoolCache<P> {
    ready:   Mutex<HashMap<String, PoolEntry<P>>>,
    dialing: DashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

impl<P: Clone> PoolCache<P> {
    fn new() -> Self {
        Self { ready: Mutex::new(HashMap::new()), dialing: DashMap::new() }
    }

    fn ready(&self) -> std::sync::MutexGuard<'_, HashMap<String, PoolEntry<P>>> {
        self.ready.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn touch(&self, key: &str) -> Option<P> {
        self.ready().get_mut(key).map(|entry| {
            entry.last_used = Instant::now();
            entry.pool.clone()
        })
    }

    fn remove(&self, key: &str) {
        self.ready().remove(key);
    }

    fn retain_recent(&self, max_idle: Duration) {
        self.ready().retain(|_, entry| entry.last_used.elapsed() < max_idle);
        self.dialing.retain(|_, lock| Arc::strong_count(lock) > 1);
    }

    async fn get_or_connect<F, Fut>(&self, key: String, connect: F) -> Result<P, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<P, String>>,
    {
        if let Some(pool) = self.touch(&key) {
            return Ok(pool);
        }
        let dial_lock = Arc::clone(&*self.dialing.entry(key.clone()).or_default());
        let outcome = {
            let _turn = dial_lock.lock().await;
            match self.touch(&key) {
                Some(pool) => Ok(pool),
                None => connect().await.inspect(|pool| {
                    let mut ready = self.ready();
                    make_room(&mut ready);
                    ready.insert(key.clone(), PoolEntry { pool: pool.clone(), last_used: Instant::now() });
                }),
            }
        };
        self.dialing.remove(&key);
        outcome
    }
}

static PG_POOLS:    Lazy<PoolCache<sqlx::PgPool>>    = Lazy::new(PoolCache::new);
static MYSQL_POOLS: Lazy<PoolCache<sqlx::MySqlPool>> = Lazy::new(PoolCache::new);

/// Redis client objects hold no connection state — cheap to create, no async op.
static REDIS_CLIENTS: Lazy<DashMap<String, redis::Client>> = Lazy::new(DashMap::new);

/// Redis multiplexed connections, cached like the Postgres and MySQL pools.
static REDIS_CONNS: Lazy<PoolCache<redis::aio::MultiplexedConnection>> = Lazy::new(PoolCache::new);

// ── Connection target check ───────────────────────────────────────────────────

/// SSRF check on the host and port the Postgres or MySQL driver will dial. The
/// driver lets query parameters (`?host=`, `?hostaddr=`, `?port=`, `?socket=`)
/// override the URL's own host, so the URL host alone proves nothing. Unix
/// sockets are refused: they bypass every network check.
pub(super) async fn check_sqlx_target(url: &str, mysql: bool, policy: SsrfPolicy) -> Result<(), String> {
    use std::str::FromStr;
    let (host, port, socket) = if mysql {
        let o = sqlx::mysql::MySqlConnectOptions::from_str(url)
            .map_err(|e| scrub_url_in_error(&format!("Invalid connection URL: {}", e)))?;
        (o.get_host().to_string(), o.get_port(), o.get_socket().is_some())
    } else {
        let o = sqlx::postgres::PgConnectOptions::from_str(url)
            .map_err(|e| scrub_url_in_error(&format!("Invalid connection URL: {}", e)))?;
        (o.get_host().to_string(), o.get_port(), o.get_socket().is_some() || o.get_host().starts_with('/'))
    };
    if socket {
        return Err("Unix socket connections are not permitted. Use a host name or IP address.".to_string());
    }
    crate::nodes::util::check_db_host_ssrf(&host, port, policy).await
}

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
        .min_idle(Some(0))
        .connection_timeout(POOL_CONNECT_TIMEOUT)
        .build(manager)
        .map_err(|e| format!("Could not create pool for '{}': {}", path, e))?;
    if SQLITE_POOLS.len() >= MAX_CACHED_POOLS {
        let oldest = SQLITE_POOLS.iter().min_by_key(|e| e.value().last_used).map(|e| e.key().clone());
        if let Some(key) = oldest {
            SQLITE_POOLS.remove(&key);
        }
    }
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
                REDIS_CLIENTS.clear();
                SQLITE_POOLS.retain(|_, v| v.last_used.elapsed() < evict_after);
                PG_POOLS.retain_recent(evict_after);
                MYSQL_POOLS.retain_recent(evict_after);
                REDIS_CONNS.retain_recent(evict_after);
            }
        });
    });
}

pub(super) async fn get_pg_pool(url: &str) -> Result<sqlx::PgPool, String> {
    PG_POOLS.get_or_connect(pool_key(url), || async {
        sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(POOL_CONNECT_TIMEOUT)
            .connect(url)
            .await
            .map_err(|e| scrub_url_in_error(&format!("Postgres connection failed: {}", e)))
    })
    .await
}

pub(super) async fn get_mysql_pool(url: &str) -> Result<sqlx::MySqlPool, String> {
    MYSQL_POOLS.get_or_connect(pool_key(url), || async {
        sqlx::mysql::MySqlPoolOptions::new()
            .acquire_timeout(POOL_CONNECT_TIMEOUT)
            .connect(url)
            .await
            .map_err(|e| scrub_url_in_error(&format!("MySQL connection failed: {}", e)))
    })
    .await
}

// ── Redis client + connection helpers ────────────────────────────────────────

fn get_redis_client(url: &str) -> Result<redis::Client, String> {
    let key = pool_key(url);
    if let Some(c) = REDIS_CLIENTS.get(&key) {
        return Ok(c.clone());
    }
    let client = redis::Client::open(url)
        .map_err(|e| scrub_url_in_error(&format!("Redis URL invalid: {}", e)))?;
    if REDIS_CLIENTS.len() >= MAX_CACHED_POOLS {
        REDIS_CLIENTS.clear();
    }
    let entry = REDIS_CLIENTS.entry(key).or_insert(client);
    Ok(entry.clone())
}

/// Returns a cached MultiplexedConnection for `url`, creating one if absent.
async fn get_redis_conn(url: &str, policy: SsrfPolicy) -> Result<redis::aio::MultiplexedConnection, String> {
    REDIS_CONNS.get_or_connect(redis_pool_key(url, policy), || async {
        let client = get_redis_client(url)?;
        let config = redis::AsyncConnectionConfig::new().set_dns_resolver(SsrfRedisResolver(policy));
        tokio::time::timeout(
            REDIS_CONNECT_TIMEOUT,
            client.get_multiplexed_async_connection_with_config(&config),
        )
        .await
        .map_err(|_| "Redis connection failed: timed out".to_string())?
        .map_err(|e| scrub_url_in_error(&format!("Redis connection failed: {}", e)))
    })
    .await
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
            REDIS_CONNS.remove(&redis_pool_key(url, policy));
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

#[cfg(test)]
mod cache_cap_tests {
    use super::{make_room, PoolEntry, MAX_CACHED_POOLS};
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    fn filled_map() -> HashMap<String, PoolEntry<()>> {
        let base = Instant::now();
        (0..MAX_CACHED_POOLS)
            .map(|i| (i.to_string(), PoolEntry { pool: (), last_used: base + Duration::from_millis(i as u64) }))
            .collect()
    }

    #[test]
    fn full_cache_drops_least_recently_used_entry() {
        let mut map = filled_map();
        make_room(&mut map);
        assert_eq!(map.len(), MAX_CACHED_POOLS - 1);
        assert!(!map.contains_key("0"));
        assert!(map.contains_key("1"));
    }

    #[tokio::test]
    async fn slow_connect_blocks_only_its_own_key_and_a_key_is_dialled_once() {
        use super::PoolCache;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let cache: Arc<PoolCache<u32>> = Arc::new(PoolCache::new());
        let dials = Arc::new(AtomicUsize::new(0));
        let (release, gate) = tokio::sync::oneshot::channel::<()>();

        let slow = {
            let (cache, dials) = (Arc::clone(&cache), Arc::clone(&dials));
            tokio::spawn(async move {
                cache
                    .get_or_connect("a".to_string(), || async {
                        dials.fetch_add(1, Ordering::SeqCst);
                        gate.await.ok();
                        Ok(1)
                    })
                    .await
            })
        };
        while dials.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }

        let other = tokio::time::timeout(
            Duration::from_secs(1),
            cache.get_or_connect("b".to_string(), || async { Ok(2) }),
        )
        .await
        .expect("a different key must not wait for the slow connect");
        assert_eq!(other, Ok(2));

        let waiter = {
            let (cache, dials) = (Arc::clone(&cache), Arc::clone(&dials));
            tokio::spawn(async move {
                cache
                    .get_or_connect("a".to_string(), || async {
                        dials.fetch_add(1, Ordering::SeqCst);
                        Ok(9)
                    })
                    .await
            })
        };
        tokio::task::yield_now().await;
        release.send(()).unwrap();

        assert_eq!(slow.await.unwrap(), Ok(1));
        assert_eq!(waiter.await.unwrap(), Ok(1));
        assert_eq!(dials.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cache_below_cap_is_left_alone() {
        let mut map = filled_map();
        map.remove("5");
        make_room(&mut map);
        assert_eq!(map.len(), MAX_CACHED_POOLS - 1);
        assert!(map.contains_key("0"));
    }
}
