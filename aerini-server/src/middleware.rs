use axum::http::HeaderValue;
use axum::response::IntoResponse;
use dashmap::DashMap;
use std::collections::VecDeque;
use std::hash::Hash;
use std::net::IpAddr;
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

fn record_hit<K: Eq + Hash>(
    map:          &DashMap<K, VecDeque<Instant>>,
    key:          K,
    max_requests: usize,
    window:       Duration,
) -> bool {
    let now    = Instant::now();
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

    // Best-effort memory bound between background eviction cycles.
    if map.len() > 10_000 {
        evict_stale(map, window);
    }

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
/// Memory: each entry holds at most `max_requests` timestamps. The map is
/// bounded to 10 000 active IPs, with stale entries evicted via a background
/// task (see `spawn_eviction_task`).
pub struct RateLimiter {
    map:          DashMap<IpAddr, VecDeque<Instant>>,
    max_requests: usize,
    window:       Duration,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            map:          DashMap::new(),
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
    ///
    /// Inline eviction fires when the map exceeds 10 000 entries to bound
    /// memory usage between background eviction cycles.
    pub fn is_allowed(&self, ip: IpAddr) -> bool {
        record_hit(&self.map, ip, self.max_requests, self.window)
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

/// Same sliding-window algorithm as [`RateLimiter`], keyed by an arbitrary
/// `String` instead of `IpAddr`. Not merged into `RateLimiter` as a generic:
/// `RateLimiter` is public API with its own call sites and tests fixed to
/// `IpAddr`, and introducing a type parameter there would touch every one of
/// them for a single new caller. This exists for the widget trigger route's
/// per-workflow_id limit (routes::widget), where the key is caller-supplied
/// and unauthenticated, so it is capped at 128 bytes before use — same
/// reasoning as any untrusted-input-as-map-key case, unbounded key length is
/// an unbounded-memory footgun.
pub struct KeyedRateLimiter {
    map:          DashMap<String, VecDeque<Instant>>,
    max_requests: usize,
    window:       Duration,
}

impl KeyedRateLimiter {
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            map:          DashMap::new(),
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
        record_hit(&self.map, truncate_key(key).to_owned(), self.max_requests, self.window)
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
/// This stays a single layer at its original position — the outermost layer
/// on `app`, added last in `api_server/mod.rs` — rather than being split into
/// two Router-level layers around the CORS/rate-limit/body-limit stack.
/// Splitting it would reorder `cors` to be outermost, and `CorsLayer` returns
/// preflight (`OPTIONS`) responses directly without calling the inner
/// service — confirmed via tower-http issue #497. That would mean `/api/*`
/// preflight responses stop getting these headers entirely, not just a
/// different `cache-control` value. Branching by path here keeps every other
/// route's layering, and therefore its preflight behaviour, byte-identical.
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
    use super::{too_many_requests, truncate_key, KeyedRateLimiter, RateLimiter};
    use std::net::{IpAddr, Ipv4Addr};

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
}
