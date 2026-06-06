use axum::http::HeaderValue;
use axum::response::IntoResponse;
use dashmap::DashMap;
use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
                    Some(rl) => {
                        let cutoff = Instant::now() - rl.window;
                        // Remove entries that have no timestamps within the window.
                        rl.map.retain(|_, timestamps| {
                            timestamps.retain(|&ts| ts > cutoff);
                            !timestamps.is_empty()
                        });
                    }
                }
            }
        });
    }

    /// Returns `true` if the request from `ip` is within the rate limit.
    ///
    /// Inline eviction fires when the map exceeds 10 000 entries to bound
    /// memory usage between background eviction cycles.
    pub fn is_allowed(&self, ip: IpAddr) -> bool {
        let now    = Instant::now();
        let cutoff = now - self.window;

        let allowed = {
            let mut entry = self.map.entry(ip).or_insert_with(VecDeque::new);
            // Prune timestamps outside the sliding window.
            while entry.front().map(|&ts| ts <= cutoff).unwrap_or(false) {
                entry.pop_front();
            }
            if entry.len() < self.max_requests {
                entry.push_back(now);
                true
            } else {
                false
            }
        };

        // Inline eviction: if the map has grown very large, prune all stale entries.
        // This is a best-effort bound on memory between background eviction cycles.
        if self.map.len() > 10_000 {
            let cutoff2 = now - self.window;
            self.map.retain(|_, timestamps| {
                timestamps.retain(|&ts| ts > cutoff2);
                !timestamps.is_empty()
            });
        }

        allowed
    }
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

/// Security headers for the JSON API server.
/// CSP is `default-src 'none'` — the API returns only JSON, never HTML.
pub async fn api_security_headers(
    req:  axum::extract::Request,
    next: axum::middleware::Next,
) -> impl IntoResponse {
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    insert_common_security_headers(h);
    h.insert("content-security-policy", HeaderValue::from_static("default-src 'none'"));
    h.insert("cache-control", HeaderValue::from_static("no-store, no-cache, must-revalidate"));
    response
}

/// SHA-256 hash (base64 standard) of `STATUS_PAGE_CSS` in status_server.rs.
/// Used in the Content-Security-Policy style-src directive.
/// A unit test in status_server.rs verifies this value matches the actual CSS bytes.
pub(crate) const CSP_CSS_HASH: &str = "EzDjh6fNSkXy+4Mk3I+Q6Pr4QSMDO7qOBhX3NHRvUFw=";

/// SHA-256 hash (base64 standard) of `STATUS_PAGE_SCRIPT` in status_server.rs.
/// Used in the Content-Security-Policy script-src directive.
/// A unit test in status_server.rs verifies this value matches the actual script bytes.
pub(crate) const CSP_SCRIPT_HASH: &str = "wU0Ewa2fRPMqN+gEO+pMBNDvkMABDjHCJ+3YwjpGSGo=";

/// Security headers for the status page server.
/// CSP includes SHA-256 hashes for the inline CSS and JS blocks rendered by
/// `status_page()`.  These hashes must be recomputed whenever those blocks change.
///
/// style-src  hash: covers `STATUS_PAGE_CSS` in status_server.rs
/// script-src hash: covers `STATUS_PAGE_SCRIPT` in status_server.rs
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
            CSP_CSS_HASH, CSP_SCRIPT_HASH,
        )).expect("CSP header value is always valid ASCII"),
    );
    response
}
