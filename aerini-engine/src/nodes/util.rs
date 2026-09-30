use serde_json::Value;

use crate::error::NodeError;
use crate::model::{ExecutionContext, NodeOutput};

/// Runs a closure when dropped unless disarmed first. Releases external
/// resources (child process groups, running DB statements) when a node future
/// is dropped mid-flight by cancellation or a workflow-level timeout.
pub(crate) struct OnDrop(Option<Box<dyn FnOnce() + Send>>);

impl OnDrop {
    pub(crate) fn new(f: impl FnOnce() + Send + 'static) -> Self {
        OnDrop(Some(Box::new(f)))
    }

    pub(crate) fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for OnDrop {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

/// Guard that SIGKILLs the whole process group led by `pid` when dropped.
/// The child must have been spawned with `process_group(0)`. Does nothing
/// on non-Unix targets or when `pid` is `None`.
#[cfg(unix)]
pub(crate) fn kill_group_on_drop(pid: Option<u32>) -> OnDrop {
    match pid.and_then(|p| i32::try_from(p).ok()).filter(|p| *p > 0) {
        Some(pgid) => OnDrop::new(move || {
            // SAFETY: kill(2) takes plain integers and has no memory-safety preconditions.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }),
        None => OnDrop(None),
    }
}

#[cfg(not(unix))]
pub(crate) fn kill_group_on_drop(_pid: Option<u32>) -> OnDrop {
    OnDrop(None)
}

#[cfg(test)]
mod on_drop_tests {
    use super::OnDrop;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn on_drop_runs_action_when_dropped() {
        let fired = Arc::new(AtomicBool::new(false));
        let flag = fired.clone();
        drop(OnDrop::new(move || flag.store(true, Ordering::SeqCst)));
        assert!(fired.load(Ordering::SeqCst));
    }

    #[test]
    fn on_drop_skips_action_when_disarmed() {
        let fired = Arc::new(AtomicBool::new(false));
        let flag = fired.clone();
        OnDrop::new(move || flag.store(true, Ordering::SeqCst)).disarm();
        assert!(!fired.load(Ordering::SeqCst));
    }
}

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
            let a = at_pos.expect("at_pos is Some: at_before_delim guard requires at_pos.is_some()");
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

/// Returns every `(node_id, output)` pair from `context.node_outputs` in a
/// deterministic order, instead of the raw `HashMap`'s unspecified (and
/// empirically randomized, per `RandomState`) iteration order.
///
/// Callers must use this instead of iterating `context.node_outputs`
/// directly: `output_node.rs`, `json_node.rs`, `text_splitter.rs`,
/// `merge.rs`, and `loop_node.rs` all rely on it so that "most recent",
/// "first found", and "array order" semantics stay identical between runs
/// of the same workflow, rather than depending on the raw `HashMap`'s
/// unspecified (and empirically randomized, per `RandomState`) iteration
/// order.
///
/// Ordering: nodes appear in `context.execution_order` order (see
/// `ExecutionState::mark_succeeded` in `context.rs` — completion order, with
/// a node that completes more than once, e.g. a loop body node re-run each
/// iteration, moved to the position of its *most recent* completion rather
/// than duplicated). Any `node_outputs` entry with no corresponding
/// `execution_order` entry (a context built directly — e.g. by a test, or by
/// any future caller that populates `node_outputs` without going through the
/// executor) is appended afterward, sorted by node id, so the result is
/// always fully deterministic and never silently drops an entry.
pub fn ordered_node_outputs(context: &ExecutionContext) -> Vec<(String, Value)> {
    let mut seen = std::collections::HashSet::with_capacity(context.node_outputs.len());
    let mut out = Vec::with_capacity(context.node_outputs.len());
    for id in context.execution_order.iter() {
        if let Some(v) = context.node_outputs.get(id) {
            out.push((id.clone(), v.clone()));
            seen.insert(id.clone());
        }
    }
    if out.len() < context.node_outputs.len() {
        let mut rest: Vec<(&String, &Value)> = context.node_outputs
            .iter()
            .filter(|(k, _)| !seen.contains(*k))
            .collect();
        rest.sort_by(|a, b| a.0.cmp(b.0));
        out.extend(rest.into_iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    out
}

#[cfg(test)]
mod ordered_node_outputs_tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use serde_json::json;

    fn ctx(node_outputs: HashMap<String, Value>, execution_order: Vec<&str>) -> ExecutionContext {
        ExecutionContext {
            variables: HashMap::new(),
            node_outputs: Arc::new(node_outputs),
            metadata: HashMap::new(),
            execution_order: Arc::new(execution_order.into_iter().map(String::from).collect()),
        }
    }

    #[test]
    fn follows_execution_order_when_present() {
        let mut outputs = HashMap::new();
        outputs.insert("b".to_string(), json!(2));
        outputs.insert("a".to_string(), json!(1));
        outputs.insert("c".to_string(), json!(3));
        let result = ordered_node_outputs(&ctx(outputs, vec!["c", "a", "b"]));
        let ids: Vec<&str> = result.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["c", "a", "b"]);
    }

    #[test]
    fn falls_back_to_sorted_keys_when_execution_order_empty() {
        // Matches every pre-existing test fixture across the five consumer
        // node files, which construct ExecutionContext directly and leave
        // execution_order at its ..Default::default() value (empty).
        let mut outputs = HashMap::new();
        outputs.insert("zebra".to_string(), json!(1));
        outputs.insert("apple".to_string(), json!(2));
        let result = ordered_node_outputs(&ctx(outputs, vec![]));
        let ids: Vec<&str> = result.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["apple", "zebra"]);
    }

    #[test]
    fn entries_missing_from_execution_order_are_appended_sorted() {
        let mut outputs = HashMap::new();
        outputs.insert("known".to_string(), json!(1));
        outputs.insert("z_unknown".to_string(), json!(2));
        outputs.insert("a_unknown".to_string(), json!(3));
        let result = ordered_node_outputs(&ctx(outputs, vec!["known"]));
        let ids: Vec<&str> = result.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["known", "a_unknown", "z_unknown"]);
    }

    #[test]
    fn stale_execution_order_entry_with_no_matching_output_is_skipped_not_panicking() {
        let mut outputs = HashMap::new();
        outputs.insert("a".to_string(), json!(1));
        let result = ordered_node_outputs(&ctx(outputs, vec!["ghost", "a"]));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "a");
    }

    #[test]
    fn empty_context_returns_empty_vec() {
        let result = ordered_node_outputs(&ctx(HashMap::new(), vec![]));
        assert!(result.is_empty());
    }
}

/// Policy for SSRF host/IP validation.
///
/// `Strict` blocks loopback, RFC 1918 private, and IPv6 unique-local ranges.
/// `AllowLocal` additionally permits those ranges -- only appropriate for a
/// node whose entire purpose is reaching a server on the user's own machine
/// or LAN, where the configured address is itself the user's explicit trust
/// signal (e.g. a1111/comfyui in `image_gen.rs`). Do not use `AllowLocal` for
/// general-purpose URL fields where the target could come from an untrusted
/// source the caller doesn't control.
///
/// Azure IMDS, link-local (including the 169.254.169.254 cloud metadata
/// address), RFC 6598 shared address space, broadcast, documentation,
/// unspecified, and multicast all remain blocked unconditionally under either
/// policy -- none of those are legitimate targets for a self-hosted local
/// server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsrfPolicy {
    Strict,
    AllowLocal,
}

impl SsrfPolicy {
    fn allow_local(self) -> bool {
        matches!(self, SsrfPolicy::AllowLocal)
    }
}

/// Validate a resolved IP against the SSRF block list per `policy`.
///
/// Rejects (always, both policies): link-local, broadcast, documentation,
/// unspecified, multicast, RFC 6598 shared address space (100.64.0.0/10),
/// IPv4-mapped IPv6 addresses, and the Azure IMDS endpoint (168.63.129.16).
/// Under `Strict`, also rejects loopback and RFC 1918 private addresses;
/// `AllowLocal` permits those.
///
/// Called for both IP-literal URLs and post-DNS domain resolution.
pub fn check_ssrf_ip(ip: std::net::IpAddr, policy: SsrfPolicy) -> Result<(), String> {
    check_ssrf_ip_impl(ip, policy.allow_local())
}

fn check_ssrf_ip_impl(ip: std::net::IpAddr, allow_local: bool) -> Result<(), String> {
    use std::net::{IpAddr, Ipv4Addr};
    let azure_imds = Ipv4Addr::new(168, 63, 129, 16);
    match ip {
        IpAddr::V4(v4) => {
            let always_blocked = v4 == azure_imds
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                // RFC 6598 — Shared Address Space (100.64.0.0/10).
                // Used by carrier-grade NAT and some cloud providers for internal
                // routing. Not covered by is_private() (which only checks RFC 1918).
                // Reachable on AWS and similar environments; must be explicitly blocked.
                || u32::from(v4) & 0xFFC0_0000 == 0x6440_0000;
            let local_only_blocked = !allow_local && (v4.is_loopback() || v4.is_private());
            if always_blocked || local_only_blocked {
                return Err(format!(
                    "Requests to private/internal IP addresses are not permitted ({})", v4
                ));
            }
        }
        IpAddr::V6(v6) => {
            // IPv4-mapped IPv6 (::ffff:x.x.x.x) must be checked as IPv4.
            if let Some(v4) = v6.to_ipv4_mapped() {
                let always_blocked = v4 == azure_imds
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_documentation()
                    || v4.is_unspecified()
                    || v4.is_multicast()
                    || u32::from(v4) & 0xFFC0_0000 == 0x6440_0000;
                let local_only_blocked = !allow_local && (v4.is_loopback() || v4.is_private());
                if always_blocked || local_only_blocked {
                    return Err(format!(
                        "Requests to private/internal IP addresses are not permitted ({})", v6
                    ));
                }
            }
            let always_blocked = v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unicast_link_local();
            let local_only_blocked = !allow_local && (v6.is_loopback() || v6.is_unique_local());
            if always_blocked || local_only_blocked {
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
pub async fn check_host_ssrf(host: url::Host<&str>, port: u16, policy: SsrfPolicy) -> Result<(), String> {
    check_host_ssrf_impl(host, port, policy.allow_local()).await
}

async fn check_host_ssrf_impl(
    host: url::Host<&str>,
    port: u16,
    allow_local: bool,
) -> Result<(), String> {
    match host {
        url::Host::Ipv4(ip) => {
            let addr = std::net::IpAddr::V4(ip);
            check_ssrf_ip_impl(addr, allow_local)
        }

        url::Host::Ipv6(ip) => {
            let addr = std::net::IpAddr::V6(ip);
            check_ssrf_ip_impl(addr, allow_local)
        }

        url::Host::Domain(domain) => {
            let lower = domain.to_lowercase();
            let is_localhost = lower == "localhost" || lower.ends_with(".localhost");
            if (is_localhost && !allow_local) || lower == "metadata.google.internal" {
                return Err(format!("Requests to '{}' are not permitted", domain));
            }

            let addrs = tokio::net::lookup_host(format!("{}:{}", domain, port))
                .await
                .map_err(|e| format!("DNS resolution failed for '{}': {}", domain, e))?;
            let mut resolved_any = false;
            for addr in addrs {
                resolved_any = true;
                check_ssrf_ip_impl(addr.ip(), allow_local)?;
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
/// **Note:** this check intentionally blocks `localhost` and loopback addresses
/// under `SsrfPolicy::Strict`, which means self-hosted inference servers (e.g.
/// Ollama at `http://localhost:11434/v1`) will be rejected. This is the
/// correct security boundary for any node whose target URL could come from an
/// untrusted source the caller doesn't fully control. For a node whose only
/// purpose is reaching a local server the user explicitly configures (e.g.
/// a1111/comfyui in `image_gen.rs`), pass `SsrfPolicy::AllowLocal` instead.
pub async fn check_host_ssrf_from_url(raw_url: &str, policy: SsrfPolicy) -> Result<(), String> {
    check_host_ssrf_from_url_impl(raw_url, policy.allow_local()).await
}

async fn check_host_ssrf_from_url_impl(raw_url: &str, allow_local: bool) -> Result<(), String> {
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
    check_host_ssrf_impl(host, port, allow_local).await
}

/// SSRF protection for database connection URLs (postgres, mysql, redis).
///
/// Parses `raw_url`, extracts the host and port, and delegates to
/// `check_host_ssrf` under the given `policy`. Port fallbacks: postgres →
/// 5432, mysql → 3306, redis/rediss → 6379. The url crate does not know these
/// as special schemes and returns `None` from `port()` when no port is
/// specified.
///
/// Database connections can legitimately target either a remote host
/// (`SsrfPolicy::Strict`, the default every existing caller keeps getting)
/// or a self-hosted local/LAN
/// instance the caller explicitly configured (`SsrfPolicy::AllowLocal`,
/// opt-in only). Unlike `ai_prompt`/`ai_agent`/`image_gen::a1111`/`comfyui`
/// (nodes whose *entire* purpose is reaching a local server, so those pass
/// `AllowLocal` unconditionally), `connection_url` here is general-purpose —
/// it targets cloud-hosted databases far more often than local ones — so the
/// policy is per-call, driven by the Database node's own `allow_local` input
/// field (see `database/postgres.rs::execute_sqlx`,
/// `database/redis.rs::execute_redis`), not hardcoded in this function.
pub async fn check_db_url_ssrf(raw_url: &str, policy: SsrfPolicy) -> Result<(), String> {
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

    check_host_ssrf_impl(host, port, policy.allow_local()).await
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

/// Cap applied when buffering an HTTP response body in memory before parsing
/// it as JSON. Mirrors `http.rs`'s own `MAX_RESPONSE_BYTES` (same 10 MB
/// value). Kept as an independent constant rather than importing http.rs's
/// private one: this helper is called from ai_prompt/ai_agent/image_gen,
/// none of which otherwise depend on the http node.
pub const MAX_JSON_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

/// Reads an HTTP response body and parses it as JSON, hard-capping memory
/// use at `MAX_JSON_RESPONSE_BYTES` regardless of what (or whether)
/// `Content-Length` declares.
///
/// Same two-stage guard as `http.rs`'s own response reader: (1) reject
/// upfront if a declared `Content-Length` already exceeds the cap, avoiding
/// a pre-sized allocation for a body we're not going to read; (2) stream the
/// body chunk-by-chunk regardless, hard-capping at the same limit, so a
/// missing/lying `Content-Length` can't bypass the guard.
///
/// Returns a plain `String` error rather than `NodeError` or `NodeOutput` so
/// each call site — some build a `NodeError::unrecoverable`, some a
/// `NodeError::recoverable`, some a bare `String` — can wrap it in whatever
/// error shape that call site already uses, without this helper picking one.
pub async fn read_json_response_capped(mut response: reqwest::Response) -> Result<Value, String> {
    if let Some(cl) = response.content_length() {
        if cl > MAX_JSON_RESPONSE_BYTES as u64 {
            return Err(format!(
                "Response Content-Length {} exceeds {} MB limit",
                cl,
                MAX_JSON_RESPONSE_BYTES / (1024 * 1024)
            ));
        }
    }

    let capacity = response
        .content_length()
        .unwrap_or(0)
        .min(MAX_JSON_RESPONSE_BYTES as u64) as usize;
    let mut body_buf = Vec::with_capacity(capacity);

    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body_buf.len() + chunk.len() > MAX_JSON_RESPONSE_BYTES {
                    return Err(format!(
                        "Response body exceeds {} MB limit",
                        MAX_JSON_RESPONSE_BYTES / (1024 * 1024)
                    ));
                }
                body_buf.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => return Err(format!("Failed to read response body: {}", e)),
        }
    }

    serde_json::from_slice::<Value>(&body_buf)
        .map_err(|e| format!("Failed to parse response as JSON: {}", e))
}

#[cfg(test)]
mod ssrf_tests {
    use super::{check_ssrf_ip, SsrfPolicy};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr { IpAddr::V4(Ipv4Addr::new(a, b, c, d)) }
    fn v6(s: &str) -> IpAddr { IpAddr::V6(s.parse::<Ipv6Addr>().unwrap()) }

    #[test]
    fn loopback_blocked()          { assert!(check_ssrf_ip(v4(127, 0, 0, 1), SsrfPolicy::Strict).is_err()); }
    #[test]
    fn rfc1918_10_blocked()        { assert!(check_ssrf_ip(v4(10, 0, 0, 1), SsrfPolicy::Strict).is_err()); }
    #[test]
    fn rfc1918_172_blocked()       { assert!(check_ssrf_ip(v4(172, 16, 0, 1), SsrfPolicy::Strict).is_err()); }
    #[test]
    fn rfc1918_192_blocked()       { assert!(check_ssrf_ip(v4(192, 168, 1, 1), SsrfPolicy::Strict).is_err()); }
    #[test]
    fn link_local_169_blocked()    { assert!(check_ssrf_ip(v4(169, 254, 0, 1), SsrfPolicy::Strict).is_err()); }
    #[test]
    fn azure_imds_blocked()        { assert!(check_ssrf_ip(v4(168, 63, 129, 16), SsrfPolicy::Strict).is_err()); }
    #[test]
    fn public_ip_allowed()         { assert!(check_ssrf_ip(v4(8, 8, 8, 8), SsrfPolicy::Strict).is_ok()); }
    #[test]
    fn another_public_allowed()    { assert!(check_ssrf_ip(v4(93, 184, 216, 34), SsrfPolicy::Strict).is_ok()); }
    #[test]
    fn ipv6_loopback_blocked()     { assert!(check_ssrf_ip(v6("::1"), SsrfPolicy::Strict).is_err()); }
    #[test]
    fn ipv6_unique_local_blocked() { assert!(check_ssrf_ip(v6("fc00::1"), SsrfPolicy::Strict).is_err()); }
    #[test]
    fn ipv6_link_local_blocked()   { assert!(check_ssrf_ip(v6("fe80::1"), SsrfPolicy::Strict).is_err()); }
    #[test]
    fn ipv4_mapped_private_blocked() {
        // ::ffff:10.0.0.1 — IPv4-mapped IPv6, must be caught as private
        assert!(check_ssrf_ip(v6("::ffff:10.0.0.1"), SsrfPolicy::Strict).is_err());
    }
}

#[cfg(test)]
mod ssrf_allow_local_tests {
    use super::{check_ssrf_ip, SsrfPolicy};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr { IpAddr::V4(Ipv4Addr::new(a, b, c, d)) }
    fn v6(s: &str) -> IpAddr { IpAddr::V6(s.parse::<Ipv6Addr>().unwrap()) }

    // Ranges permitted only under SsrfPolicy::AllowLocal; blocked under the default Strict policy.
    #[test]
    fn loopback_now_allowed()       { assert!(check_ssrf_ip(v4(127, 0, 0, 1), SsrfPolicy::AllowLocal).is_ok()); }
    #[test]
    fn rfc1918_10_now_allowed()     { assert!(check_ssrf_ip(v4(10, 0, 0, 1), SsrfPolicy::AllowLocal).is_ok()); }
    #[test]
    fn rfc1918_172_now_allowed()    { assert!(check_ssrf_ip(v4(172, 16, 0, 1), SsrfPolicy::AllowLocal).is_ok()); }
    #[test]
    fn rfc1918_192_now_allowed()    { assert!(check_ssrf_ip(v4(192, 168, 1, 1), SsrfPolicy::AllowLocal).is_ok()); }
    #[test]
    fn ipv6_loopback_now_allowed()  { assert!(check_ssrf_ip(v6("::1"), SsrfPolicy::AllowLocal).is_ok()); }
    #[test]
    fn ipv6_unique_local_now_allowed() { assert!(check_ssrf_ip(v6("fc00::1"), SsrfPolicy::AllowLocal).is_ok()); }
    #[test]
    fn ipv4_mapped_private_now_allowed() {
        assert!(check_ssrf_ip(v6("::ffff:10.0.0.1"), SsrfPolicy::AllowLocal).is_ok());
    }

    // Must remain blocked even for local providers — none of these are a
    // legitimate local-inference-server target.
    #[test]
    fn link_local_169_still_blocked() { assert!(check_ssrf_ip(v4(169, 254, 0, 1), SsrfPolicy::AllowLocal).is_err()); }
    #[test]
    fn cloud_metadata_ip_still_blocked() { assert!(check_ssrf_ip(v4(169, 254, 169, 254), SsrfPolicy::AllowLocal).is_err()); }
    #[test]
    fn azure_imds_still_blocked()     { assert!(check_ssrf_ip(v4(168, 63, 129, 16), SsrfPolicy::AllowLocal).is_err()); }
    #[test]
    fn rfc6598_still_blocked()        { assert!(check_ssrf_ip(v4(100, 64, 0, 0), SsrfPolicy::AllowLocal).is_err()); }
    #[test]
    fn ipv6_link_local_still_blocked() { assert!(check_ssrf_ip(v6("fe80::1"), SsrfPolicy::AllowLocal).is_err()); }
    #[test]
    fn unspecified_v4_still_blocked() { assert!(check_ssrf_ip(v4(0, 0, 0, 0), SsrfPolicy::AllowLocal).is_err()); }
    #[test]
    fn multicast_v4_still_blocked()  { assert!(check_ssrf_ip(v4(224, 0, 0, 1), SsrfPolicy::AllowLocal).is_err()); }

    // Public IPs remain allowed, same as the strict variant.
    #[test]
    fn public_ip_allowed() { assert!(check_ssrf_ip(v4(8, 8, 8, 8), SsrfPolicy::AllowLocal).is_ok()); }
}

#[cfg(test)]
mod rfc6598_tests {
    use super::{check_ssrf_ip, SsrfPolicy};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr { IpAddr::V4(Ipv4Addr::new(a, b, c, d)) }
    fn v6(s: &str) -> IpAddr { IpAddr::V6(s.parse::<Ipv6Addr>().unwrap()) }

    #[test]
    fn rfc6598_low_blocked()  { assert!(check_ssrf_ip(v4(100, 64, 0, 0), SsrfPolicy::Strict).is_err()); }
    #[test]
    fn rfc6598_mid_blocked()  { assert!(check_ssrf_ip(v4(100, 100, 0, 1), SsrfPolicy::Strict).is_err()); }
    #[test]
    fn rfc6598_high_blocked() { assert!(check_ssrf_ip(v4(100, 127, 255, 255), SsrfPolicy::Strict).is_err()); }
    #[test]
    fn rfc6598_boundary_low_allowed() {
        // 100.63.255.255 is just below 100.64.0.0/10 — must not be blocked by RFC 6598 rule
        assert!(check_ssrf_ip(v4(100, 63, 255, 255), SsrfPolicy::Strict).is_ok());
    }
    #[test]
    fn rfc6598_boundary_high_allowed() {
        // 100.128.0.0 is just above 100.64.0.0/10 — must not be blocked by RFC 6598 rule
        assert!(check_ssrf_ip(v4(100, 128, 0, 0), SsrfPolicy::Strict).is_ok());
    }
    #[test]
    fn rfc6598_ipv4_mapped_blocked() {
        // ::ffff:100.64.0.1 — IPv4-mapped form of RFC 6598 address (hex: 6440:0001)
        assert!(check_ssrf_ip(v6("::ffff:6440:0001"), SsrfPolicy::Strict).is_err());
    }
}

#[cfg(test)]
mod read_json_response_capped_tests {
    use super::{read_json_response_capped, MAX_JSON_RESPONSE_BYTES};

    /// Spawns a one-shot raw TCP server that replies with exactly `raw_response`
    /// (the caller supplies the full HTTP status line + headers + body, so tests
    /// can control Content-Length independently of actual body length). Same
    /// idiom as `ai_prompt/openai.rs::backward_compat_tests`'s mock server.
    async fn spawn_raw_mock(raw_response: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock server bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();

        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            // Drain the request so the client isn't left waiting on a full
            // request write before we respond.
            let mut discard = [0u8; 1024];
            let _ = stream.read(&mut discard).await;
            let _ = stream.write_all(&raw_response).await;
            let _ = stream.shutdown().await;
        });

        format!("http://127.0.0.1:{}/", port)
    }

    #[tokio::test]
    async fn small_valid_json_parses() {
        let body = r#"{"ok":true,"n":1}"#;
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let url = spawn_raw_mock(raw.into_bytes()).await;
        let resp = reqwest::Client::new().get(&url).send().await.unwrap();
        let json = read_json_response_capped(resp).await.expect("should parse");
        assert_eq!(json["ok"], true);
        assert_eq!(json["n"], 1);
    }

    #[tokio::test]
    async fn oversized_content_length_rejected_before_read() {
        // Declares a body far larger than the cap; body itself is small — if
        // the function read the body anyway before checking, this would
        // incorrectly succeed. Asserts the Content-Length pre-check fires.
        let declared = MAX_JSON_RESPONSE_BYTES as u64 + 1;
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{{}}",
            declared
        );
        let url = spawn_raw_mock(raw.into_bytes()).await;
        let resp = reqwest::Client::new().get(&url).send().await.unwrap();
        let err = read_json_response_capped(resp).await.expect_err("must reject");
        assert!(err.contains("Content-Length"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn oversized_body_without_content_length_rejected_during_stream() {
        // No Content-Length header (server declares none) — cap must still be
        // enforced during the chunked read, not skipped because there was
        // nothing to pre-check.
        let oversized_body = "x".repeat(MAX_JSON_RESPONSE_BYTES + 1024);
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
            oversized_body
        );
        let url = spawn_raw_mock(raw.into_bytes()).await;
        let resp = reqwest::Client::new().get(&url).send().await.unwrap();
        let err = read_json_response_capped(resp).await.expect_err("must reject");
        assert!(err.contains("exceeds"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn invalid_json_within_size_cap_reports_parse_error() {
        let body = "not json";
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let url = spawn_raw_mock(raw.into_bytes()).await;
        let resp = reqwest::Client::new().get(&url).send().await.unwrap();
        let err = read_json_response_capped(resp).await.expect_err("must fail to parse");
        assert!(err.contains("parse"), "unexpected error: {err}");
    }
}
