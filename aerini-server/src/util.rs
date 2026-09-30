use axum::extract::{ConnectInfo, Request};
use std::net::{IpAddr, SocketAddr};

/// Extracts the real client IP from X-Forwarded-For when behind trusted proxies.
///
/// `trusted_proxy_count` = how many proxy hops sit between the internet and
/// this server. Each trusted proxy appends the address of its own peer, so the
/// client is the Nth entry from the right; anything to its left is
/// client-supplied and never trusted. With count=1 and header
/// "9.9.9.9, 1.2.3.4", returns 1.2.3.4.
/// Falls back to the TCP source IP if: count is 0, header is absent/malformed,
/// or fewer IPs are present than the trust count.
pub(crate) fn extract_client_ip(req: &Request, trusted_proxy_count: usize) -> IpAddr {
    let tcp_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));

    if trusted_proxy_count == 0 {
        return tcp_ip;
    }

    let xff = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let ips: Vec<&str> = xff.split(',').map(|s| s.trim()).collect();
    if ips.len() < trusted_proxy_count {
        return tcp_ip;
    }

    let idx = ips.len() - trusted_proxy_count;
    ips[idx].parse::<IpAddr>().unwrap_or(tcp_ip)
}
