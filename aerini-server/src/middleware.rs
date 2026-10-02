use axum::http::HeaderValue;
use axum::response::IntoResponse;
use dashmap::DashMap;
use std::collections::VecDeque;
use std::hash::Hash;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// `Instant` is measured from an arbitrary origin (boot time on Windows and
/// macOS), so `now - window` can underflow shortly after boot. When it does,
/// nothing can be older than the window yet and nothing is pruned.
fn evict_stale<K: Eq + Hash>(map: &DashMap<K, VecDeque<Instant>>, window: Duration) {
    let Some(cutoff) = Instant::now().checked_sub(window) else { return };
    map.retain(|_, timestamps| {
        timestamps.retain(|&ts| ts > cutoff);
        !timestamps.is_empty()
    });
}

/// Inline eviction sweeps start once a limiter holds more than this many keys.
const EVICT_THRESHOLD: usize = 10_000;

/// Past this many tracked keys, keys not yet in the map are let through
/// untracked until a sweep brings the map back under the ceiling.
const MAX_TRACKED_KEYS: usize = 50_000;

/// Minimum spacing between inline maintenance runs.
const MAINTENANCE_INTERVAL_MS: u64 = 1_000;

const NEVER_RAN: u64 = u64::MAX;

/// Lock-free throttle for inline maintenance, shared by every limiter.
///
/// `last_ms` is the millisecond offset from `base` of the last run, so a
/// single `compare_exchange` lets exactly one concurrent caller win a slot.
/// `saturated` is refreshed by each run and read on the request path, which
/// keeps the hot path free of any map-wide operation.
struct EvictionGate {
    base:      Instant,
    last_ms:   AtomicU64,
    saturated: AtomicBool,
}

impl EvictionGate {
    fn new() -> Self {
        Self {
            base:      Instant::now(),
            last_ms:   AtomicU64::new(NEVER_RAN),
            saturated: AtomicBool::new(false),
        }
    }

    fn try_acquire(&self, now: Instant) -> bool {
        let elapsed = u64::try_from(now.saturating_duration_since(self.base).as_millis())
            .unwrap_or(NEVER_RAN - 1);
        let last = self.last_ms.load(Ordering::Relaxed);
        if last != NEVER_RAN && elapsed.saturating_sub(last) < MAINTENANCE_INTERVAL_MS {
            return false;
        }
        self.last_ms
            .compare_exchange(last, elapsed, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }
}

/// Runs at most once per interval across all callers. Returns whether a
/// stale-entry sweep ran.
fn maintain<K: Eq + Hash>(
    map:    &DashMap<K, VecDeque<Instant>>,
    gate:   &EvictionGate,
    window: Duration,
    now:    Instant,
) -> bool {
    if !gate.try_acquire(now) {
        return false;
    }
    let sweep = map.len() > EVICT_THRESHOLD;
    if sweep {
        evict_stale(map, window);
    }
    let saturated = map.len() >= MAX_TRACKED_KEYS;
    if gate.saturated.swap(saturated, Ordering::Relaxed) != saturated && saturated {
        tracing::warn!(
            limit = MAX_TRACKED_KEYS,
            "rate limiter key table is full; new keys are not tracked until stale entries expire"
        );
    }
    sweep
}

fn record_hit<K: Eq + Hash>(
    map:          &DashMap<K, VecDeque<Instant>>,
    gate:         &EvictionGate,
    key:          K,
    max_requests: usize,
    window:       Duration,
) -> bool {
    record_hit_at(map, gate, key, max_requests, window, Instant::now())
}

fn record_hit_at<K: Eq + Hash>(
    map:          &DashMap<K, VecDeque<Instant>>,
    gate:         &EvictionGate,
    key:          K,
    max_requests: usize,
    window:       Duration,
    now:          Instant,
) -> bool {
    if gate.saturated.load(Ordering::Relaxed) && !map.contains_key(&key) {
        maintain(map, gate, window, now);
        return true;
    }

    let cutoff = now.checked_sub(window);

    let allowed = {
        let mut entry = map.entry(key).or_default();
        if let Some(cutoff) = cutoff {
            while entry.front().is_some_and(|&ts| ts <= cutoff) {
                entry.pop_front();
            }
        }
        if entry.len() < max_requests {
            entry.push_back(now);
            true
        } else {
            false
        }
    };

    maintain(map, gate, window, now);
    allowed
}

/// Per-IP sliding-window rate limiter backed by a `DashMap`.
///
/// Each IP address entry holds a `VecDeque<Instant>` of accepted request
/// timestamps within the current window. On each call to `is_allowed`:
/// 1. Timestamps older than `window_secs` are pruned from the front.
/// 2. If fewer than `max_requests` timestamps remain, the request is accepted
///    and the current timestamp is appended.
/// 3. Otherwise the request is rejected.
///
/// This is a true sliding-window implementation: there is no 2× burst at
/// the window boundary because the window is measured from each individual
/// request's timestamp, not from an epoch boundary.
///
/// Memory: each entry holds at most `max_requests` timestamps. Stale entries
/// are evicted by a background task (see `spawn_eviction_task`). Once the map
/// holds more than 10 000 IPs, requests also trigger an inline sweep, at most
/// one per second, so the map can exceed 10 000 entries between sweeps. If
/// 50 000 IPs are tracked at once, further IPs are let through untracked
/// (fail open) until a sweep frees space; tracked IPs stay limited.
pub struct RateLimiter {
    map:          DashMap<IpAddr, VecDeque<Instant>>,
    gate:         EvictionGate,
    max_requests: usize,
    window:       Duration,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            map:          DashMap::new(),
            gate:         EvictionGate::new(),
            max_requests: max_requests as usize,
            window:       Duration::from_secs(window_secs),
        }
    }

    /// Spawns a Tokio background task that evicts stale entries every
    /// `window_secs` seconds. Call once after creating the limiter.
    /// Without this, IPs with no traffic accumulate as empty `VecDeque`
    /// entries and leak memory under sustained IP churn (e.g. a botnet scan).
    pub fn spawn_eviction_task(self: Arc<Self>) {
        let weak = Arc::downgrade(&self);
        let interval = self.window;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await; // skip the immediate first tick
            loop {
                ticker.tick().await;
                match weak.upgrade() {
                    None => break,
                    Some(rl) => evict_stale(&rl.map, rl.window),
                }
            }
        });
    }

    /// Returns `true` if the request from `ip` is within the rate limit.
    pub fn is_allowed(&self, ip: IpAddr) -> bool {
        record_hit(&self.map, &self.gate, ip, self.max_requests, self.window)
    }
}

const MAX_KEY_BYTES: usize = 128;

fn truncate_key(key: &str) -> &str {
    if key.len() <= MAX_KEY_BYTES {
        return key;
    }
    let mut end = MAX_KEY_BYTES;
    while !key.is_char_boundary(end) {
        end -= 1;
    }
    &key[..end]
}

/// Same sliding-window algorithm as [`RateLimiter`], keyed by `String`
/// instead of `IpAddr`. Used for the widget trigger route's per-workflow_id
/// limit (routes::widget), where the key is caller-supplied and
/// unauthenticated, so it is truncated to 128 bytes before use. Memory
/// behaviour matches `RateLimiter`: inline sweeps at most once per second above
/// 10 000 keys, and unseen keys pass untracked while 50 000 are tracked.
pub struct KeyedRateLimiter {
    map:          DashMap<String, VecDeque<Instant>>,
    gate:         EvictionGate,
    max_requests: usize,
    window:       Duration,
}

impl KeyedRateLimiter {
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            map:          DashMap::new(),
            gate:         EvictionGate::new(),
            max_requests: max_requests as usize,
            window:       Duration::from_secs(window_secs),
        }
    }

    pub fn spawn_eviction_task(self: Arc<Self>) {
        let weak = Arc::downgrade(&self);
        let interval = self.window;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                match weak.upgrade() {
                    None => break,
                    Some(rl) => evict_stale(&rl.map, rl.window),
                }
            }
        });
    }

    /// Returns `true` if a request keyed by `key` is within the rate limit.
    /// `key` is truncated to 128 bytes first (see struct doc).
    pub fn is_allowed(&self, key: &str) -> bool {
        record_hit(&self.map, &self.gate, truncate_key(key).to_owned(), self.max_requests, self.window)
    }
}

/// `429` response shared by every rate-limit layer. Every limiter in this
/// server uses a 60-second sliding window, so `Retry-After: 60` is the upper
/// bound on how long a caller must wait before a slot frees up.
pub fn too_many_requests() -> axum::response::Response {
    let mut resp = (
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        axum::Json(serde_json::json!({"error": "rate limit exceeded"})),
    )
        .into_response();
    resp.headers_mut()
        .insert(axum::http::header::RETRY_AFTER, HeaderValue::from(60u64));
    resp
}

fn insert_common_security_headers(h: &mut axum::http::HeaderMap) {
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("x-frame-options",        HeaderValue::from_static("DENY"));
    h.insert("referrer-policy",        HeaderValue::from_static("strict-origin-when-cross-origin"));
    h.insert("permissions-policy",     HeaderValue::from_static("camera=(), microphone=(), geolocation=()"));
    // Intentionally set even when behind a TLS-terminating reverse proxy; the
    // proxy strips this header on plain-HTTP internal connections, which is
    // correct. Without it, a proxy misconfiguration or removal leaves clients
    // with no cached HTTPS-enforcement directive.
    h.insert(
        "strict-transport-security",
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
}

/// Security headers for the JSON API server, plus the one static asset
/// (`/aerini-widget.js`) that is intentionally registered outside the
/// `protected` router (see `api_server/mod.rs` route table doc comment).
///
/// This must stay one layer, outermost on `app` (added last in
/// `api_server/mod.rs`). `CorsLayer` answers preflight (`OPTIONS`) requests
/// itself without calling the inner service (tower-http issue #497), so any
/// layer placed outside it would stop `/api/*` preflight responses from
/// getting these headers. It branches by path instead of splitting into two
/// layers for the same reason.
///
/// CSP is `default-src 'none'` for the JSON API — it returns only JSON, never
/// HTML. The widget asset is excluded from that CSP value: a
/// `content-security-policy` response header on a plain `<script src>`
/// resource is inert (CSP governs what a *document* may load and execute,
/// not how a fetched script asset is itself cached or run), so omitting it
/// here changes no browser-enforced behavior — it would just be a header the
/// browser ignores on this response.
///
/// `/api/health` deliberately stays under the strict `no-store` branch even
/// though it is also unauthenticated: it exists to be polled by load
/// balancers and uptime monitors, and a cached stale "ok" would defeat that
/// purpose by hiding a real outage.
pub async fn api_security_headers(
    req:  axum::extract::Request,
    next: axum::middleware::Next,
) -> impl IntoResponse {
    let is_widget_asset = req.method().as_str() == "GET"
        && req.uri().path() == "/aerini-widget.js";
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    insert_common_security_headers(h);
    if is_widget_asset {
        // No content hash in the URL to bust on deploy, so this bounds how
        // long an updated widget can take to reach embedders' visitors.
        h.insert("cache-control", HeaderValue::from_static("public, max-age=300"));
    } else {
        h.insert("content-security-policy", HeaderValue::from_static("default-src 'none'"));
        h.insert("cache-control", HeaderValue::from_static("no-store, no-cache, must-revalidate"));
    }
    response
}

/// SHA-256 hash (base64 standard) of `STATUS_PAGE_CSS` in status_server.rs.
/// Computed once on first use directly from the actual CSS bytes, so it can
/// never drift out of sync with the content it covers.
fn csp_css_hash() -> &'static str {
    use sha2::{Sha256, Digest};
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| {
        BASE64.encode(Sha256::digest(crate::status_server::STATUS_PAGE_CSS.as_bytes()))
    })
}

/// SHA-256 hash (base64 standard) of `STATUS_PAGE_SCRIPT` in status_server.rs.
/// Computed once on first use directly from the actual script bytes, so it can
/// never drift out of sync with the content it covers.
fn csp_script_hash() -> &'static str {
    use sha2::{Sha256, Digest};
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| {
        BASE64.encode(Sha256::digest(crate::status_server::STATUS_PAGE_SCRIPT.as_bytes()))
    })
}

/// Security headers for the status page server.
/// CSP includes SHA-256 hashes for the inline CSS and JS blocks rendered by
/// `status_page()`, computed directly from `STATUS_PAGE_CSS` / `STATUS_PAGE_SCRIPT`
/// in status_server.rs — always in sync, no manual recomputation needed.
pub async fn status_security_headers(
    req:  axum::extract::Request,
    next: axum::middleware::Next,
) -> impl IntoResponse {
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    insert_common_security_headers(h);
    h.insert(
        "content-security-policy",
        HeaderValue::from_str(&format!(
            "default-src 'none'; \
             style-src 'sha256-{}'; \
             script-src 'sha256-{}'; \
             frame-ancestors 'none'",
            csp_css_hash(), csp_script_hash(),
        )).expect("CSP header value is always valid ASCII"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::{
        maintain, record_hit_at, too_many_requests, truncate_key, EvictionGate, KeyedRateLimiter,
        RateLimiter, EVICT_THRESHOLD, MAINTENANCE_INTERVAL_MS, MAX_TRACKED_KEYS,
    };
    use dashmap::DashMap;
    use std::collections::VecDeque;
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::{Duration, Instant};

    const WINDOW: Duration = Duration::from_secs(60);

    fn live_map(n: usize, at: Instant) -> DashMap<u32, VecDeque<Instant>> {
        let map = DashMap::new();
        for i in 0..n {
            map.insert(i as u32, VecDeque::from([at]));
        }
        map
    }

    #[test]
    fn too_many_requests_sets_status_and_retry_after() {
        let resp = too_many_requests();
        assert_eq!(resp.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers().get("retry-after").unwrap(), "60");
    }

    // A window longer than any possible uptime forces `now - window` to underflow.
    const UNDERFLOWING_WINDOW_SECS: u64 = u64::MAX / 4;

    #[test]
    fn rate_limiter_survives_window_exceeding_uptime() {
        let rl = RateLimiter::new(2, UNDERFLOWING_WINDOW_SECS);
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert!(rl.is_allowed(ip));
        assert!(rl.is_allowed(ip));
        assert!(!rl.is_allowed(ip));
    }

    #[test]
    fn keyed_rate_limiter_survives_window_exceeding_uptime() {
        let rl = KeyedRateLimiter::new(2, UNDERFLOWING_WINDOW_SECS);
        assert!(rl.is_allowed("wf"));
        assert!(rl.is_allowed("wf"));
        assert!(!rl.is_allowed("wf"));
        for i in 0..10_001 {
            rl.is_allowed(&format!("k{i}"));
        }
        assert!(!rl.is_allowed("wf"));
    }

    #[test]
    fn keyed_limiter_key_cap_is_bytes_on_char_boundary() {
        assert_eq!(truncate_key("short"), "short");
        assert_eq!(truncate_key(&"a".repeat(200)).len(), 128);
        assert_eq!(truncate_key(&"\u{e9}".repeat(100)).len(), 128);
        assert_eq!(truncate_key(&"\u{20ac}".repeat(50)).len(), 126);
    }

    #[test]
    fn inline_sweep_runs_once_per_interval() {
        let gate = EvictionGate::new();
        let t0 = gate.base;
        let map = live_map(EVICT_THRESHOLD + 1, t0);
        let sweeps = (0..MAINTENANCE_INTERVAL_MS)
            .filter(|&ms| maintain(&map, &gate, WINDOW, t0 + Duration::from_millis(ms)))
            .count();
        assert_eq!(sweeps, 1);
    }

    #[test]
    fn inline_sweep_runs_again_after_interval() {
        let gate = EvictionGate::new();
        let t0 = gate.base;
        let map = live_map(EVICT_THRESHOLD + 1, t0);
        let interval = Duration::from_millis(MAINTENANCE_INTERVAL_MS);
        assert!(maintain(&map, &gate, WINDOW, t0));
        assert!(!maintain(&map, &gate, WINDOW, t0 + interval - Duration::from_millis(1)));
        assert!(maintain(&map, &gate, WINDOW, t0 + interval));
        assert!(!maintain(&map, &gate, WINDOW, t0 + interval));
    }

    #[test]
    fn limit_holds_while_sweeps_are_skipped() {
        let gate = EvictionGate::new();
        let t0 = gate.base;
        let map = live_map(EVICT_THRESHOLD + 1, t0);
        assert!(maintain(&map, &gate, WINDOW, t0));
        let results: Vec<bool> = (0..5)
            .map(|ms| {
                let now = t0 + Duration::from_millis(ms);
                record_hit_at(&map, &gate, u32::MAX, 3, WINDOW, now)
            })
            .collect();
        assert_eq!(results, [true, true, true, false, false]);
    }

    #[test]
    fn full_table_admits_unseen_keys_untracked_and_keeps_limiting_tracked_ones() {
        let gate = EvictionGate::new();
        let t0 = gate.base;
        let map = live_map(MAX_TRACKED_KEYS, t0);
        map.insert(u32::MAX, VecDeque::from([t0; 3]));
        assert!(maintain(&map, &gate, WINDOW, t0));
        assert!(record_hit_at(&map, &gate, u32::MAX - 1, 3, WINDOW, t0));
        assert!(!map.contains_key(&(u32::MAX - 1)));
        assert!(!record_hit_at(&map, &gate, u32::MAX, 3, WINDOW, t0));
    }
}
