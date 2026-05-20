use serde_json::Value;

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
/// validates every returned address. Narrows the DNS-rebinding attack window
/// to the gap between this lookup and the actual TCP connect.
///
/// `port` is used only as the port argument to `lookup_host` and does not
/// affect the IP validation logic.
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

