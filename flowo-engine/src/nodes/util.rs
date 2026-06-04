use serde_json::Value;

use crate::error::NodeError;
use crate::model::NodeOutput;

/// Scrubs embedded URL passwords from a driver error string before it is
/// stored or returned to callers.
///
/// Replaces `scheme://user:password@host` with `scheme://user:[REDACTED]@host`
/// anywhere in `s`. Safe to call on strings that contain no URL — returned
/// unchanged. Used to prevent DB connection strings (which include credentials)
/// from leaking through sqlx / redis error messages.
pub fn scrub_url_in_error(s: &str) -> String {
    let marker = "://";
    let mut result = String::with_capacity(s.len());
    let mut haystack = s;

    while let Some(pos) = haystack.find(marker) {
        result.push_str(&haystack[..pos + marker.len()]);
        let after = &haystack[pos + marker.len()..];

        let at_pos    = after.find('@');
        let slash_pos = after.find('/');
        let space_pos = after.find(|c: char| c.is_ascii_whitespace());

        // '@' must appear before any '/' or whitespace to be part of credentials.
        let at_before_delim = match at_pos {
            None    => false,
            Some(a) => slash_pos.is_none_or(|s| a < s) && space_pos.is_none_or(|s| a < s),
        };

        if at_before_delim {
            let a = at_pos.unwrap();
            let before_at = &after[..a];
            if let Some(colon) = before_at.find(':') {
                let user = &before_at[..colon];
                if !user.is_empty() && !user.contains(|c: char| c.is_ascii_whitespace()) {
                    result.push_str(user);
                    result.push_str(":[REDACTED]@");
                    haystack = &after[a + 1..];
                    continue;
                }
            }
        }
        haystack = after;
    }

    result.push_str(haystack);
    result
}

#[cfg(test)]
mod scrub_tests {
    use super::scrub_url_in_error;

    #[test]
    fn redacts_postgres_password_in_error() {
        let e = "Postgres connection failed: error connecting to server: postgres://admin:s3cr3t@db.internal:5432/prod — connection refused";
        let out = scrub_url_in_error(e);
        assert!(!out.contains("s3cr3t"), "password must be removed");
        assert!(out.contains("[REDACTED]"), "must have redaction marker");
        assert!(out.contains("admin"), "username must stay");
    }

    #[test]
    fn string_without_url_unchanged() {
        let e = "connection timed out after 30s";
        assert_eq!(scrub_url_in_error(e), e);
    }

    #[test]
    fn url_without_credentials_unchanged() {
        let e = "failed: redis://cache.internal:6379";
        let out = scrub_url_in_error(e);
        assert_eq!(out, e);
    }
}

/// Traverse a dot-separated field path into a JSON Value.
///
/// Returns `Value::Null` on any miss: key absent, non-object intermediate value,
/// or empty path segments. Does not support array index traversal — object keys only.
///
/// Example: `traverse_dotpath(&val, "body.user.name")` is equivalent to
/// `val["body"]["user"]["name"]` but returns Null instead of panicking on miss.
pub fn traverse_dotpath(data: &Value, path: &str) -> Value {
    let mut current = data;
    for part in path.split('.') {
        match current.get(part) {
            Some(v) => current = v,
            None => return Value::Null,
        }
    }
    current.clone()
}

/// Validate a resolved IP against the SSRF block list.
///
/// Rejects loopback, private, link-local, broadcast, documentation,
/// unspecified, multicast, and IPv4-mapped IPv6 addresses, plus the
/// Azure IMDS endpoint (168.63.129.16) which is not RFC 1918 but must
/// be explicitly blocked.
///
/// Called for both IP-literal URLs and post-DNS domain resolution.
pub fn check_ssrf_ip(ip: std::net::IpAddr) -> Result<(), String> {
    use std::net::{IpAddr, Ipv4Addr};
    let azure_imds = Ipv4Addr::new(168, 63, 129, 16);
    match ip {
        IpAddr::V4(v4) => {
            if v4 == azure_imds
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
            {
                return Err(format!(
                    "Requests to private/internal IP addresses are not permitted ({})", v4
                ));
            }
        }
        IpAddr::V6(v6) => {
            // IPv4-mapped IPv6 (::ffff:x.x.x.x) must be checked as IPv4.
            if let Some(v4) = v6.to_ipv4_mapped() {
                if v4 == azure_imds
                    || v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_documentation()
                    || v4.is_unspecified()
                    || v4.is_multicast()
                {
                    return Err(format!(
                        "Requests to private/internal IP addresses are not permitted ({})", v6
                    ));
                }
            }
            if v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
            {
                return Err(format!(
                    "Requests to private/internal IPv6 addresses are not permitted ({})", v6
                ));
            }
        }
    }
    Ok(())
}

/// SSRF protection for a pre-parsed host and port.
///
/// For IP literals: validates directly via `check_ssrf_ip`.
/// For domain names: checks the static block list then resolves via DNS and
/// validates every returned address.
///
/// **TOCTOU gap:** a TOCTOU (time-of-check / time-of-use) window exists between
/// this DNS pre-check and the actual TCP connect. A malicious DNS server can
/// return a public IP during validation and a private IP on the actual connection
/// (DNS rebinding). This is unavoidable at the application layer — this check is
/// defence-in-depth only.
///
/// **Required mitigation:** configure a network-level egress firewall to block
/// outbound TCP connections to private IP ranges (RFC 1918, link-local, loopback).
/// The application-layer SSRF check alone does not provide a complete boundary.
pub async fn check_host_ssrf(host: url::Host<&str>, port: u16) -> Result<(), String> {
    match host {
        url::Host::Ipv4(ip) => check_ssrf_ip(std::net::IpAddr::V4(ip)),

        url::Host::Ipv6(ip) => check_ssrf_ip(std::net::IpAddr::V6(ip)),

        url::Host::Domain(domain) => {
            let lower = domain.to_lowercase();
            if lower == "localhost"
                || lower.ends_with(".localhost")
                || lower == "metadata.google.internal"
            {
                return Err(format!("Requests to '{}' are not permitted", domain));
            }

            let addrs = tokio::net::lookup_host(format!("{}:{}", domain, port))
                .await
                .map_err(|e| format!("DNS resolution failed for '{}': {}", domain, e))?;
            let mut resolved_any = false;
            for addr in addrs {
                resolved_any = true;
                check_ssrf_ip(addr.ip())?;
            }
            if !resolved_any {
                return Err(format!("DNS resolution returned no addresses for '{}'", domain));
            }
            Ok(())
        }
    }
}

/// SSRF protection for AI node `base_url` fields and any other HTTP/HTTPS URL.
///
/// Parses `raw_url`, validates the scheme is `http` or `https`, extracts the
/// host and port, then delegates to `check_host_ssrf`. Blocks loopback, private,
/// link-local, Azure IMDS, and all other non-public IP ranges.
///
/// **Note:** this check intentionally blocks `localhost` and loopback addresses,
/// which means self-hosted inference servers (e.g. Ollama at
/// `http://localhost:11434/v1`) will be rejected in server/API mode. This is
/// the correct security boundary for multi-user deployments — the local network
/// is not trusted from the server's perspective. Desktop-only users running
/// Ollama must use a non-loopback address reachable from outside (e.g. bind
/// Ollama to `0.0.0.0` and use the machine's LAN IP) or disable SSRF checking
/// via a dedicated flag if one is added in a future release.
pub async fn check_host_ssrf_from_url(raw_url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(raw_url)
        .map_err(|e| format!("Invalid base_url: {}", e))?;

    match parsed.scheme() {
        "http" | "https" => {}
        s => return Err(format!("URL scheme '{}' is not permitted. Use http or https.", s)),
    }

    let host = match parsed.host() {
        Some(h) => h,
        None => return Err("base_url has no host".to_string()),
    };

    let port = parsed.port_or_known_default().unwrap_or(443);
    check_host_ssrf(host, port).await
}

/// SSRF protection for database connection URLs (postgres, mysql, redis).
///
/// Parses `raw_url`, extracts the host and port, and delegates to
/// `check_host_ssrf`. Port fallbacks: postgres → 5432, mysql → 3306,
/// redis/rediss → 6379. The url crate does not know these as special
/// schemes and returns `None` from `port()` when no port is specified.
pub async fn check_db_url_ssrf(raw_url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(raw_url)
        .map_err(|e| format!("Invalid connection URL: {}", e))?;

    let host = match parsed.host() {
        Some(h) => h,
        None => return Err("Connection URL has no host".to_string()),
    };

    let port = parsed.port().unwrap_or_else(|| match parsed.scheme() {
        "postgres" | "postgresql" => 5432,
        "mysql"                   => 3306,
        "redis" | "rediss"        => 6379,
        _                         => 0,
    });

    check_host_ssrf(host, port).await
}

/// Maps a reqwest network error to a `NodeOutput`, classifying timeout and
/// connection errors as recoverable (eligible for scheduler retry).
/// All other errors are unrecoverable.
pub fn http_err_output(e: &reqwest::Error) -> NodeOutput {
    if e.is_timeout() || e.is_connect() {
        NodeOutput::failure(NodeError::recoverable("HTTP_ERROR", e.to_string()))
    } else {
        NodeOutput::failure(NodeError::unrecoverable("HTTP_ERROR", e.to_string()))
    }
}

#[cfg(test)]
mod ssrf_tests {
    use super::check_ssrf_ip;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr { IpAddr::V4(Ipv4Addr::new(a, b, c, d)) }
    fn v6(s: &str) -> IpAddr { IpAddr::V6(s.parse::<Ipv6Addr>().unwrap()) }

    #[test]
    fn loopback_blocked()          { assert!(check_ssrf_ip(v4(127, 0, 0, 1)).is_err()); }
    #[test]
    fn rfc1918_10_blocked()        { assert!(check_ssrf_ip(v4(10, 0, 0, 1)).is_err()); }
    #[test]
    fn rfc1918_172_blocked()       { assert!(check_ssrf_ip(v4(172, 16, 0, 1)).is_err()); }
    #[test]
    fn rfc1918_192_blocked()       { assert!(check_ssrf_ip(v4(192, 168, 1, 1)).is_err()); }
    #[test]
    fn link_local_169_blocked()    { assert!(check_ssrf_ip(v4(169, 254, 0, 1)).is_err()); }
    #[test]
    fn azure_imds_blocked()        { assert!(check_ssrf_ip(v4(168, 63, 129, 16)).is_err()); }
    #[test]
    fn public_ip_allowed()         { assert!(check_ssrf_ip(v4(8, 8, 8, 8)).is_ok()); }
    #[test]
    fn another_public_allowed()    { assert!(check_ssrf_ip(v4(93, 184, 216, 34)).is_ok()); }
    #[test]
    fn ipv6_loopback_blocked()     { assert!(check_ssrf_ip(v6("::1")).is_err()); }
    #[test]
    fn ipv6_unique_local_blocked() { assert!(check_ssrf_ip(v6("fc00::1")).is_err()); }
    #[test]
    fn ipv6_link_local_blocked()   { assert!(check_ssrf_ip(v6("fe80::1")).is_err()); }
    #[test]
    fn ipv4_mapped_private_blocked() {
        // ::ffff:10.0.0.1 — IPv4-mapped IPv6, must be caught as private
        assert!(check_ssrf_ip(v6("::ffff:10.0.0.1")).is_err());
    }
}
