use axum::http::HeaderValue;
use axum::response::IntoResponse;
use dashmap::DashMap;
use std::net::IpAddr;
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

    /// Returns `true` if the request is within the rate limit; `false` if it
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
    response
}

/// Security headers for the status page server.
/// CSP includes SHA-256 hashes for the inline CSS and JS blocks rendered by
/// `status_page()`.  These hashes must be recomputed whenever those blocks change.
///
/// style-src  hash: covers `STATUS_PAGE_CSS` in status_server.rs
/// script-src hash: covers the `_flowoRun` script block in status_server.rs
pub async fn status_security_headers(
    req:  axum::extract::Request,
    next: axum::middleware::Next,
) -> impl IntoResponse {
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    insert_common_security_headers(h);
    h.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'none'; \
             style-src 'sha256-EzDjh6fNSkXy+4Mk3I+Q6Pr4QSMDO7qOBhX3NHRvUFw='; \
             script-src 'sha256-wU0Ewa2fRPMqN+gEO+pMBNDvkMABDjHCJ+3YwjpGSGo='; \
             frame-ancestors 'none'",
        ),
    );
    response
}
