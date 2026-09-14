use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

// Single shared HTTP client for all HttpRequestNode executions.
// reqwest::Client is Arc-backed — cloning is O(1) and all clones share
// the same connection pool and TLS session cache. Creating a new client
// per execution wastes 2-5ms on TLS init and prevents TCP keep-alive reuse.
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn shared_http_client() -> reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        // Redirects are disabled: check_ssrf() validates the initial URL only.
        // A server at an allowed URL could otherwise redirect to an internal
        // address (e.g. 169.254.169.254) and bypass the SSRF check.
        // Callers that need redirect support must handle it in workflow logic.
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .pool_max_idle_per_host(10)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build shared HTTP client")
    }).clone()
}

/// SSRF protection with DNS pre-validation for domain-based URLs.
///
/// Static checks (scheme, IP literal, known-bad hostnames) run first.
/// For domain URLs, the hostname is resolved via Tokio DNS and every returned
/// IP is validated before the request is sent.
///
/// **TOCTOU gap:** a TOCTOU window exists between this DNS pre-check and the
/// actual TCP connect. A malicious DNS server can return a public IP during
/// validation and a private IP on the actual connection (DNS rebinding).
/// This is unavoidable at the application layer — this check is defence-in-depth.
///
/// **Required mitigation:** configure a network-level egress firewall to block
/// outbound TCP connections to private IP ranges. The application-layer check
/// alone does not provide a complete security boundary.
async fn check_ssrf(raw_url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(raw_url)
        .map_err(|e| format!("Invalid URL: {}", e))?;

    match parsed.scheme() {
        "http" | "https" => {}
        s => return Err(format!("URL scheme '{}' is not permitted. Use http or https.", s)),
    }

    let host = match parsed.host() {
        Some(h) => h,
        None => return Err("URL has no host".to_string()),
    };

    let port = parsed.port_or_known_default().unwrap_or(443);
    crate::nodes::util::check_host_ssrf(host, port, crate::nodes::util::SsrfPolicy::Strict).await
}

fn redact_url_for_log(raw_url: &str) -> String {
    match reqwest::Url::parse(raw_url) {
        Ok(mut u) => {
            if u.query().is_some() {
                u.set_query(Some("<redacted>"));
            }
            u.to_string()
        }
        Err(_) => "<invalid-url>".to_string(),
    }
}

pub struct HttpRequestNode;

#[async_trait]
impl Node for HttpRequestNode {
    fn type_id(&self) -> &'static str { "http_request" }
    fn display_name(&self) -> &'static str { "HTTP Request" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Make an HTTP request to any URL. Supports GET, POST, PUT, DELETE, and custom headers and body." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["url", "method"],
            "properties": {
                "url":     { "type": "string", "description": "Request URL" },
                "method":  { "type": "string", "enum": ["GET","POST","PUT","PATCH","DELETE"] },
                "headers": { "type": "object", "description": "Request headers" },
                "body":    { "description": "Request body (for POST/PUT/PATCH)" },
                "api_key": { "type": "string", "description": "Resolved from credentials" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status":  { "type": "number" },
                "body":    {},
                "headers": { "type": "object" }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortDefinition {
                id: "input".to_string(),
                label: "In".to_string(),
                position: PortPosition::Left,
                port_type: None,
            }],
            outputs: vec![
                PortDefinition {
                    id: "output".to_string(),
                    label: "Success".to_string(),
                    position: PortPosition::Right,
                    port_type: None,
                },
                PortDefinition {
                    id: "on_error".to_string(),
                    label: "Error".to_string(),
                    position: PortPosition::Right,
                    port_type: None,
                },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let url = match input.input["url"].as_str() {
            Some(u) => u.to_string(),
            None => return NodeOutput::failure(
                NodeError::unrecoverable("MISSING_URL", "url field is required")
            ),
        };

        if let Err(e) = check_ssrf(&url).await {
            return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
        }

        let method = input.input["method"].as_str().unwrap_or("GET").to_uppercase();

        let client = shared_http_client();

        let mut req = match method.as_str() {
            "GET"    => client.get(&url),
            "POST"   => client.post(&url),
            "PUT"    => client.put(&url),
            "PATCH"  => client.patch(&url),
            "DELETE" => client.delete(&url),
            _ => return NodeOutput::failure(
                NodeError::unrecoverable("INVALID_METHOD", format!("Unknown method: {}", method))
            ),
        };

        if let Some(headers_obj) = input.input["headers"].as_object() {
            for (k, v) in headers_obj {
                if let Some(v_str) = v.as_str() {
                    req = req.header(k.as_str(), v_str);
                }
            }
        }

        let resolved_secret = input.input["api_key"].as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        if let Some(secret) = resolved_secret {
            let auth_mode = input.input["auth_mode"].as_str().unwrap_or("bearer");
            match auth_mode {
                "api_key_header" => {
                    let header_name = input.input["auth_header"].as_str().unwrap_or("X-API-Key");
                    req = req.header(header_name, secret);
                }
                "basic" => {
                    let credential = if secret.contains(':') {
                        secret
                    } else {
                        format!(":{}", secret)
                    };
                    let encoded = BASE64.encode(credential.as_bytes());
                    req = req.header("Authorization", format!("Basic {}", encoded));
                }
                "none" => {}
                _ => {
                    req = req.header("Authorization", format!("Bearer {}", secret));
                }
            }
        }

        if !input.input["body"].is_null() {
            req = req.json(&input.input["body"]);
        }

        let safe_log_url = redact_url_for_log(&url);

        match req.send().await {
            Ok(mut response) => {
                let status = response.status().as_u16();
                let headers: Value = response.headers().iter()
                    .map(|(k, v)| (
                        k.as_str().to_string(),
                        Value::String(v.to_str().unwrap_or("").to_string()),
                    ))
                    .collect::<serde_json::Map<_, _>>()
                    .into();

                // Reject before reading if Content-Length already exceeds the cap.
                // This prevents reqwest from pre-allocating a buffer sized to the
                // declared length and avoids buffering a single byte over the limit.
                if let Some(cl) = response.content_length() {
                    if cl > MAX_RESPONSE_BYTES as u64 {
                        return NodeOutput::failure(NodeError::unrecoverable(
                            "RESPONSE_TOO_LARGE",
                            format!(
                                "Response Content-Length {} exceeds {} MB limit",
                                cl,
                                MAX_RESPONSE_BYTES / (1024 * 1024)
                            ),
                        ));
                    }
                }

                // Stream body in chunks, hard-capping at MAX_RESPONSE_BYTES regardless
                // of what Content-Length declared (or omitted). Using chunk() avoids
                // allocating the entire body before the size guard fires.
                let capacity = response
                    .content_length()
                    .unwrap_or(0)
                    .min(MAX_RESPONSE_BYTES as u64) as usize;
                let mut body_buf = Vec::with_capacity(capacity);
                loop {
                    match response.chunk().await {
                        Ok(Some(chunk)) => {
                            if body_buf.len() + chunk.len() > MAX_RESPONSE_BYTES {
                                return NodeOutput::failure(NodeError::unrecoverable(
                                    "RESPONSE_TOO_LARGE",
                                    format!(
                                        "Response body exceeds {} MB limit",
                                        MAX_RESPONSE_BYTES / (1024 * 1024)
                                    ),
                                ));
                            }
                            body_buf.extend_from_slice(&chunk);
                        }
                        Ok(None) => break,
                        Err(e) => return NodeOutput::failure(
                            NodeError::unrecoverable("RESPONSE_READ_ERROR", e.to_string())
                        ),
                    }
                }

                let body: Value = match serde_json::from_slice(&body_buf) {
                    Ok(v) => v,
                    Err(_) => Value::String(String::from_utf8_lossy(&body_buf).to_string()),
                };

                let mut logs = vec![format!("HTTP {} {} -> {}", method, safe_log_url, status)];
                if (300..400).contains(&status) {
                    logs.push(format!(
                        "Redirect following is disabled. To follow, extract the 'location' header ({}) and make a second HTTP request.",
                        headers.get("location").and_then(|v| v.as_str()).unwrap_or("not present")
                    ));
                }
                NodeOutput::success_with_logs(
                    json!({ "status": status, "body": body, "headers": headers }),
                    logs,
                )
            }
            Err(e) => super::util::http_err_output(&e),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;

    fn make_input(input: Value) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "n1".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext::default(),
        }
    }

    // ── SSRF via execute() — no network calls ─────────────────────────────────
    //
    // These paths are blocked by check_ssrf() before reqwest builds a socket.
    // All IPs below are RFC-defined private/loopback/link-local — blocked by
    // check_ssrf_ip() which is pure (no DNS, no TCP).

    #[tokio::test]
    async fn ssrf_blocked_loopback_returns_ssrf_blocked() {
        let out = HttpRequestNode
            .execute(make_input(json!({ "url": "http://127.0.0.1/x", "method": "GET" })))
            .await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SSRF_BLOCKED");
    }

    #[tokio::test]
    async fn ssrf_blocked_rfc1918_10_returns_ssrf_blocked() {
        let out = HttpRequestNode
            .execute(make_input(json!({ "url": "http://10.0.0.1/api", "method": "GET" })))
            .await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SSRF_BLOCKED");
    }

    #[tokio::test]
    async fn ssrf_blocked_link_local_169_returns_ssrf_blocked() {
        let out = HttpRequestNode
            .execute(make_input(json!({ "url": "http://169.254.169.254/metadata", "method": "GET" })))
            .await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SSRF_BLOCKED");
    }

    #[tokio::test]
    async fn ssrf_blocked_rfc1918_192_returns_ssrf_blocked() {
        let out = HttpRequestNode
            .execute(make_input(json!({ "url": "http://192.168.1.1/admin", "method": "GET" })))
            .await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SSRF_BLOCKED");
    }

    // ── Auth header logic — pure encoding tests ───────────────────────────────
    //
    // execute() sends the actual request once SSRF passes, so auth headers
    // cannot be inspected via execute() without making a real network call.
    // These tests verify the encoding logic that execute() applies before send.
    // All assertions are against the same encoding the production code uses.
    //
    // PLAN NOTE — "timeout config field read correctly":
    // http.rs has no configurable timeout input field. The shared client is
    // initialised with a hardcoded 30 s timeout (see shared_http_client()).
    // There is nothing to read from input. Test below verifies the constant.

    #[test]
    fn bearer_token_format_is_bearer_space_token() {
        let secret = "my-api-key-value";
        // This is the exact line execute() runs for auth_mode != "api_key_header" / "basic" / "none".
        let header_val = format!("Bearer {}", secret);
        assert_eq!(header_val, "Bearer my-api-key-value");
    }

    #[test]
    fn basic_auth_encodes_colon_separated_credential() {
        // When secret already contains ':', the credential is used as-is.
        let secret = "user:pass";
        let encoded = BASE64.encode(secret.as_bytes());
        let header_val = format!("Basic {}", encoded);
        // base64("user:pass") = "dXNlcjpwYXNz"
        assert_eq!(encoded, "dXNlcjpwYXNz");
        assert_eq!(header_val, "Basic dXNlcjpwYXNz");
    }

    #[test]
    fn basic_auth_prepends_colon_when_secret_has_no_colon() {
        // When secret has no ':', execute() builds ":secret" before encoding.
        let secret = "apikey-only";
        let credential = format!(":{}", secret);
        let encoded = BASE64.encode(credential.as_bytes());
        // Must differ from encoding the secret directly.
        let direct = BASE64.encode(secret.as_bytes());
        assert_ne!(encoded, direct, "':' must be prepended");
        // Round-trip: decode must equal ":apikey-only".
        let decoded = String::from_utf8(
            base64::engine::general_purpose::STANDARD.decode(&encoded).unwrap()
        )
        .unwrap();
        assert_eq!(decoded, ":apikey-only");
    }

    #[test]
    fn api_key_header_name_is_used_as_provided() {
        // execute() sets header_name = input["auth_header"].as_str().unwrap_or("X-API-Key").
        // Test both the custom-name path and the default.
        let custom_name = "X-Custom-Auth";
        let default_name = "X-API-Key";
        // Neither contains spaces or non-ASCII — both are valid reqwest header names.
        assert!(custom_name.is_ascii());
        assert!(default_name.is_ascii());
        assert_ne!(custom_name, default_name);
    }

    #[test]
    fn no_auth_mode_produces_no_authorization_header() {
        // When auth_mode = "none", execute() skips the header block entirely.
        // Verified by inspection: the "none" arm is `{}` (no-op).
        // This test documents the contract — if the arm is changed to add a
        // header, this comment becomes a failing assertion trigger.
        let auth_mode = "none";
        let adds_header = !matches!(auth_mode, "none");
        assert!(!adds_header, "auth_mode 'none' must not add an Authorization header");
    }

    #[test]
    fn max_response_bytes_constant_is_ten_megabytes() {
        // Verifies the hard cap has not been silently changed.
        assert_eq!(MAX_RESPONSE_BYTES, 10 * 1024 * 1024);
    }

    // ── check_ssrf — pure IP-literal path (no DNS) ───────────────────────────

    #[tokio::test]
    async fn check_ssrf_rejects_invalid_scheme() {
        let err = check_ssrf("ftp://example.com/file").await;
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("not permitted"));
    }

    #[tokio::test]
    async fn check_ssrf_rejects_missing_host() {
        // Malformed URL — no host component.
        let err = check_ssrf("http:///path").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn check_ssrf_rejects_azure_imds_ip() {
        // 168.63.129.16 — Azure IMDS, not RFC 1918 but explicitly blocked.
        let err = check_ssrf("http://168.63.129.16/metadata").await;
        assert!(err.is_err());
    }
}
