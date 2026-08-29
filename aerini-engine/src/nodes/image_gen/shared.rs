use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use crate::error::NodeError;
use crate::nodes::util::{check_host_ssrf_from_url, SsrfPolicy};

pub(super) fn network_err(e: reqwest::Error) -> NodeError {
    let recoverable = e.is_timeout() || e.is_connect();
    if recoverable {
        NodeError::recoverable("NETWORK_ERROR", e.to_string())
    } else {
        NodeError::unrecoverable("NETWORK_ERROR", e.to_string())
    }
}

/// Strip a "data:<mime>;base64," prefix if present.
/// DATA CONTRACT requires raw base64 without any data: URI prefix.
pub(super) fn strip_data_uri_prefix(s: &str) -> &str {
    if let Some(comma_pos) = s.find(',') {
        if s[..comma_pos].starts_with("data:") {
            return &s[comma_pos + 1..];
        }
    }
    s
}

pub(super) fn mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp"               => "webp",
        "image/gif"                => "gif",
        _                          => "png",
    }
}

/// Hard cap on bytes read into memory before base64-encoding a downloaded
/// image (pattern: unbounded HTTP response buffering). Mirrors
/// http.rs's MAX_RESPONSE_BYTES / s3.rs's MAX_DOWNLOAD_BYTES / util.rs's
/// MAX_JSON_RESPONSE_BYTES (same 10 MB value) -- a malicious or compromised
/// image-gen provider returning a URL to an oversized file must not be able
/// to exhaust process memory via a single download. Kept local rather than
/// reusing util.rs's helper: that one returns parsed JSON, this needs raw
/// bytes.
const MAX_IMAGE_DOWNLOAD_BYTES: usize = 10 * 1024 * 1024;

/// Reads a response body into memory, hard-capping at
/// `MAX_IMAGE_DOWNLOAD_BYTES` regardless of what (or whether)
/// `Content-Length` declares. Same two-stage guard as util.rs's
/// `read_json_response_capped`: (1) reject upfront on an oversized declared
/// `Content-Length`, (2) stream chunk-by-chunk regardless, so a missing or
/// lying `Content-Length` can't bypass the cap. SSRF validation is the
/// caller's responsibility (done in `download_to_base64` before the request
/// is sent) -- this function only guards the read itself.
pub(super) async fn read_bytes_response_capped(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    if let Some(cl) = response.content_length() {
        if cl > MAX_IMAGE_DOWNLOAD_BYTES as u64 {
            return Err(format!(
                "Response Content-Length {} exceeds {} MB limit",
                cl,
                MAX_IMAGE_DOWNLOAD_BYTES / (1024 * 1024)
            ));
        }
    }

    let capacity = response
        .content_length()
        .unwrap_or(0)
        .min(MAX_IMAGE_DOWNLOAD_BYTES as u64) as usize;
    let mut bytes: Vec<u8> = Vec::with_capacity(capacity);

    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len() + chunk.len() > MAX_IMAGE_DOWNLOAD_BYTES {
                    return Err(format!(
                        "Response body exceeds {} MB limit",
                        MAX_IMAGE_DOWNLOAD_BYTES / (1024 * 1024)
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => return Err(e.to_string()),
        }
    }

    Ok(bytes)
}

/// Download image from URL -> raw base64 (no data: URI prefix).
/// Validates host against SSRF before fetch.
pub(super) async fn download_to_base64(client: &reqwest::Client, url: &str) -> Result<String, NodeError> {
    // Validates URL host is not RFC-1918/loopback -- a spoofed API could return
    // an internal address (e.g. AWS metadata) to exfiltrate infrastructure data.
    check_host_ssrf_from_url(url, SsrfPolicy::Strict).await.map_err(|e| {
        NodeError::unrecoverable("SSRF_BLOCKED", e)
    })?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(network_err)?;
    let bytes = read_bytes_response_capped(response)
        .await
        .map_err(|e| NodeError::unrecoverable("DOWNLOAD_ERROR", e))?;
    Ok(BASE64.encode(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── mime_to_ext ───────────────────────────────────────────────────────────

    #[test]
    fn jpeg_maps_to_jpg() { assert_eq!(mime_to_ext("image/jpeg"), "jpg"); }

    #[test]
    fn jpg_alias_maps_to_jpg() { assert_eq!(mime_to_ext("image/jpg"), "jpg"); }

    #[test]
    fn webp_maps_to_webp() { assert_eq!(mime_to_ext("image/webp"), "webp"); }

    #[test]
    fn gif_maps_to_gif() { assert_eq!(mime_to_ext("image/gif"), "gif"); }

    #[test]
    fn png_maps_to_png() { assert_eq!(mime_to_ext("image/png"), "png"); }

    #[test]
    fn unknown_mime_falls_back_to_png() { assert_eq!(mime_to_ext("image/avif"), "png"); }

    #[test]
    fn empty_mime_falls_back_to_png() { assert_eq!(mime_to_ext(""), "png"); }

    // ── strip_data_uri_prefix ─────────────────────────────────────────────────

    #[test]
    fn strips_data_uri_prefix() {
        assert_eq!(strip_data_uri_prefix("data:image/png;base64,ABC"), "ABC");
    }

    #[test]
    fn no_prefix_unchanged() {
        assert_eq!(strip_data_uri_prefix("ABC"), "ABC");
    }

    #[test]
    fn empty_string_unchanged() {
        assert_eq!(strip_data_uri_prefix(""), "");
    }

    #[test]
    fn comma_without_data_prefix_unchanged() {
        // comma present but prefix is not "data:" — must NOT strip
        assert_eq!(
            strip_data_uri_prefix("not:image/png;base64,ABC"),
            "not:image/png;base64,ABC",
        );
    }

    #[test]
    fn strips_jpeg_data_uri() {
        assert_eq!(strip_data_uri_prefix("data:image/jpeg;base64,/9j/xyz"), "/9j/xyz");
    }

    // ── read_bytes_response_capped ─────────────────────────────────
    // download_to_base64 itself can't be unit-tested here: check_host_ssrf_from_url
    // rejects a loopback mock URL before the download logic ever runs. Testing
    // the capped-read helper directly (same split as util.rs's own
    // read_json_response_capped, which likewise doesn't do its own SSRF check)
    // covers unbounded buffering during the read without needing to
    // fight the SSRF guard. Same raw-mock idiom as
    // util.rs::read_json_response_capped_tests / ai_prompt/openai.rs's
    // backward_compat_tests.

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
            let mut discard = [0u8; 1024];
            let _ = stream.read(&mut discard).await;
            let _ = stream.write_all(&raw_response).await;
            let _ = stream.shutdown().await;
        });

        format!("http://127.0.0.1:{}/", port)
    }

    #[tokio::test]
    async fn small_body_reads_correctly() {
        let body = b"not-really-a-png-but-thats-fine-for-this-test";
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut raw = raw.into_bytes();
        raw.extend_from_slice(body);
        let url = spawn_raw_mock(raw).await;
        let resp = reqwest::Client::new().get(&url).send().await.unwrap();
        let bytes = read_bytes_response_capped(resp).await.expect("should read");
        assert_eq!(bytes, body);
    }

    #[tokio::test]
    async fn oversized_content_length_rejected_before_read() {
        // Declared body far exceeds the cap; actual body is tiny -- if the
        // function read anyway before checking, this would wrongly succeed.
        let declared = MAX_IMAGE_DOWNLOAD_BYTES as u64 + 1;
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n{{}}",
            declared
        );
        let url = spawn_raw_mock(raw.into_bytes()).await;
        let resp = reqwest::Client::new().get(&url).send().await.unwrap();
        let err = read_bytes_response_capped(resp).await.expect_err("must reject");
        assert!(err.contains("Content-Length"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn oversized_body_without_content_length_rejected_during_stream() {
        // No Content-Length header -- cap must still be enforced mid-stream,
        // not skipped because there was nothing to pre-check.
        let oversized_body = vec![b'x'; MAX_IMAGE_DOWNLOAD_BYTES + 1024];
        let mut raw = b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nConnection: close\r\n\r\n".to_vec();
        raw.extend_from_slice(&oversized_body);
        let url = spawn_raw_mock(raw).await;
        let resp = reqwest::Client::new().get(&url).send().await.unwrap();
        let err = read_bytes_response_capped(resp).await.expect_err("must reject");
        assert!(err.contains("exceeds"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn body_at_exactly_the_cap_succeeds() {
        let body = vec![b'y'; MAX_IMAGE_DOWNLOAD_BYTES];
        let raw_head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut raw = raw_head.into_bytes();
        raw.extend_from_slice(&body);
        let url = spawn_raw_mock(raw).await;
        let resp = reqwest::Client::new().get(&url).send().await.unwrap();
        let bytes = read_bytes_response_capped(resp).await.expect("cap itself must still be accepted");
        assert_eq!(bytes.len(), MAX_IMAGE_DOWNLOAD_BYTES);
    }
}
