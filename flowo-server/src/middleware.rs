use axum::http::HeaderValue;
use axum::response::IntoResponse;
use dashmap::DashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;

/// Per-IP sliding-window rate limiter backed by a `DashMap`.
///
/// `max_requests` in `window_secs` seconds per client address.
/// Evicts stale entries automatically when the map exceeds 10 000 entries.
pub struct RateLimiter {
    map:          DashMap<IpAddr, (u32, Instant)>,
    max_requests: u32,
    window_secs:  u64,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self { map: DashMap::new(), max_requests, window_secs }
    }

    /// Spawns a Tokio background task that evicts stale entries every
    /// `window_secs` seconds.  Call once after creating the limiter.
    /// Without this, stale entries only evict when the map exceeds 10 000
    /// entries — on low-traffic deployments they never evict, leaking memory
    /// under sustained IP churn (e.g. a botnet scan).
    pub fn spawn_eviction_task(self: Arc<Self>) {
        let weak = Arc::downgrade(&self);
        let interval_secs = self.window_secs;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(
                std::time::Duration::from_secs(interval_secs),
            );
            ticker.tick().await; // skip the immediate first tick
            loop {
                ticker.tick().await;
                match weak.upgrade() {
                    None => break, // RateLimiter dropped — stop the task
                    Some(rl) => {
                        let cutoff = Instant::now()
                            - std::time::Duration::from_secs(rl.window_secs * 2);
                        rl.map.retain(|_, v| v.1 > cutoff);
                    }
                }
            }
        });
    }
    /// should be rejected.  Eviction of stale entries runs inline when the map
    /// grows large, preventing unbounded growth under IP rotation or DoS.
    pub fn is_allowed(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let allowed = {
            let mut entry = self.map.entry(ip).or_insert((0u32, now));
            if now.duration_since(entry.1).as_secs() >= self.window_secs {
                *entry = (1, now);
                true
            } else if entry.0 < self.max_requests {
                entry.0 += 1;
                true
            } else {
                false
            }
        };
        if self.map.len() > 10_000 {
            let cutoff = now - std::time::Duration::from_secs(self.window_secs * 2);
            self.map.retain(|_, v| v.1 > cutoff);
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
