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
/// Returns `Value::Null` on any miss: key absent, index out of range, a
/// scalar where a container was expected, or an empty path segment.
/// A segment made only of ASCII digits indexes into an array (`items.0.id`);
/// on an object it is looked up as an ordinary key.
///
/// Example: `traverse_dotpath(&val, "body.user.name")` is equivalent to
/// `val["body"]["user"]["name"]` but returns Null instead of panicking on miss.
pub fn traverse_dotpath(data: &Value, path: &str) -> Value {
    let mut current = data;
    for part in path.split('.') {
        let next = match current {
            Value::Array(items) => array_index(part).and_then(|i| items.get(i)),
            _ => current.get(part),
        };
        match next {
            Some(v) => current = v,
            None => return Value::Null,
        }
    }
    current.clone()
}

fn array_index(segment: &str) -> Option<usize> {
    if segment.is_empty() || !segment.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    segment.parse().ok()
}

#[cfg(test)]
mod traverse_dotpath_tests {
    use super::traverse_dotpath;
    use serde_json::{json, Value};

    #[test]
    fn dotpath_resolves_the_expected_value_per_shape() {
        let data = json!({
            "items": [{ "id": 7 }, { "id": 8 }],
            "map": { "0": "zero", "k": "v" },
            "matrix": [[1, 2], [3, 4]],
            "name": "x"
        });
        let cases: &[(&str, Value)] = &[
            ("items.0.id", json!(7)),
            ("items.1.id", json!(8)),
            ("matrix.1.0", json!(3)),
            ("map.0", json!("zero")),
            ("map.k", json!("v")),
            ("name", json!("x")),
            ("items.2.id", Value::Null),
            ("items.-1.id", Value::Null),
            ("items.+0.id", Value::Null),
            ("items.first", Value::Null),
            ("items.99999999999999999999.id", Value::Null),
            ("name.0", Value::Null),
            ("missing.0", Value::Null),
            ("", Value::Null),
        ];
        for (path, expected) in cases {
            assert_eq!(&traverse_dotpath(&data, path), expected, "path {path:?}");
        }
    }
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
    ordered_node_outputs_where(context, |_| true)
}

/// Same ordering as [`ordered_node_outputs`], restricted to the node ids `keep`
/// accepts. Outputs that are filtered out are never cloned.
pub fn ordered_node_outputs_where(
    context: &ExecutionContext,
    keep: impl Fn(&str) -> bool,
) -> Vec<(String, Value)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for id in context.execution_order.iter() {
        if !keep(id.as_str()) {
            continue;
        }
        if let Some(v) = context.node_outputs.get(id) {
            out.push((id.clone(), v.clone()));
            seen.insert(id.clone());
        }
    }
    let mut rest: Vec<(&String, &Value)> = context.node_outputs
        .iter()
        .filter(|(k, _)| keep(k.as_str()) && !seen.contains(*k))
        .collect();
    rest.sort_by(|a, b| a.0.cmp(b.0));
    out.extend(rest.into_iter().map(|(k, v)| (k.clone(), v.clone())));
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
    fn where_variant_keeps_only_accepted_ids_in_the_same_order() {
        let mut outputs = HashMap::new();
        outputs.insert("a".to_string(), json!(1));
        outputs.insert("b".to_string(), json!(2));
        outputs.insert("c".to_string(), json!(3));
        outputs.insert("z_unordered".to_string(), json!(4));
        let result = ordered_node_outputs_where(
            &ctx(outputs, vec!["c", "b", "a"]),
            |id| id != "b",
        );
        let ids: Vec<&str> = result.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["c", "a", "z_unordered"]);
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
/// multicast, `0.0.0.0/8`, reserved `240.0.0.0/4`, RFC 6598 shared address
/// space (100.64.0.0/10), and the Azure IMDS endpoint (168.63.129.16).
/// Under `Strict`, also rejects loopback and RFC 1918 private addresses;
/// `AllowLocal` permits those.
///
/// An IPv6 address that embeds an IPv4 address (IPv4-mapped, NAT64
/// `64:ff9b::/96`, 6to4 `2002::/16`, IPv4-compatible `::a.b.c.d`) is judged
/// by the same IPv4 rules applied to the embedded address, so a public IPv4
/// target reached through NAT64 still passes. `198.18.0.0/15` is deliberately
/// not blocked: fake-IP proxy tools resolve every domain into it.
///
/// Called for both IP-literal URLs and post-DNS domain resolution.
pub fn check_ssrf_ip(ip: std::net::IpAddr, policy: SsrfPolicy) -> Result<(), String> {
    check_ssrf_ip_impl(ip, policy.allow_local())
}

fn ipv4_blocked(v4: std::net::Ipv4Addr, allow_local: bool) -> bool {
    let azure_imds = std::net::Ipv4Addr::new(168, 63, 129, 16);
    let first = v4.octets()[0];
    let always_blocked = v4 == azure_imds
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_multicast()
        || first == 0
        || first & 0xF0 == 0xF0
        // RFC 6598 shared address space (100.64.0.0/10); is_private() only
        // covers RFC 1918.
        || u32::from(v4) & 0xFFC0_0000 == 0x6440_0000;
    always_blocked || (!allow_local && (v4.is_loopback() || v4.is_private()))
}

/// The IPv4 address carried inside an IPv6 address, if its form embeds one.
/// `::` and `::1` return `None` so the IPv6 checks judge them as themselves.
fn embedded_ipv4(v6: std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let s = v6.segments();
    let from = |hi: u16, lo: u16| {
        std::net::Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8)
    };
    let upper_zero = s[..5].iter().all(|&x| x == 0);
    if upper_zero && s[5] == 0xffff {
        return Some(from(s[6], s[7]));
    }
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6].iter().all(|&x| x == 0) {
        return Some(from(s[6], s[7]));
    }
    if s[0] == 0x2002 {
        return Some(from(s[1], s[2]));
    }
    if upper_zero && s[5] == 0 && ((u32::from(s[6]) << 16) | u32::from(s[7])) > 1 {
        return Some(from(s[6], s[7]));
    }
    None
}

fn check_ssrf_ip_impl(ip: std::net::IpAddr, allow_local: bool) -> Result<(), String> {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            if ipv4_blocked(v4, allow_local) {
                return Err(format!(
                    "Requests to private/internal IP addresses are not permitted ({})", v4
                ));
            }
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = embedded_ipv4(v6) {
                if ipv4_blocked(v4, allow_local) {
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
/// This is an early check that gives a clear error before any request is
/// built. It does not by itself stop DNS rebinding: a DNS server can answer
/// with a public address here and a private one when the connection is made.
/// Clients built with [`guarded_client_builder`] close that gap by running the
/// same address filter inside the connector, so the address that is validated
/// is the address that is dialled. Redis, SMTP and custom S3 endpoints get the
/// same filter through their own connection hooks (`resolve_validated_addrs`).
/// Plugin `wasi:http` requests are pinned the same way (`plugin_http`).
/// Postgres and MySQL connections (sqlx) cannot be pinned without breaking TLS
/// host verification and server-name indication, and keep the gap. A
/// verifying TLS mode (`verify-full`, `VERIFY_IDENTITY`) makes a rebound host
/// fail the handshake; a host-level egress firewall covers the other modes.
pub async fn check_host_ssrf(host: url::Host<&str>, port: u16, policy: SsrfPolicy) -> Result<(), String> {
    check_host_ssrf_impl(host, port, policy.allow_local()).await
}

/// Like [`check_host_ssrf`], but returns the validated addresses so the caller
/// can dial one of them instead of resolving the name a second time.
pub(crate) async fn resolve_host_validated(
    host: url::Host<&str>,
    port: u16,
    policy: SsrfPolicy,
) -> Result<Vec<std::net::SocketAddr>, String> {
    let allow_local = policy.allow_local();
    match host {
        url::Host::Ipv4(ip) => {
            let addr = std::net::IpAddr::V4(ip);
            check_ssrf_ip_impl(addr, allow_local)?;
            Ok(vec![std::net::SocketAddr::new(addr, port)])
        }
        url::Host::Ipv6(ip) => {
            let addr = std::net::IpAddr::V6(ip);
            check_ssrf_ip_impl(addr, allow_local)?;
            Ok(vec![std::net::SocketAddr::new(addr, port)])
        }
        url::Host::Domain(domain) => resolve_public_addrs(domain, port, allow_local)
            .await
            .map_err(DnsCheck::into_message),
    }
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

        url::Host::Domain(domain) => resolve_public_addrs(domain, port, allow_local)
            .await
            .map(|_| ())
            .map_err(DnsCheck::into_message),
    }
}

/// Why [`resolve_public_addrs`] refused a host.
enum DnsCheck {
    /// The name or one of its addresses is on the block list.
    Blocked(String),
    /// Resolution failed or returned nothing; says nothing about the target.
    Failed(String),
}

impl DnsCheck {
    fn into_message(self) -> String {
        match self {
            DnsCheck::Blocked(m) | DnsCheck::Failed(m) => m,
        }
    }
}

/// Rejects a name that is on the static block list, before any lookup.
fn check_domain_name(domain: &str, allow_local: bool) -> Result<(), DnsCheck> {
    let lower = domain.to_lowercase();
    let is_localhost = lower == "localhost" || lower.ends_with(".localhost");
    if (is_localhost && !allow_local) || lower == "metadata.google.internal" {
        return Err(DnsCheck::Blocked(format!("Requests to '{}' are not permitted", domain)));
    }
    Ok(())
}

/// Passes `addrs` through only if the list is non-empty and every address
/// clears the SSRF block list. One bad address rejects the whole name, so a
/// host that mixes public and internal answers is never partly used.
fn check_resolved_addrs(
    domain: &str,
    addrs: Vec<std::net::SocketAddr>,
    allow_local: bool,
) -> Result<Vec<std::net::SocketAddr>, DnsCheck> {
    if addrs.is_empty() {
        return Err(DnsCheck::Failed(format!(
            "DNS resolution returned no addresses for '{}'",
            domain
        )));
    }
    for addr in &addrs {
        check_ssrf_ip_impl(addr.ip(), allow_local).map_err(DnsCheck::Blocked)?;
    }
    Ok(addrs)
}

/// Resolves `domain` and returns its addresses if they pass the SSRF policy.
async fn resolve_public_addrs(
    domain: &str,
    port: u16,
    allow_local: bool,
) -> Result<Vec<std::net::SocketAddr>, DnsCheck> {
    check_domain_name(domain, allow_local)?;
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((domain, port))
        .await
        .map_err(|e| DnsCheck::Failed(format!("DNS resolution failed for '{}': {}", domain, e)))?
        .collect();
    check_resolved_addrs(domain, addrs, allow_local)
}

/// The error a guarded client reports when its connector refuses a target.
/// A distinct type lets callers tell a policy block from a network failure.
#[derive(Debug)]
struct SsrfBlocked(String);

impl std::fmt::Display for SsrfBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SsrfBlocked {}

/// DNS resolver for guarded clients: resolves with the system resolver, then
/// hands the connector only addresses that pass the SSRF policy.
struct SsrfResolver {
    allow_local: bool,
}

impl reqwest::dns::Resolve for SsrfResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let allow_local = self.allow_local;
        let host = name.as_str().to_owned();
        Box::pin(async move {
            // Port 0 lets reqwest substitute the URL's port or the scheme default.
            match resolve_public_addrs(&host, 0, allow_local).await {
                Ok(addrs) => Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs),
                Err(DnsCheck::Blocked(msg)) => {
                    Err(Box::new(SsrfBlocked(msg)) as Box<dyn std::error::Error + Send + Sync>)
                }
                Err(DnsCheck::Failed(msg)) => {
                    Err(Box::new(std::io::Error::other(msg)) as Box<dyn std::error::Error + Send + Sync>)
                }
            }
        })
    }
}

/// Resolves `host` and returns only addresses that pass `policy`. For clients
/// that expose a DNS hook but not a reqwest one (Redis, AWS SDK): the address
/// the connector dials is the one that was validated. A block carries
/// [`SsrfBlocked`] in the error's source chain.
pub(crate) async fn resolve_validated_addrs(
    host: &str,
    port: u16,
    policy: SsrfPolicy,
) -> std::io::Result<Vec<std::net::SocketAddr>> {
    resolve_public_addrs(host, port, policy.allow_local())
        .await
        .map_err(|e| match e {
            DnsCheck::Blocked(msg) => std::io::Error::other(SsrfBlocked(msg)),
            DnsCheck::Failed(msg) => std::io::Error::other(msg),
        })
}

/// Connect-time DNS filter for the Redis client. TLS (`rediss://`) still
/// verifies the original host name, since only the address lookup is replaced.
pub(crate) struct SsrfRedisResolver(pub(crate) SsrfPolicy);

impl redis::io::AsyncDNSResolver for SsrfRedisResolver {
    fn resolve<'a, 'b: 'a>(
        &'a self,
        host: &'b str,
        port: u16,
    ) -> redis::RedisFuture<'a, Box<dyn Iterator<Item = std::net::SocketAddr> + Send + 'a>> {
        let policy = self.0;
        Box::pin(async move {
            let addrs = resolve_validated_addrs(host, port, policy)
                .await
                .map_err(redis::RedisError::from)?;
            Ok(Box::new(addrs.into_iter()) as Box<dyn Iterator<Item = std::net::SocketAddr> + Send + 'a>)
        })
    }
}

/// Redirect policy for guarded clients: follows up to 10 hops, and refuses a
/// hop whose target is an IP literal the policy blocks. Literals never reach
/// the resolver, so without this a redirect to `http://169.254.169.254/` would
/// skip the connect-time filter. Domain targets are covered by the resolver.
fn ssrf_redirect_policy(allow_local: bool) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        let blocked = match attempt.url().host() {
            Some(url::Host::Ipv4(ip)) => {
                check_ssrf_ip_impl(std::net::IpAddr::V4(ip), allow_local).err()
            }
            Some(url::Host::Ipv6(ip)) => {
                check_ssrf_ip_impl(std::net::IpAddr::V6(ip), allow_local).err()
            }
            _ => None,
        };
        if let Some(msg) = blocked {
            attempt.error(SsrfBlocked(msg))
        } else if attempt.previous().len() >= 10 {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    })
}

fn parse_flag(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes")
    )
}

/// Whether the operator opted back into system proxies with
/// `AERINI_ALLOW_SYSTEM_PROXY`. Read once per process.
fn system_proxy_opted_in() -> bool {
    static OPTED_IN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OPTED_IN.get_or_init(|| {
        let on = parse_flag(std::env::var("AERINI_ALLOW_SYSTEM_PROXY").ok().as_deref());
        if on {
            tracing::warn!(
                "AERINI_ALLOW_SYSTEM_PROXY is set: outbound HTTP uses system proxies and the \
                 connect-time SSRF filter is off; only the pre-request SSRF check applies"
            );
        }
        on
    })
}

/// Starts a `reqwest` client whose connections are checked against `policy`
/// at connect time, which closes the check-then-connect (DNS rebinding) gap
/// that [`check_host_ssrf`] alone leaves open.
///
/// The builder carries three settings, so callers add only their own
/// (timeout, user agent, pool size, and a stricter redirect policy if wanted):
/// - a resolver that rejects the host if any of its addresses is blocked, and
///   hands the connector only the addresses that passed, so the address that
///   was validated is the one dialled;
/// - a redirect policy that also blocks IP-literal hops;
/// - no system proxy. A proxy resolves the target itself, so the filter could
///   not see it, and a private-address proxy would be rejected by the filter.
///
/// Operators who must route through a proxy can set
/// `AERINI_ALLOW_SYSTEM_PROXY=1`, which returns the builder without the
/// resolver and without `no_proxy`, leaving only the early pre-request check.
///
/// IP-literal URLs never reach the resolver; callers keep running
/// [`check_host_ssrf`] or [`check_host_ssrf_from_url`] first for those.
pub(crate) fn guarded_client_builder(policy: SsrfPolicy) -> reqwest::ClientBuilder {
    let allow_local = policy.allow_local();
    let builder = reqwest::Client::builder().redirect(ssrf_redirect_policy(allow_local));
    if system_proxy_opted_in() {
        return builder;
    }
    builder
        .dns_resolver(std::sync::Arc::new(SsrfResolver { allow_local }))
        .no_proxy()
}

/// True when `e` was caused by a guarded client refusing the target.
pub(crate) fn is_ssrf_blocked(e: &reqwest::Error) -> bool {
    use std::error::Error as _;
    let mut cause = e.source();
    while let Some(c) = cause {
        if c.is::<SsrfBlocked>() {
            return true;
        }
        cause = c.source();
    }
    false
}

/// A failed connection is always worth retrying, since nothing was sent. A
/// timeout can follow a request the provider already acted on, so it is
/// retried only for [`Replay::Safe`] calls. A policy block is never retried.
pub(crate) fn is_retryable_network_error(replay: Replay, e: &reqwest::Error) -> bool {
    if is_ssrf_blocked(e) {
        return false;
    }
    e.is_connect() || (e.is_timeout() && replay == Replay::Safe)
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

/// Renders a `reqwest::Error` as a message that is safe to store and show.
///
/// `reqwest::Error`'s `Display` appends ` for url (<full request URL>)`, and
/// many APIs carry secrets in the URL (a Telegram bot token in the path, a
/// Discord webhook token, a `?key=` query parameter), so `e.to_string()` must
/// never reach a node error, run history or log. This drops that suffix,
/// appends the underlying cause chain (so "connection refused" or a DNS
/// failure is still visible), then removes any remaining occurrence of the
/// URL and any `user:password@` credentials a cause may echo.
pub fn reqwest_err_msg(e: &reqwest::Error) -> String {
    use std::error::Error as _;

    let mut msg = e.to_string();
    if let Some(url) = e.url() {
        let suffix = format!(" for url ({url})");
        if let Some(head) = msg.strip_suffix(&suffix) {
            msg = head.to_string();
        }
    }

    let mut source = e.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !msg.ends_with(&text) {
            msg.push_str(": ");
            msg.push_str(&text);
        }
        source = cause.source();
    }

    if let Some(url) = e.url() {
        msg = msg.replace(url.as_str(), "[url]");
    }
    scrub_url_in_error(&msg)
}

/// Maps a reqwest network error to a `NodeOutput`. A connection failure is
/// recoverable (eligible for scheduler retry); a timeout is recoverable only
/// when `replay` is [`Replay::Safe`]. A target refused by the SSRF policy is
/// reported as `SSRF_BLOCKED` and never retried. All other errors are
/// unrecoverable.
pub fn http_err_output(replay: Replay, e: &reqwest::Error) -> NodeOutput {
    let msg = reqwest_err_msg(e);
    let code = if is_ssrf_blocked(e) { "SSRF_BLOCKED" } else { "HTTP_ERROR" };
    if is_retryable_network_error(replay, e) {
        NodeOutput::failure(NodeError::recoverable(code, msg))
    } else {
        NodeOutput::failure(NodeError::unrecoverable(code, msg))
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
            Err(e) => return Err(format!("Failed to read response body: {}", reqwest_err_msg(&e))),
        }
    }

    serde_json::from_slice::<Value>(&body_buf)
        .map_err(|e| format!("Failed to parse response as JSON: {}", e))
}

/// Whether sending a provider request a second time can repeat an effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replay {
    /// A repeat changes nothing: reads, calls that carry a stable idempotency
    /// key, and stateless generation (a repeat can at most bill twice).
    Safe,
    /// A repeat can duplicate an effect: creating records, sending messages,
    /// charging, queuing paid jobs.
    Never,
}

/// Builds the error for a failed provider call. A `429` is always
/// recoverable so the scheduler retries it. For [`Replay::Safe`] calls a
/// `502`, `503` or `504` (a gateway answering for a service that did not
/// finish the work) and an overloaded `529` are recoverable too. Everything
/// else, and every status on a [`Replay::Never`] call, is unrecoverable under
/// `code`: a gateway often returns those after the service already acted.
pub fn provider_error_for(replay: Replay, status: u16, code: &str, message: impl Into<String>) -> NodeError {
    match (status, replay) {
        (429, _) | (529, Replay::Safe) => NodeError::recoverable("RATE_LIMITED", message),
        (502..=504, Replay::Safe) => NodeError::recoverable("UPSTREAM_UNAVAILABLE", message),
        _ => NodeError::unrecoverable(code, message),
    }
}

/// [`provider_error_for`] with [`Replay::Never`]: only a `429` is retried.
pub fn provider_error(status: u16, code: &str, message: impl Into<String>) -> NodeError {
    provider_error_for(Replay::Never, status, code, message)
}

/// Consecutive failed polls tolerated before a polling node gives up.
pub(crate) const POLL_MAX_CONSECUTIVE_FAULTS: u32 = 5;
const POLL_DELAY_CAP_MS: u64 = 8_000;

/// One failed poll of an already-submitted provider job.
pub(crate) enum PollFault {
    /// Worth another poll: network error, 408/425/429/5xx, unreadable body.
    Transient(String),
    /// Polling cannot succeed; fail now.
    Fatal(NodeError),
}

/// Classifies a poll response status. `None` means the body should be read.
pub(crate) fn poll_status_fault(status: u16) -> Option<PollFault> {
    match status {
        0..=399 => None,
        408 | 425 | 429 | 500..=599 => Some(PollFault::Transient(format!("HTTP {status}"))),
        _ => Some(PollFault::Fatal(NodeError::unrecoverable(
            "POLL_REJECTED",
            format!("Poll request rejected with HTTP {status}; the job was not resubmitted and may still be billed by the provider."),
        ))),
    }
}

/// Classifies a poll send error. A policy block never gets better.
pub(crate) fn poll_network_fault(e: &reqwest::Error) -> PollFault {
    if is_ssrf_blocked(e) {
        PollFault::Fatal(NodeError::unrecoverable("SSRF_BLOCKED", reqwest_err_msg(e)))
    } else {
        PollFault::Transient(reqwest_err_msg(e))
    }
}

/// Counts consecutive failed polls. The poll is a GET on a job that already
/// exists, so a transient failure must never reach the scheduler as a
/// recoverable error: a node-level retry would submit, and bill, a second job.
pub(crate) struct PollRetry {
    provider: &'static str,
    consecutive: u32,
}

impl PollRetry {
    pub(crate) fn new(provider: &'static str) -> Self {
        Self { provider, consecutive: 0 }
    }

    /// Wait before the next poll: `base_ms`, doubled per consecutive fault, capped.
    pub(crate) fn delay_ms(&self, base_ms: u64) -> u64 {
        base_ms
            .saturating_mul(1u64 << self.consecutive.min(4))
            .min(POLL_DELAY_CAP_MS.max(base_ms))
    }

    pub(crate) fn good(&mut self) {
        self.consecutive = 0;
    }

    /// `Ok` = poll again. `Err` is always unrecoverable.
    pub(crate) fn fault(&mut self, fault: PollFault) -> Result<(), NodeError> {
        match fault {
            PollFault::Fatal(e) => Err(e),
            PollFault::Transient(reason) => {
                self.consecutive += 1;
                if self.consecutive >= POLL_MAX_CONSECUTIVE_FAULTS {
                    Err(NodeError::unrecoverable(
                        "POLL_FAILED",
                        format!(
                            "{} poll failed {} times in a row (last: {}). The job was not resubmitted and may still finish and be billed by the provider.",
                            self.provider, self.consecutive, reason
                        ),
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// Marks a failure that happened after the provider accepted the job as
/// unrecoverable, so a node-level retry cannot submit a second paid job.
pub(crate) fn after_submit(e: NodeError) -> NodeError {
    if !e.recoverable {
        return e;
    }
    NodeError::unrecoverable(
        e.code,
        format!("{} (job already submitted; not resubmitted)", e.message),
    )
}

/// Maximum bytes of a provider's error body included in a node error.
pub const MAX_ERROR_BODY_BYTES: usize = 1024;

fn truncate_lossy(mut buf: Vec<u8>, max: usize) -> String {
    let truncated = buf.len() > max;
    buf.truncate(max);
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if truncated {
        text.push_str("… [truncated]");
    }
    text
}

/// Reads at most `max` bytes of a response body as text, stopping the read as
/// soon as the limit is passed. A read failure yields what was read so far.
pub async fn read_text_capped(mut response: reqwest::Response, max: usize) -> String {
    let mut buf: Vec<u8> = Vec::new();
    while buf.len() <= max {
        match response.chunk().await {
            Ok(Some(chunk)) => buf.extend_from_slice(&chunk),
            _ => break,
        }
    }
    truncate_lossy(buf, max)
}

const TWO_POW_64: f64 = 18_446_744_073_709_551_616.0;

fn finite_f64_from_str(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|f| f.is_finite())
}

fn whole_f64_to_u64(f: f64) -> Option<u64> {
    (f.fract() == 0.0 && (0.0..TWO_POW_64).contains(&f)).then_some(f as u64)
}

/// Reads a non-negative whole number from a node config value: a JSON number,
/// or a string holding one (the form a `{{...}}` expression resolves to).
/// Fractions, negatives, out-of-range values and anything else are `None`.
pub fn cfg_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().and_then(whole_f64_to_u64)),
        Value::String(s) => {
            let t = s.trim();
            if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
                t.parse().ok()
            } else {
                finite_f64_from_str(t).and_then(whole_f64_to_u64)
            }
        }
        _ => None,
    }
}

/// Reads a finite number from a node config value: a JSON number or a numeric
/// string. `NaN`, infinities and strings that overflow `f64` are `None`.
pub fn cfg_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64().filter(|f| f.is_finite()),
        Value::String(s) => finite_f64_from_str(s),
        _ => None,
    }
}

/// Reads a boolean from a node config value: a JSON bool, or the string
/// `true` / `false` in any case. Other strings (`1`, `yes`) are `None`.
pub fn cfg_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => {
            let t = s.trim();
            if t.eq_ignore_ascii_case("true") {
                Some(true)
            } else if t.eq_ignore_ascii_case("false") {
                Some(false)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn cfg_is_unset(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

fn invalid_config(field: &str, expected: &str) -> NodeError {
    NodeError::unrecoverable("INVALID_CONFIG", format!("{field} must be {expected}"))
}

/// Like `cfg_u64`, but a value that is set and unparseable is an
/// `INVALID_CONFIG` error instead of `None`. Absent, null and blank are `Ok(None)`.
pub fn cfg_u64_opt(v: &Value, field: &str) -> Result<Option<u64>, NodeError> {
    if cfg_is_unset(v) {
        return Ok(None);
    }
    cfg_u64(v).map(Some).ok_or_else(|| invalid_config(field, "a whole number, 0 or more"))
}

/// Like `cfg_f64`, with the same unset / invalid split as `cfg_u64_opt`.
pub fn cfg_f64_opt(v: &Value, field: &str) -> Result<Option<f64>, NodeError> {
    if cfg_is_unset(v) {
        return Ok(None);
    }
    cfg_f64(v).map(Some).ok_or_else(|| invalid_config(field, "a number"))
}

/// Like `cfg_bool`, with the same unset / invalid split as `cfg_u64_opt`.
pub fn cfg_bool_opt(v: &Value, field: &str) -> Result<Option<bool>, NodeError> {
    if cfg_is_unset(v) {
        return Ok(None);
    }
    cfg_bool(v).map(Some).ok_or_else(|| invalid_config(field, "true or false"))
}

/// Decodes the base64 `data` of a media-contract file entry. Accepts a leading
/// `data:<mime>;base64,` prefix, whitespace or line breaks anywhere in the
/// payload, and missing `=` padding. Anything else that is not standard base64
/// is an error; a stray comma is not treated as a prefix separator.
pub(crate) fn decode_file_data(data: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};

    let trimmed = data.trim_start();
    let payload = match trimmed.get(..5) {
        Some(head) if head.eq_ignore_ascii_case("data:") => match trimmed.split_once(',') {
            Some((_, rest)) => rest,
            None => return Err("data URI has no payload".to_string()),
        },
        _ => trimmed,
    };
    let compact: String = payload.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    STANDARD
        .decode(&compact)
        .or_else(|_| STANDARD_NO_PAD.decode(compact.trim_end_matches('=')))
        .map_err(|e| format!("base64 decode failed: {e}"))
}

/// Hands out filenames that are unique within one batch, comparing
/// case-insensitively because the common target filesystems (NTFS, APFS) do.
#[derive(Default)]
pub(crate) struct FilenameSet(std::collections::HashSet<String>);

impl FilenameSet {
    /// Reserves `name`, or the first free variant of it. With `tag` the variants
    /// are `stem_{tag}.ext`, `stem_{tag}_2.ext`, ...; without it `stem_2.ext`,
    /// `stem_3.ext`, ... The extension is always preserved.
    pub(crate) fn claim(&mut self, name: &str, tag: Option<usize>) -> String {
        let (stem, ext) = match name.rfind('.') {
            Some(pos) if pos > 0 => name.split_at(pos),
            _ => (name, ""),
        };
        let mut candidate = name.to_string();
        let mut attempt = 0usize;
        while self.0.contains(&candidate.to_lowercase()) {
            attempt += 1;
            candidate = match (tag, attempt) {
                (Some(t), 1) => format!("{stem}_{t}{ext}"),
                (Some(t), n) => format!("{stem}_{t}_{n}{ext}"),
                (None, n) => format!("{stem}_{}{ext}", n + 1),
            };
        }
        self.0.insert(candidate.to_lowercase());
        candidate
    }
}

/// Per-stream cap on captured child-process output.
pub(crate) const OUTPUT_CAP_BYTES: usize = 10 * 1024 * 1024;

/// Which end of an over-long stream `drain_capped` keeps.
#[derive(Clone, Copy)]
pub(crate) enum Keep {
    Head,
    Tail,
}

/// Most bytes one output stream may produce before the child is stopped. Far
/// above `OUTPUT_CAP_BYTES` (which only limits what is kept), so only a child
/// that writes without end reaches it.
pub(crate) const OUTPUT_FLOOD_BYTES: usize = 256 * 1024 * 1024;

/// Per-stream byte budget plus the signal a drain raises when it is spent.
#[derive(Clone)]
pub(crate) struct FloodLimit {
    bytes: usize,
    signal: std::sync::Arc<tokio::sync::Notify>,
}

impl FloodLimit {
    pub(crate) fn new(bytes: usize) -> Self {
        Self { bytes, signal: std::sync::Arc::new(tokio::sync::Notify::new()) }
    }

    /// Resolves once any drain sharing this limit has read more than its budget.
    pub(crate) async fn tripped(&self) {
        self.signal.notified().await
    }
}

#[derive(Default)]
pub(crate) struct Captured {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
    pub(crate) flooded: bool,
}

/// Reads `reader` to EOF and keeps at most `cap` bytes from the chosen end.
/// Bytes past the cap are read and discarded, never left in the pipe, so a
/// child that writes more than `cap` is not blocked on a full pipe.
/// `None` (a stream that was not piped) yields an empty capture.
/// Reading stops, and `flood` is signalled, once the stream has produced more
/// than the flood budget in total.
pub(crate) async fn drain_capped<R>(reader: Option<R>, cap: usize, keep: Keep, flood: &FloodLimit) -> Captured
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut out = Captured::default();
    let Some(mut reader) = reader else { return out };
    let mut chunk = vec![0u8; 16 * 1024];
    let mut total = 0usize;
    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        total = total.saturating_add(n);
        if total > flood.bytes {
            out.flooded = true;
            flood.signal.notify_one();
            break;
        }
        match keep {
            Keep::Head => {
                let room = cap.saturating_sub(out.bytes.len());
                out.bytes.extend_from_slice(&chunk[..n.min(room)]);
                if n > room {
                    out.truncated = true;
                }
            }
            Keep::Tail => {
                out.bytes.extend_from_slice(&chunk[..n]);
                if out.bytes.len() > cap.saturating_mul(2) {
                    let excess = out.bytes.len() - cap;
                    out.bytes.drain(..excess);
                    out.truncated = true;
                }
            }
        }
    }
    if out.bytes.len() > cap {
        let excess = out.bytes.len() - cap;
        out.bytes.drain(..excess);
        out.truncated = true;
    }
    out
}

#[cfg(test)]
mod drain_tests {
    use super::*;

    #[tokio::test]
    async fn drain_capped_keeps_the_requested_end_and_flags_overflow() {
        let data: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
        let cases: [(usize, Keep, &[u8], bool); 6] = [
            (40_000, Keep::Head, &data[..], false),
            (50_000, Keep::Tail, &data[..], false),
            (40_000, Keep::Tail, &data[..], false),
            (1_000, Keep::Head, &data[..1_000], true),
            (1_000, Keep::Tail, &data[39_000..], true),
            (39_999, Keep::Head, &data[..39_999], true),
        ];
        for (cap, keep, want, truncated) in cases {
            let got = drain_capped(Some(&data[..]), cap, keep, &FloodLimit::new(usize::MAX)).await;
            assert_eq!(got.bytes, want, "cap {cap}");
            assert_eq!(got.truncated, truncated, "cap {cap}");
            assert!(!got.flooded);
        }
    }

    #[tokio::test]
    async fn drain_capped_stops_and_signals_when_a_stream_exceeds_the_flood_budget() {
        let data = vec![b'x'; 100_000];
        let flood = FloodLimit::new(40_000);
        let got = drain_capped(Some(&data[..]), 1_000, Keep::Head, &flood).await;
        assert!(got.flooded);
        assert!(got.bytes.len() <= 1_000);
        tokio::time::timeout(std::time::Duration::from_secs(1), flood.tripped())
            .await
            .expect("flood signal was not raised");
    }
}

#[cfg(test)]
mod provider_error_tests {
    use super::*;

    #[test]
    fn status_429_is_recoverable_others_are_not() {
        let e = provider_error(429, "X_ERROR", "slow down");
        assert!(e.recoverable);
        assert_eq!(e.code, "RATE_LIMITED");
        let e = provider_error(400, "X_ERROR", "bad");
        assert!(!e.recoverable);
        assert_eq!(e.code, "X_ERROR");
    }

    #[test]
    fn gateway_statuses_retry_only_for_safe_calls() {
        let cases: [(Replay, u16, bool, &str); 12] = [
            (Replay::Safe, 429, true, "RATE_LIMITED"),
            (Replay::Safe, 529, true, "RATE_LIMITED"),
            (Replay::Safe, 502, true, "UPSTREAM_UNAVAILABLE"),
            (Replay::Safe, 503, true, "UPSTREAM_UNAVAILABLE"),
            (Replay::Safe, 504, true, "UPSTREAM_UNAVAILABLE"),
            (Replay::Safe, 500, false, "X_ERROR"),
            (Replay::Safe, 400, false, "X_ERROR"),
            (Replay::Never, 429, true, "RATE_LIMITED"),
            (Replay::Never, 502, false, "X_ERROR"),
            (Replay::Never, 503, false, "X_ERROR"),
            (Replay::Never, 504, false, "X_ERROR"),
            (Replay::Never, 529, false, "X_ERROR"),
        ];
        for (replay, status, recoverable, code) in cases {
            let e = provider_error_for(replay, status, "X_ERROR", "m");
            assert_eq!((e.recoverable, e.code.as_str()), (recoverable, code), "{replay:?} {status}");
        }
    }

    #[test]
    fn provider_error_never_retries_a_gateway_failure() {
        assert!(!provider_error(503, "X_ERROR", "m").recoverable);
    }

    #[test]
    fn error_body_is_cut_at_the_limit() {
        let out = truncate_lossy(vec![b'a'; 2000], MAX_ERROR_BODY_BYTES);
        assert!(out.starts_with(&"a".repeat(1024)));
        assert!(out.ends_with("[truncated]"));
        assert_eq!(truncate_lossy(b"short".to_vec(), MAX_ERROR_BODY_BYTES), "short");
    }
}

#[cfg(test)]
pub(crate) mod poll_test_support {
    /// Serves one canned `(status, body)` per connection, in order, then
    /// stops listening. Returns the base URL.
    pub(crate) async fn spawn_sequence_mock(responses: Vec<(u16, &'static str)>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock server bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            for (status, body) in responses {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                let mut discard = [0u8; 2048];
                let _ = stream.read(&mut discard).await;
                let raw = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(raw.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        format!("http://127.0.0.1:{port}")
    }
}

#[cfg(test)]
mod poll_tests {
    use super::*;

    #[test]
    fn poll_status_table_classifies_each_http_status() {
        for s in [200u16, 204, 302] {
            assert!(poll_status_fault(s).is_none(), "{s} should be read");
        }
        for s in [408u16, 425, 429, 500, 502, 503, 504] {
            assert!(matches!(poll_status_fault(s), Some(PollFault::Transient(_))), "{s} should be transient");
        }
        for s in [400u16, 401, 403, 404, 410, 422] {
            match poll_status_fault(s) {
                Some(PollFault::Fatal(e)) => assert!(!e.recoverable, "{s}"),
                _ => panic!("{s} should be fatal"),
            }
        }
    }

    #[test]
    fn five_consecutive_transient_faults_end_unrecoverable_and_say_not_resubmitted() {
        let mut retry = PollRetry::new("Acme");
        for _ in 0..4 {
            assert!(retry.fault(PollFault::Transient("HTTP 503".into())).is_ok());
        }
        let e = retry.fault(PollFault::Transient("HTTP 503".into())).expect_err("fifth must stop");
        assert_eq!(e.code, "POLL_FAILED");
        assert!(!e.recoverable);
        assert!(e.message.contains("not resubmitted"), "{}", e.message);
        assert!(e.message.contains("Acme"));
    }

    #[test]
    fn a_good_poll_resets_the_fault_streak() {
        let mut retry = PollRetry::new("Acme");
        for _ in 0..4 {
            retry.fault(PollFault::Transient("x".into())).unwrap();
        }
        retry.good();
        for _ in 0..4 {
            assert!(retry.fault(PollFault::Transient("x".into())).is_ok());
        }
    }

    #[test]
    fn poll_delay_doubles_per_fault_and_is_capped() {
        let mut retry = PollRetry::new("Acme");
        assert_eq!(retry.delay_ms(500), 500);
        retry.fault(PollFault::Transient("x".into())).unwrap();
        assert_eq!(retry.delay_ms(500), 1_000);
        retry.fault(PollFault::Transient("x".into())).unwrap();
        retry.fault(PollFault::Transient("x".into())).unwrap();
        retry.fault(PollFault::Transient("x".into())).unwrap();
        assert_eq!(retry.delay_ms(500), 8_000);
        assert_eq!(retry.delay_ms(2_000), 8_000);
    }

    #[test]
    fn failure_after_submit_is_never_recoverable() {
        let e = after_submit(NodeError::recoverable("NETWORK_ERROR", "timed out"));
        assert!(!e.recoverable);
        assert_eq!(e.code, "NETWORK_ERROR");
        assert!(e.message.contains("not resubmitted"));
        assert_eq!(after_submit(NodeError::unrecoverable("X", "m")).message, "m");
    }
}

#[cfg(test)]
mod reqwest_err_msg_tests {
    use super::reqwest_err_msg;

    #[tokio::test]
    async fn omits_request_url_but_keeps_cause() {
        // Port 1 on loopback refuses connections immediately.
        let err = reqwest::Client::new()
            .get("http://127.0.0.1:1/bot123456:SECRET-TOKEN/sendMessage?key=QSECRET")
            .send()
            .await
            .expect_err("connection to port 1 must fail");

        let msg = reqwest_err_msg(&err);
        assert!(!msg.contains("SECRET-TOKEN"), "token leaked: {msg}");
        assert!(!msg.contains("QSECRET"), "query secret leaked: {msg}");
        assert!(msg.starts_with("error sending request"), "unexpected message: {msg}");
        assert!(msg.len() > "error sending request".len(), "cause chain missing: {msg}");
    }

    #[tokio::test]
    async fn builder_error_omits_url() {
        let err = reqwest::Client::new()
            .get("ftp://user:hunter2@example.invalid/SECRET-PATH")
            .send()
            .await
            .expect_err("unsupported scheme must fail");
        let msg = reqwest_err_msg(&err);
        assert!(!msg.contains("SECRET-PATH"), "path leaked: {msg}");
        assert!(!msg.contains("hunter2"), "password leaked: {msg}");
    }
}

#[cfg(test)]
mod guarded_client_tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn sa(a: u8, b: u8, c: u8, d: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(a, b, c, d)), 0)
    }

    #[test]
    fn resolved_addresses_are_filtered_per_policy() {
        let public = || vec![sa(93, 184, 216, 34)];
        let private = || vec![sa(10, 0, 0, 5)];
        let loopback = || vec![sa(127, 0, 0, 1)];
        let metadata = || vec![sa(169, 254, 169, 254)];
        let mixed = || vec![sa(93, 184, 216, 34), sa(10, 0, 0, 5)];

        // (answer, passes under Strict, passes under AllowLocal)
        let cases: Vec<(Vec<SocketAddr>, bool, bool)> = vec![
            (public(), true, true),
            (private(), false, true),
            (loopback(), false, true),
            (metadata(), false, false),
            (mixed(), false, true),
            (vec![sa(93, 184, 216, 34), sa(169, 254, 169, 254)], false, false),
        ];
        for (answer, strict, local) in cases {
            let label = format!("{:?}", answer);
            assert_eq!(check_resolved_addrs("h", answer.clone(), false).is_ok(), strict, "strict {label}");
            assert_eq!(check_resolved_addrs("h", answer, true).is_ok(), local, "allow_local {label}");
        }
        assert!(matches!(check_resolved_addrs("h", vec![], false), Err(DnsCheck::Failed(_))));
    }

    #[test]
    fn a_rebinding_host_is_refused_on_its_second_answer() {
        // A hostname that answers public, then private. Each connect resolves
        // again through the filter, so only the first answer gets through.
        let first = check_resolved_addrs("h", vec![sa(93, 184, 216, 34)], false);
        let second = check_resolved_addrs("h", vec![sa(192, 168, 1, 10)], false);
        assert!(first.is_ok());
        assert!(matches!(second, Err(DnsCheck::Blocked(_))));
    }

    #[test]
    fn blocked_names_are_refused_before_any_lookup() {
        for (name, allow_local, blocked) in [
            ("localhost", false, true),
            ("API.Localhost", false, true),
            ("localhost", true, false),
            ("metadata.google.internal", true, true),
            ("example.com", false, false),
        ] {
            assert_eq!(
                matches!(check_domain_name(name, allow_local), Err(DnsCheck::Blocked(_))),
                blocked,
                "{name} allow_local={allow_local}"
            );
        }
    }

    #[test]
    fn proxy_opt_in_flag_accepts_only_explicit_truthy_values() {
        for (value, expected) in [
            (None, false),
            (Some(""), false),
            (Some("0"), false),
            (Some("no"), false),
            (Some("1"), true),
            (Some(" TRUE "), true),
            (Some("yes"), true),
        ] {
            assert_eq!(parse_flag(value), expected, "{value:?}");
        }
    }

    async fn free_port() -> u16 {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    }

    /// Minimal HTTP server on loopback: `/start` redirects to `redirect_to`,
    /// every other path answers 200.
    async fn spawn_redirecting_server(redirect_to: String) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let target = redirect_to.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let path = request.split_whitespace().nth(1).unwrap_or("/");
                    let response = if path == "/start" {
                        format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    } else {
                        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_string()
                    };
                    let _ = sock.write_all(response.as_bytes()).await;
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn strict_client_refuses_a_hostname_that_resolves_to_loopback() {
        let client = guarded_client_builder(SsrfPolicy::Strict).build().unwrap();
        let port = free_port().await;
        let err = client
            .get(format!("http://localhost:{port}/"))
            .send()
            .await
            .expect_err("loopback by name must be refused");
        assert!(is_ssrf_blocked(&err), "not flagged as blocked: {}", reqwest_err_msg(&err));
        assert!(!is_retryable_network_error(Replay::Safe, &err));

        let out = http_err_output(Replay::Safe, &err);
        let failure = out.error.unwrap();
        assert_eq!(failure.code, "SSRF_BLOCKED");
        assert!(!failure.recoverable);
    }

    #[tokio::test]
    async fn allow_local_client_reaches_a_loopback_hostname() {
        let port = spawn_redirecting_server("/unused".into()).await;
        let client = guarded_client_builder(SsrfPolicy::AllowLocal).build().unwrap();
        let resp = client
            .get(format!("http://localhost:{port}/ok"))
            .send()
            .await
            .expect("AllowLocal must reach localhost");
        assert_eq!(resp.status().as_u16(), 200);
    }

    #[tokio::test]
    async fn refused_connection_stays_retryable() {
        let port = free_port().await;
        let client = guarded_client_builder(SsrfPolicy::AllowLocal).build().unwrap();
        let err = client
            .get(format!("http://localhost:{port}/"))
            .send()
            .await
            .expect_err("nothing is listening");
        assert!(!is_ssrf_blocked(&err));
        assert!(is_retryable_network_error(Replay::Never, &err), "{}", reqwest_err_msg(&err));
    }

    #[tokio::test]
    async fn timeout_after_the_request_went_out_is_retryable_only_for_safe_calls() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock);
            }
        });
        let client = guarded_client_builder(SsrfPolicy::AllowLocal)
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();
        let err = client
            .post(format!("http://localhost:{port}/"))
            .body("x")
            .send()
            .await
            .expect_err("the server never answers");
        assert!(err.is_timeout() && !err.is_connect(), "{}", reqwest_err_msg(&err));
        assert!(is_retryable_network_error(Replay::Safe, &err));
        assert!(!is_retryable_network_error(Replay::Never, &err));

        let failure = http_err_output(Replay::Never, &err).error.unwrap();
        assert_eq!(failure.code, "HTTP_ERROR");
        assert!(!failure.recoverable);
        assert!(http_err_output(Replay::Safe, &err).error.unwrap().recoverable);
    }

    #[tokio::test]
    async fn redirect_to_a_blocked_ip_literal_is_refused() {
        let port = spawn_redirecting_server("http://169.254.169.254/latest/meta-data".into()).await;
        // AllowLocal admits the loopback test server, and 169.254.0.0/16 stays
        // blocked under it, so the redirect hop is the only thing refused.
        let client = guarded_client_builder(SsrfPolicy::AllowLocal).build().unwrap();
        let err = client
            .get(format!("http://127.0.0.1:{port}/start"))
            .send()
            .await
            .expect_err("redirect to metadata address must be refused");
        assert!(is_ssrf_blocked(&err), "{}", reqwest_err_msg(&err));
    }

    #[tokio::test]
    async fn redirect_to_an_allowed_target_is_followed() {
        let port = spawn_redirecting_server("/ok".into()).await;
        let client = guarded_client_builder(SsrfPolicy::AllowLocal).build().unwrap();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/start"))
            .send()
            .await
            .expect("same-host redirect must be followed");
        assert_eq!(resp.status().as_u16(), 200);
    }
}

#[cfg(test)]
mod redis_resolver_tests {
    use super::*;
    use redis::io::AsyncDNSResolver;

    #[tokio::test]
    async fn strict_redis_resolver_refuses_localhost() {
        let err = match SsrfRedisResolver(SsrfPolicy::Strict).resolve("localhost", 6379).await {
            Ok(_) => panic!("loopback by name must be refused"),
            Err(e) => e,
        };
        assert!(err.is_io_error());
        assert!(err.to_string().contains("not permitted"), "{err}");
    }

    #[tokio::test]
    async fn allow_local_redis_resolver_returns_loopback_for_localhost() {
        let addrs: Vec<_> = SsrfRedisResolver(SsrfPolicy::AllowLocal)
            .resolve("localhost", 6379)
            .await
            .expect("AllowLocal must resolve localhost")
            .collect();
        assert!(!addrs.is_empty());
        assert!(addrs.iter().all(|a| a.ip().is_loopback() && a.port() == 6379), "{addrs:?}");
    }
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
mod ssrf_embedded_ipv4_tests {
    use super::{check_ssrf_ip, SsrfPolicy};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr { IpAddr::V4(Ipv4Addr::new(a, b, c, d)) }
    fn v6(s: &str) -> IpAddr { IpAddr::V6(s.parse::<Ipv6Addr>().unwrap()) }
    fn strict(ip: IpAddr) -> bool { check_ssrf_ip(ip, SsrfPolicy::Strict).is_ok() }
    fn local(ip: IpAddr) -> bool { check_ssrf_ip(ip, SsrfPolicy::AllowLocal).is_ok() }

    #[test]
    fn nat64_public_address_allowed() {
        assert!(strict(v6("64:ff9b::808:808")));
        assert!(local(v6("64:ff9b::808:808")));
    }

    #[test]
    fn nat64_internal_addresses_blocked_under_strict() {
        assert!(!strict(v6("64:ff9b::a00:1")));
        assert!(!strict(v6("64:ff9b::7f00:1")));
        assert!(!strict(v6("64:ff9b::a9fe:a9fe")));
    }

    #[test]
    fn nat64_allow_local_permits_private_but_not_metadata() {
        assert!(local(v6("64:ff9b::a00:1")));
        assert!(local(v6("64:ff9b::7f00:1")));
        assert!(!local(v6("64:ff9b::a9fe:a9fe")));
    }

    #[test]
    fn six_to_four_embedding_private_blocked() {
        assert!(!strict(v6("2002:a00:1::1")));
        assert!(local(v6("2002:a00:1::1")));
        assert!(!local(v6("2002:a9fe:a9fe::1")));
        assert!(strict(v6("2002:808:808::1")));
    }

    #[test]
    fn ipv4_compatible_loopback_blocked_under_strict() {
        assert!(!strict(v6("::127.0.0.1")));
        assert!(local(v6("::127.0.0.1")));
        assert!(!local(v6("::169.254.169.254")));
        assert!(strict(v6("::8.8.8.8")));
    }

    #[test]
    fn plain_ipv6_loopback_and_unspecified_keep_their_own_rules() {
        assert!(!strict(v6("::1")));
        assert!(local(v6("::1")));
        assert!(!local(v6("::")));
    }

    #[test]
    fn zero_and_reserved_ipv4_blocked_under_both_policies() {
        for ip in [v4(0, 0, 0, 0), v4(0, 1, 2, 3), v4(240, 0, 0, 1), v4(255, 255, 255, 254)] {
            assert!(!strict(ip), "{ip}");
            assert!(!local(ip), "{ip}");
        }
        assert!(!local(v6("::ffff:240.0.0.1")));
        assert!(!local(v6("64:ff9b::f000:1")));
    }

    #[test]
    fn fake_ip_range_198_18_stays_allowed() {
        assert!(strict(v4(198, 18, 0, 1)));
        assert!(strict(v4(198, 19, 255, 254)));
    }
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

#[cfg(test)]
mod cfg_helper_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cfg_u64_accepts_numbers_and_whole_number_strings_only() {
        let cases: &[(Value, Option<u64>)] = &[
            (json!(1000), Some(1000)),
            (json!(2.0), Some(2)),
            (json!("1000"), Some(1000)),
            (json!(" 7 "), Some(7)),
            (json!("3.0"), Some(3)),
            (json!("18446744073709551615"), Some(u64::MAX)),
            (json!("18446744073709551616"), None),
            (json!("1.5"), None),
            (json!(2.5), None),
            (json!("-5"), None),
            (json!(-1), None),
            (json!("abc"), None),
            (json!(""), None),
            (json!("inf"), None),
            (json!("0x10"), None),
            (json!(true), None),
            (Value::Null, None),
        ];
        for (input, expected) in cases {
            assert_eq!(cfg_u64(input), *expected, "input: {input}");
        }
    }

    #[test]
    fn cfg_f64_accepts_finite_numbers_and_numeric_strings_only() {
        let cases: &[(Value, Option<f64>)] = &[
            (json!(2.5), Some(2.5)),
            (json!(3), Some(3.0)),
            (json!("2"), Some(2.0)),
            (json!(" -0.5 "), Some(-0.5)),
            (json!("1e3"), Some(1000.0)),
            (json!("NaN"), None),
            (json!("inf"), None),
            (json!("1e999"), None),
            (json!("abc"), None),
            (json!(""), None),
            (json!(false), None),
            (Value::Null, None),
        ];
        for (input, expected) in cases {
            assert_eq!(cfg_f64(input), *expected, "input: {input}");
        }
    }

    #[test]
    fn cfg_bool_accepts_bools_and_true_false_strings_only() {
        let cases: &[(Value, Option<bool>)] = &[
            (json!(true), Some(true)),
            (json!(false), Some(false)),
            (json!("true"), Some(true)),
            (json!("TRUE"), Some(true)),
            (json!(" False "), Some(false)),
            (json!("1"), None),
            (json!("yes"), None),
            (json!(""), None),
            (json!(1), None),
            (Value::Null, None),
        ];
        for (input, expected) in cases {
            assert_eq!(cfg_bool(input), *expected, "input: {input}");
        }
    }

    #[test]
    fn set_but_unparseable_config_is_invalid_while_unset_is_none() {
        let unset = [Value::Null, json!(""), json!("  ")];
        for v in &unset {
            assert_eq!(cfg_u64_opt(v, "f").unwrap(), None, "input: {v}");
            assert_eq!(cfg_f64_opt(v, "f").unwrap(), None, "input: {v}");
            assert_eq!(cfg_bool_opt(v, "f").unwrap(), None, "input: {v}");
        }
        assert_eq!(cfg_u64_opt(&json!("7"), "f").unwrap(), Some(7));
        assert_eq!(cfg_f64_opt(&json!(0.5), "f").unwrap(), Some(0.5));
        assert_eq!(cfg_bool_opt(&json!("False"), "f").unwrap(), Some(false));

        let err = cfg_u64_opt(&json!("abc"), "duration").unwrap_err();
        assert_eq!(err.code, "INVALID_CONFIG");
        assert!(!err.recoverable);
        assert!(err.message.contains("duration"));
        assert!(!err.message.contains("abc"));
        assert!(cfg_u64_opt(&json!(-1), "f").is_err());
        assert!(cfg_f64_opt(&json!("NaN"), "f").is_err());
        assert!(cfg_bool_opt(&json!("yes"), "f").is_err());
        assert!(cfg_bool_opt(&json!(1), "f").is_err());
    }
}

#[cfg(test)]
mod file_batch_helper_tests {
    use super::*;

    #[test]
    fn decode_file_data_accepts_the_shapes_producers_emit() {
        let cases: &[(&str, Option<&[u8]>)] = &[
            ("SGVsbG8=", Some(b"Hello")),
            ("SGVsbG8", Some(b"Hello")),
            ("data:text/plain;base64,SGVsbG8=", Some(b"Hello")),
            ("DATA:text/plain;base64,SGVs\r\nbG8=", Some(b"Hello")),
            ("  SGVs bG8=\n", Some(b"Hello")),
            ("", Some(b"")),
            ("hello, SGVsbG8=", None),
            ("not base64!", None),
            ("data:text/plain;base64", None),
        ];
        for (input, expected) in cases {
            assert_eq!(decode_file_data(input).ok().as_deref(), *expected, "input: {input:?}");
        }
    }

    #[test]
    fn filename_set_keeps_every_name_unique_and_preserves_extensions() {
        let mut set = FilenameSet::default();
        assert_eq!(set.claim("a.txt", None), "a.txt");
        assert_eq!(set.claim("a.txt", None), "a_2.txt");
        assert_eq!(set.claim("A.TXT", None), "A_3.TXT");
        assert_eq!(set.claim("README", None), "README");
        assert_eq!(set.claim("README", None), "README_2");
        assert_eq!(set.claim(".env", None), ".env");
        assert_eq!(set.claim(".env", None), ".env_2");

        let mut tagged = FilenameSet::default();
        assert_eq!(tagged.claim("f.txt", Some(1)), "f.txt");
        assert_eq!(tagged.claim("f.txt", Some(1)), "f_1.txt");
        assert_eq!(tagged.claim("f.txt", Some(1)), "f_1_2.txt");
        assert_eq!(tagged.claim("f.txt", Some(1)), "f_1_3.txt");
        assert_eq!(tagged.claim("f_1.txt", Some(1)), "f_1_1.txt");
    }
}
