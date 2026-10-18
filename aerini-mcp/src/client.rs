use std::time::Duration;

use reqwest::header::{ETAG, RETRY_AFTER};
use reqwest::{Method, Url};
use serde_json::Value;

use crate::output::{clip_text, sanitize_text};

const ERROR_TEXT_LIMIT: usize = 400;

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base: Url,
    token: String,
    timeout: Duration,
}

#[derive(Debug)]
pub struct ApiResponse {
    pub body: Vec<u8>,
    pub etag: Option<String>,
}

impl ApiResponse {
    pub fn json(&self) -> Result<Value, String> {
        serde_json::from_slice(&self.body)
            .map_err(|e| format!("The Aerini server sent a body that is not JSON: {e}"))
    }

    pub fn row_version(&self) -> Option<i64> {
        self.etag.as_deref().and_then(|raw| raw.trim().trim_matches('"').parse().ok())
    }
}

impl ApiClient {
    pub fn new(base_url: &str, token: String, timeout: Duration) -> Result<Self, String> {
        let base = Url::parse(base_url).map_err(|e| format!("AERINI_URL is not a valid URL: {e}"))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err("AERINI_URL must start with http:// or https://".into());
        }
        if !base.username().is_empty() || base.password().is_some() {
            return Err("AERINI_URL must not contain a username or password; use AERINI_TOKEN".into());
        }
        if base.host_str().is_none() {
            return Err("AERINI_URL has no host".into());
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("aerini-mcp/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("cannot build the HTTP client: {e}"))?;
        Ok(Self { http, base, token, timeout })
    }

    pub fn sends_token_in_clear(&self) -> bool {
        let host = self.base.host_str().unwrap_or_default().trim_matches(['[', ']']);
        let loopback = host.eq_ignore_ascii_case("localhost")
            || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
        self.base.scheme() == "http" && !loopback
    }

    pub fn origin(&self) -> String {
        self.base.origin().ascii_serialization()
    }

    pub fn url_for(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty().extend(segments);
        }
        url
    }

    pub async fn request(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<Value>,
        if_match: Option<i64>,
        timeout: Option<Duration>,
    ) -> Result<ApiResponse, String> {
        let mut url = self.url_for(segments);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        }
        let mut req = self
            .http
            .request(method, url)
            .bearer_auth(&self.token)
            .timeout(timeout.unwrap_or(self.timeout));
        if let Some(b) = body {
            req = req.json(&b);
        }
        if let Some(version) = if_match {
            req = req.header(reqwest::header::IF_MATCH, format!("\"{version}\""));
        }

        let resp = req.send().await.map_err(|e| self.transport_message(&e, timeout))?;
        let status = resp.status();
        let retry_after = header_text(&resp, RETRY_AFTER.as_str());
        let etag = header_text(&resp, ETAG.as_str());
        let body = resp.bytes().await.map_err(|e| self.transport_message(&e, timeout))?.to_vec();

        if status.is_success() {
            return Ok(ApiResponse { body, etag });
        }
        Err(status_message(status.as_u16(), retry_after.as_deref(), &body))
    }

    fn transport_message(&self, e: &reqwest::Error, timeout: Option<Duration>) -> String {
        let wait = timeout.unwrap_or(self.timeout).as_secs();
        if e.is_timeout() {
            return format!("Timed out after {wait}s waiting for the Aerini server at {}.", self.origin());
        }
        format!("Cannot reach the Aerini server at {}: {}", self.origin(), cause_chain(e))
    }
}

fn cause_chain(e: &reqwest::Error) -> String {
    let mut parts = Vec::new();
    let mut next = std::error::Error::source(e);
    while let Some(cause) = next {
        parts.push(cause.to_string());
        next = cause.source();
    }
    if parts.is_empty() {
        "the request failed before a response arrived".to_string()
    } else {
        parts.join(": ")
    }
}

fn header_text(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers().get(name).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string())
}

fn server_error_text(body: &[u8]) -> (Option<String>, Option<Value>) {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let text = match &parsed {
        Some(v) => v.get("error").and_then(Value::as_str).map(str::to_string),
        None => None,
    }
    .or_else(|| {
        let raw = String::from_utf8_lossy(body);
        let trimmed = raw.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
    .map(|t| clip_text(&sanitize_text(&t), ERROR_TEXT_LIMIT).to_string());
    (text, parsed)
}

fn retry_hint(retry_after: Option<&str>) -> String {
    match retry_after {
        Some(secs) => format!("Retry-After: {secs}s. Wait that long before calling again; aerini-mcp never retries for you."),
        None => "No Retry-After was sent; wait several seconds before calling again. aerini-mcp never retries for you.".to_string(),
    }
}

pub fn status_message(status: u16, retry_after: Option<&str>, body: &[u8]) -> String {
    let (text, parsed) = server_error_text(body);
    let detail: &str = text.as_deref().unwrap_or_default();
    match status {
        401 => "401 Unauthorized: AERINI_TOKEN is wrong, expired or revoked.".to_string(),
        403 => format!(
            "403 Forbidden by the Aerini server: {}. The token's scope or workflow access list does not allow this; ask the server operator instead of retrying.",
            if detail.is_empty() { "no reason given" } else { detail }
        ),
        404 if text.is_none() => format!(
            "404: this Aerini server has no such route. It is probably older than aerini-mcp {}; update aerini-server.",
            env!("CARGO_PKG_VERSION")
        ),
        404 => format!("404 Not found: {detail}"),
        409 => {
            let current = parsed
                .as_ref()
                .and_then(|v| v.get("current_row_version"))
                .map(|v| format!(" (current_row_version={v})"))
                .unwrap_or_default();
            format!("409 Conflict: {detail}{current}. Re-read with get_workflow, merge, then save again with the new if_match_row_version.")
        }
        429 => format!(
            "429 Too Many Requests: {}. {}",
            if detail.is_empty() { "rate limited" } else { detail },
            retry_hint(retry_after)
        ),
        503 => format!(
            "503 Service Unavailable: {}. {}",
            if detail.is_empty() { "server at capacity" } else { detail },
            retry_hint(retry_after)
        ),
        other => format!("{other} from the Aerini server: {}", if detail.is_empty() { "no details" } else { detail }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(base: &str) -> ApiClient {
        ApiClient::new(base, "t".into(), Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn urls_keep_a_proxy_prefix_and_encode_ids() {
        let c = client("https://host.example/aerini/");
        assert_eq!(c.url_for(&["api", "workflows"]).as_str(), "https://host.example/aerini/api/workflows");
        assert_eq!(
            c.url_for(&["api", "workflows", "a/b c?d"]).as_str(),
            "https://host.example/aerini/api/workflows/a%2Fb%20c%3Fd"
        );
    }

    #[test]
    fn bad_base_urls_are_refused_and_clear_text_remote_is_flagged() {
        let token = || "t".to_string();
        let d = Duration::from_secs(1);
        assert!(ApiClient::new("ftp://x", token(), d).is_err());
        assert!(ApiClient::new("http://user:pw@127.0.0.1", token(), d).is_err());
        assert!(ApiClient::new("not a url", token(), d).is_err());

        assert!(!client("http://127.0.0.1:3000").sends_token_in_clear());
        assert!(!client("http://localhost:3000").sends_token_in_clear());
        assert!(!client("https://aerini.example").sends_token_in_clear());
        assert!(client("http://aerini.example").sends_token_in_clear());
    }

    #[test]
    fn status_messages_cover_passthrough_retry_and_missing_route() {
        let forbidden = status_message(403, None, br#"{"error":"token ACL does not permit access to that workflow"}"#);
        assert!(forbidden.starts_with("403 Forbidden"));
        assert!(forbidden.contains("token ACL does not permit"));

        let busy = status_message(429, Some("7"), br#"{"error":"workflow already running"}"#);
        assert!(busy.contains("workflow already running") && busy.contains("Retry-After: 7s"));

        let no_header = status_message(503, None, br#"{"error":"server is at capacity"}"#);
        assert!(no_header.contains("No Retry-After was sent"));

        assert!(status_message(404, None, b"").contains("no such route"));
        assert!(status_message(404, None, br#"{"error":"Not found"}"#).starts_with("404 Not found"));

        let conflict = status_message(409, None, br#"{"error":"modified","current_row_version":9}"#);
        assert!(conflict.contains("current_row_version=9"));
    }

    #[test]
    fn server_text_is_cleaned_and_clipped() {
        let long = format!(r#"{{"error":"{}\u0007"}}"#, "x".repeat(2000));
        let msg = status_message(500, None, long.as_bytes());
        assert!(msg.len() < 600 && !msg.contains('\u{7}'));
    }
}
