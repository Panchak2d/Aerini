use axum::extract::{ConnectInfo, Request};
use std::net::{IpAddr, SocketAddr};

/// Extracts the real client IP from X-Forwarded-For when behind trusted proxies.
///
/// `trusted_proxy_count` = how many proxy hops sit between the internet and
/// this server. Each trusted proxy appends the address of its own peer, so the
/// client is the Nth entry from the right; anything to its left is
/// client-supplied and never trusted. With count=1 and header
/// "9.9.9.9, 1.2.3.4", returns 1.2.3.4.
///
/// Every `X-Forwarded-For` header line is read, in order, as one comma-joined
/// list (RFC 9110 §5.3): a proxy may append its own line instead of extending
/// an existing one, and counting from the right must see that line.
/// Falls back to the TCP source IP if: count is 0, the header is absent or
/// any line is not valid text, the selected entry is not an IP address, or
/// fewer entries are present than the trust count.
pub(crate) fn extract_client_ip(req: &Request, trusted_proxy_count: usize) -> IpAddr {
    let tcp_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));

    if trusted_proxy_count == 0 {
        return tcp_ip;
    }

    // A line that cannot be read must not be skipped: dropping it would shift
    // every entry's distance from the right.
    let mut lines = Vec::new();
    for value in req.headers().get_all("x-forwarded-for") {
        match value.to_str() {
            Ok(line) => lines.push(line),
            Err(_) => return tcp_ip,
        }
    }

    forwarded_client_ip(&lines.join(","), trusted_proxy_count).unwrap_or(tcp_ip)
}

fn forwarded_client_ip(xff: &str, trusted_proxy_count: usize) -> Option<IpAddr> {
    if trusted_proxy_count == 0 {
        return None;
    }
    let ips: Vec<&str> = xff.split(',').map(str::trim).collect();
    let idx = ips.len().checked_sub(trusted_proxy_count)?;
    ips[idx].parse::<IpAddr>().ok()
}

/// Extracts the credential from an `Authorization: Bearer <token>` value.
/// The scheme name is case-insensitive and one or more spaces may precede the
/// token (RFC 7235 §2.1, §5.1.2); a value with no token after the scheme is
/// treated as missing.
pub(crate) fn parse_bearer(value: &str) -> Option<&str> {
    let (scheme, rest) = value.split_once(' ')?;
    let token = rest.trim_start_matches(' ');
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::{extract_client_ip, forwarded_client_ip, parse_bearer};
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::HeaderValue;
    use std::net::{IpAddr, Ipv4Addr};

    const TCP_FALLBACK: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

    fn req_with_xff_lines(lines: &[&str]) -> Request<Body> {
        let mut b = Request::builder();
        for l in lines {
            b = b.header("x-forwarded-for", *l);
        }
        b.body(Body::empty()).unwrap()
    }

    #[test]
    fn xff_lines_are_counted_from_the_right_across_all_lines() {
        let req = req_with_xff_lines(&["9.9.9.9", "1.2.3.4"]);
        assert_eq!(extract_client_ip(&req, 1), "1.2.3.4".parse::<IpAddr>().unwrap());
        assert_eq!(extract_client_ip(&req, 2), "9.9.9.9".parse::<IpAddr>().unwrap());
        let req = req_with_xff_lines(&["7.7.7.7, 9.9.9.9", "1.2.3.4"]);
        assert_eq!(extract_client_ip(&req, 2), "9.9.9.9".parse::<IpAddr>().unwrap());
        assert_eq!(extract_client_ip(&req, 4), TCP_FALLBACK);
    }

    #[test]
    fn unreadable_xff_line_falls_back_to_tcp_ip() {
        let mut req = req_with_xff_lines(&["1.2.3.4"]);
        req.headers_mut().append(
            "x-forwarded-for",
            HeaderValue::from_bytes(&[0xff]).unwrap(),
        );
        assert_eq!(extract_client_ip(&req, 1), TCP_FALLBACK);
    }

    #[test]
    fn selected_xff_entry_must_be_an_ip() {
        assert_eq!(forwarded_client_ip("1.2.3.4,", 1), None);
        assert_eq!(forwarded_client_ip("", 1), None);
        assert_eq!(forwarded_client_ip("a, 1.2.3.4", 1), Some("1.2.3.4".parse().unwrap()));
        assert_eq!(forwarded_client_ip("a, 1.2.3.4", 2), None);
    }

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        assert_eq!(parse_bearer("Bearer abc"), Some("abc"));
        assert_eq!(parse_bearer("bearer abc"), Some("abc"));
        assert_eq!(parse_bearer("BEARER abc"), Some("abc"));
    }

    #[test]
    fn bearer_accepts_extra_spaces_and_rejects_missing_token() {
        assert_eq!(parse_bearer("Bearer   abc"), Some("abc"));
        assert_eq!(parse_bearer("Bearer"), None);
        assert_eq!(parse_bearer("Bearer "), None);
        assert_eq!(parse_bearer("Bearer    "), None);
        assert_eq!(parse_bearer("Basic abc"), None);
    }
}
