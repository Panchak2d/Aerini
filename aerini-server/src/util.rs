use axum::extract::{ConnectInfo, Request};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::pin;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tower::Service;

const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Serves `app` over HTTP/1.1 and drops any connection that has not delivered a
/// complete request head within `HEADER_READ_TIMEOUT`. `axum::serve` sets no hyper
/// timer, so hyper's default 30 s header timeout never arms there and a client
/// that trickles bytes can hold a connection open forever. Once `shutdown`
/// resolves, no new connections are accepted and open ones finish gracefully.
/// Dropping the returned future aborts every connection still open.
pub(crate) async fn serve_http<F>(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    shutdown: F,
) -> std::io::Result<()>
where
    F: Future<Output = ()>,
{
    let mut connections: JoinSet<()> = JoinSet::new();
    let stop = CancellationToken::new();
    let mut shutdown = pin!(shutdown);

    loop {
        let (stream, remote_addr) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(pair) => pair,
                Err(e) => {
                    if !matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::ConnectionReset
                    ) {
                        tracing::warn!("accept failed: {e}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    continue;
                }
            },
            _ = &mut shutdown => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => continue,
        };

        let app = app.clone();
        let stop = stop.clone();
        connections.spawn(async move {
            let service = service_fn(move |mut req: Request<Incoming>| {
                req.extensions_mut().insert(ConnectInfo(remote_addr));
                let mut app = app.clone();
                async move { app.call(req).await }
            });
            let mut conn = pin!(http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_READ_TIMEOUT)
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades());
            tokio::select! {
                _ = conn.as_mut() => return,
                _ = stop.cancelled() => conn.as_mut().graceful_shutdown(),
            }
            let _ = conn.await;
        });
    }

    stop.cancel();
    while connections.join_next().await.is_some() {}
    Ok(())
}

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

/// `host:port` for display and logs, with an IPv6 literal in brackets.
pub(crate) fn join_host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
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
    use super::{extract_client_ip, forwarded_client_ip, join_host_port, parse_bearer};
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

    #[test]
    fn ipv6_literal_is_bracketed_and_other_hosts_are_not() {
        assert_eq!(join_host_port("::1", 8080), "[::1]:8080");
        assert_eq!(join_host_port("[::1]", 8080), "[::1]:8080");
        assert_eq!(join_host_port("127.0.0.1", 8080), "127.0.0.1:8080");
        assert_eq!(join_host_port("localhost", 80), "localhost:80");
    }
}
