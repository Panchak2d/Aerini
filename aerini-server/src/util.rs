use axum::extract::{ConnectInfo, Request};
use std::net::{IpAddr, SocketAddr};

/// Extracts the credential from an `Authorization: Bearer <token>` value.
/// The scheme name is case-insensitive and may be followed by more than one
/// space (RFC 7235 §2.1); an empty credential is treated as absent.
pub(crate) fn parse_bearer(value: &str) -> Option<&str> {
    let (scheme, rest) = value.split_once(' ')?;
    let credential = rest.trim_start_matches(' ');
    (scheme.eq_ignore_ascii_case("bearer") && !credential.is_empty()).then_some(credential)
}

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

    // Repeated field lines are equivalent to one comma-joined line
    // (RFC 9110 §5.3), and a proxy may append its own line instead of
    // extending the client's, so every line has to be read, in order.
    let ips: Vec<&str> = req
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .collect();
    if ips.len() < trusted_proxy_count {
        return tcp_ip;
    }

    let idx = ips.len() - trusted_proxy_count;
    ips[idx].parse::<IpAddr>().unwrap_or(tcp_ip)
}
