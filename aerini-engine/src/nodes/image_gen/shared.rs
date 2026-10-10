use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::util::{
    after_submit, check_host_ssrf_from_url, is_retryable_network_error, Replay, SsrfPolicy,
};

pub(super) fn network_err(e: reqwest::Error) -> NodeError {
    let recoverable = crate::nodes::util::is_retryable_network_error(Replay::Never, &e);
    if recoverable {
        NodeError::recoverable("NETWORK_ERROR", crate::nodes::util::reqwest_err_msg(&e))
    } else {
        NodeError::unrecoverable("NETWORK_ERROR", crate::nodes::util::reqwest_err_msg(&e))
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
/// image, so a provider URL pointing at an oversized file cannot exhaust
/// process memory. Same 10 MB value as http.rs, s3.rs and util.rs.
const MAX_IMAGE_DOWNLOAD_BYTES: usize = 10 * 1024 * 1024;

/// Why a body read failed. Only `Transport` is worth a second download: a
/// body over the cap is over the cap every time.
#[derive(Debug)]
pub(super) enum BodyReadError {
    TooLarge(String),
    Transport(String),
}

impl BodyReadError {
    pub(super) fn message(&self) -> &str {
        match self {
            BodyReadError::TooLarge(m) | BodyReadError::Transport(m) => m,
        }
    }
}

/// Reads a response body into memory, hard-capping at
/// `MAX_IMAGE_DOWNLOAD_BYTES` regardless of what (or whether)
/// `Content-Length` declares: an oversized declared length is rejected up
/// front, and the stream is counted chunk by chunk either way. SSRF validation
/// is the caller's job; this only guards the read.
pub(super) async fn read_bytes_response_capped(mut response: reqwest::Response) -> Result<Vec<u8>, BodyReadError> {
    if let Some(cl) = response.content_length() {
        if cl > MAX_IMAGE_DOWNLOAD_BYTES as u64 {
            return Err(BodyReadError::TooLarge(format!(
                "Response Content-Length {} exceeds {} MB limit",
                cl,
                MAX_IMAGE_DOWNLOAD_BYTES / (1024 * 1024)
            )));
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
                    return Err(BodyReadError::TooLarge(format!(
                        "Response body exceeds {} MB limit",
                        MAX_IMAGE_DOWNLOAD_BYTES / (1024 * 1024)
                    )));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => return Err(BodyReadError::Transport(crate::nodes::util::reqwest_err_msg(&e))),
        }
    }

    Ok(bytes)
}

/// True for a `Content-Type` that is certainly not an image: an error page or
/// API reply served with a 2xx status. Anything else, including a missing
/// header or `application/octet-stream`, is accepted because CDNs and local
/// servers label image bytes loosely.
fn is_non_image_content_type(content_type: &str) -> bool {
    let essence = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    essence.starts_with("text/")
        || matches!(essence.as_str(), "application/json" | "application/xml" | "application/xhtml+xml")
}

const DOWNLOAD_ATTEMPTS: u32 = 3;
pub(super) const DOWNLOAD_BACKOFF_MS: u64 = 500;

/// Statuses worth another download attempt. Every other non-2xx reply is final.
pub(super) fn download_status_retryable(status: u16) -> bool {
    matches!(status, 408 | 429 | 500..=599)
}

/// GETs an image with up to `DOWNLOAD_ATTEMPTS` tries (the wait doubles from
/// `backoff_ms`). Only a send error that is a timeout or connect failure, a
/// body that breaks off mid-stream, and the statuses in
/// `download_status_retryable` are tried again.
///
/// A non-2xx reply, a text or JSON `Content-Type`, or an empty body is an
/// error, never image bytes. A body that breaks off mid-stream is fetched
/// again like a failed send. Every failure is unrecoverable: the caller downloads after a paid or queued job
/// exists, so a node-level retry would submit a second one.
pub(super) async fn fetch_image_bytes(
    make_request: impl Fn() -> reqwest::RequestBuilder,
    backoff_ms: u64,
) -> Result<Vec<u8>, NodeError> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let last = attempt >= DOWNLOAD_ATTEMPTS;
        let pause = std::time::Duration::from_millis(backoff_ms << (attempt - 1));

        let response = match make_request().send().await {
            Ok(r) => r,
            Err(e) => {
                if !last && is_retryable_network_error(Replay::Safe, &e) {
                    tokio::time::sleep(pause).await;
                    continue;
                }
                return Err(after_submit(network_err(e)));
            }
        };

        let status = response.status();
        if !status.is_success() {
            if !last && download_status_retryable(status.as_u16()) {
                tokio::time::sleep(pause).await;
                continue;
            }
            return Err(NodeError::unrecoverable(
                "DOWNLOAD_ERROR",
                format!("Image download failed: HTTP {}", status.as_u16()),
            ));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if is_non_image_content_type(&content_type) {
            return Err(NodeError::unrecoverable(
                "DOWNLOAD_ERROR",
                format!("Image download returned {}, not an image", content_type),
            ));
        }

        let bytes = match read_bytes_response_capped(response).await {
            Ok(b) => b,
            Err(BodyReadError::Transport(_)) if !last => {
                tokio::time::sleep(pause).await;
                continue;
            }
            Err(e) => return Err(NodeError::unrecoverable("DOWNLOAD_ERROR", e.message())),
        };
        if bytes.is_empty() {
            return Err(NodeError::unrecoverable("DOWNLOAD_ERROR", "Image download returned an empty body"));
        }
        return Ok(bytes);
    }
}

/// Error codes after which another image in the same `n` loop would only add
/// spend on a path that has just failed: a bad key or account, a blocked
/// prompt, a job that may still be running (timeout, poll failure) or a
/// connection that is down. Failures that are specific to one image (a failed
/// job, a rate limit, a bad download, a safety block on one result) let the
/// loop go on.
pub(super) fn stops_the_loop(code: &str) -> bool {
    matches!(
        code,
        "INVALID_API_KEY"
            | "INSUFFICIENT_CREDITS"
            | "INSUFFICIENT_QUOTA"
            | "CONTENT_POLICY_VIOLATION"
            | "PROMPT_BLOCKED"
            | "TIMEOUT"
            | "POLL_FAILED"
            | "POLL_REJECTED"
            | "TASK_NOT_FOUND"
            | "SSRF_BLOCKED"
            | "NETWORK_ERROR"
            | "REGION_NOT_SUPPORTED"
            | "PROJECT_DENIED"
            | "API_KEY_RESTRICTED"
            | "API_KEY_LEAKED"
            | "PERMISSION_DENIED"
    )
}

/// Collects the outcome of an `n`-image loop: logs, the files made so far and
/// the failures, and builds the node result once the loop ends.
pub(super) struct ImageRun {
    label: String,
    n: usize,
    files: Vec<Value>,
    logs: Vec<String>,
    failures: Vec<String>,
    last_error: Option<NodeError>,
}

impl ImageRun {
    pub(super) fn new(label: impl Into<String>, n: usize) -> Self {
        Self { label: label.into(), n, files: Vec::new(), logs: Vec::new(), failures: Vec::new(), last_error: None }
    }

    pub(super) fn generated(&mut self, index: usize, file: Value) {
        self.logs.push(format!("{} {}/{} generated", self.label, index + 1, self.n));
        self.files.push(file);
    }

    /// Records a failed image and returns true when the loop must stop.
    pub(super) fn failed(&mut self, index: usize, e: NodeError) -> bool {
        let msg = format!("{} {}/{} failed: [{}] {}", self.label, index + 1, self.n, e.code, e.message);
        self.logs.push(msg.clone());
        self.failures.push(msg);
        let stop = stops_the_loop(&e.code);
        if stop && index + 1 < self.n {
            self.logs.push(format!(
                "Stopped after image {} of {}: [{}] makes further images pointless. The remaining {} were not requested.",
                index + 1, self.n, e.code, self.n - index - 1
            ));
        }
        self.last_error = Some(e);
        stop
    }

    pub(super) fn finish(self, source: &str) -> NodeOutput {
        let Self { n, files, mut logs, failures, last_error, .. } = self;
        if files.is_empty() {
            let err = last_error.unwrap_or_else(|| {
                NodeError::unrecoverable("ALL_IMAGES_FAILED", failures.join("; "))
            });
            return NodeOutput::failure_with_logs(err, logs);
        }
        if !failures.is_empty() {
            logs.push(format!(
                "Partial success: {}/{} images generated. {} failed.",
                files.len(), n, failures.len()
            ));
        }
        NodeOutput::success_with_logs(
            json!({ "files": files, "count": files.len(), "source": source }),
            logs,
        )
    }
}

/// Download image from URL -> raw base64 (no data: URI prefix).
/// Validates host against SSRF before fetch.
pub(super) async fn download_to_base64(client: &reqwest::Client, url: &str) -> Result<String, NodeError> {
    // Validates URL host is not RFC-1918/loopback -- a spoofed API could return
    // an internal address (e.g. AWS metadata) to exfiltrate infrastructure data.
    check_host_ssrf_from_url(url, SsrfPolicy::Strict).await.map_err(|e| {
        NodeError::unrecoverable("SSRF_BLOCKED", e)
    })?;
    let bytes = fetch_image_bytes(|| client.get(url), DOWNLOAD_BACKOFF_MS).await?;
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
        assert!(matches!(err, BodyReadError::TooLarge(_)) && err.message().contains("Content-Length"), "unexpected error: {err:?}");
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
        assert!(matches!(err, BodyReadError::TooLarge(_)) && err.message().contains("exceeds"), "unexpected error: {err:?}");
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

    // ── fetch_image_bytes ─────────────────────────────────────────
    struct Reply {
        status: u16,
        content_type: &'static str,
        body: &'static str,
        declared_len: Option<usize>,
    }

    fn reply(status: u16, body: &'static str) -> Reply {
        Reply { status, content_type: "image/png", body, declared_len: None }
    }

    async fn spawn_image_mock(replies: Vec<Reply>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("mock server bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            for r in replies {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                let mut discard = [0u8; 2048];
                let _ = stream.read(&mut discard).await;
                let raw = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    r.status, r.content_type, r.declared_len.unwrap_or(r.body.len()), r.body
                );
                let _ = stream.write_all(raw.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    async fn fetch(replies: Vec<Reply>) -> Result<Vec<u8>, NodeError> {
        let url = spawn_image_mock(replies).await;
        let client = reqwest::Client::new();
        fetch_image_bytes(|| client.get(&url), 1).await
    }

    #[tokio::test]
    async fn a_4xx_download_fails_at_once_and_never_returns_the_error_body_as_an_image() {
        // The 200 queued behind the 403 must not be reached: no retry on a 4xx.
        let e = fetch(vec![reply(403, "<html>expired</html>"), reply(200, "IMG")]).await.expect_err("must fail");
        assert_eq!(e.code, "DOWNLOAD_ERROR");
        assert!(!e.recoverable);
        assert!(e.message.contains("403"), "{}", e.message);
    }

    #[tokio::test]
    async fn a_transient_5xx_is_retried_and_the_image_is_returned() {
        let bytes = fetch(vec![reply(503, "x"), reply(429, "x"), reply(200, "IMG")]).await.expect("third try succeeds");
        assert_eq!(bytes, b"IMG");
    }

    #[tokio::test]
    async fn a_download_gives_up_after_three_attempts() {
        // The fourth reply is a 200 that a fourth attempt would return.
        let e = fetch(vec![reply(503, "x"), reply(503, "x"), reply(503, "x"), reply(200, "IMG")]).await.expect_err("must stop at 3");
        assert_eq!(e.code, "DOWNLOAD_ERROR");
        assert!(!e.recoverable);
        assert!(e.message.contains("503"), "{}", e.message);
    }

    #[tokio::test]
    async fn an_empty_2xx_body_is_an_error() {
        let e = fetch(vec![reply(200, "")]).await.expect_err("empty body is not an image");
        assert_eq!(e.code, "DOWNLOAD_ERROR");
    }

    #[tokio::test]
    async fn a_network_failure_is_unrecoverable_so_the_job_is_not_resubmitted() {
        let e = fetch(vec![]).await.expect_err("nothing is listening");
        assert_eq!(e.code, "NETWORK_ERROR");
        assert!(!e.recoverable);
    }

    #[tokio::test]
    async fn a_2xx_error_page_is_rejected_but_an_octet_stream_is_accepted() {
        let html = Reply { content_type: "text/html; charset=utf-8", ..reply(200, "<html>sign in</html>") };
        let e = fetch(vec![html]).await.expect_err("html is not an image");
        assert_eq!(e.code, "DOWNLOAD_ERROR");
        assert!(!e.recoverable);
        assert!(e.message.contains("text/html"), "{}", e.message);

        let blob = Reply { content_type: "application/octet-stream", ..reply(200, "IMG") };
        assert_eq!(fetch(vec![blob]).await.expect("octet-stream is accepted"), b"IMG");
    }

    #[tokio::test]
    async fn a_body_cut_off_mid_stream_is_fetched_again() {
        let cut = Reply { declared_len: Some(100), ..reply(200, "IM") };
        let bytes = fetch(vec![cut, reply(200, "IMG")]).await.expect("second try succeeds");
        assert_eq!(bytes, b"IMG");
    }

    #[test]
    fn json_xml_and_text_types_are_not_images() {
        for ct in ["application/json", "Application/JSON; charset=utf-8", "text/plain", "application/xml", "application/xhtml+xml"] {
            assert!(is_non_image_content_type(ct), "{ct}");
        }
        for ct in ["image/png", "image/webp", "application/octet-stream", "binary/octet-stream", ""] {
            assert!(!is_non_image_content_type(ct), "{ct}");
        }
    }

    #[test]
    fn the_loop_stops_for_account_and_dead_path_errors_but_not_for_one_bad_image() {
        for code in ["INVALID_API_KEY", "INSUFFICIENT_CREDITS", "INSUFFICIENT_QUOTA", "CONTENT_POLICY_VIOLATION", "PROMPT_BLOCKED", "TIMEOUT", "POLL_FAILED", "NETWORK_ERROR", "REGION_NOT_SUPPORTED", "API_KEY_LEAKED"] {
            assert!(stops_the_loop(code), "{code}");
        }
        for code in ["GENERATION_FAILED", "RATE_LIMITED", "DOWNLOAD_ERROR", "EMPTY_RESPONSE", "NO_IMAGE_RETURNED", "IMAGE_BLOCKED"] {
            assert!(!stops_the_loop(code), "{code}");
        }
    }

    #[test]
    fn an_image_run_reports_partial_success_and_says_why_it_stopped() {
        let mut run = ImageRun::new("X image", 3);
        run.generated(0, json!({"filename": "a"}));
        assert!(!run.failed(1, NodeError::unrecoverable("GENERATION_FAILED", "m")));
        let out = run.finish("src");
        assert_eq!(out.output.as_ref().unwrap()["count"], 1);
        assert!(out.logs.iter().any(|l| l.starts_with("Partial success: 1/3")));

        let mut run = ImageRun::new("X image", 3);
        assert!(run.failed(0, NodeError::unrecoverable("TIMEOUT", "slow")));
        let out = run.finish("src");
        assert_eq!(out.error.as_ref().unwrap().code, "TIMEOUT");
        assert!(out.logs.iter().any(|l| l.contains("Stopped after image 1 of 3") && l.contains("remaining 2")), "{:?}", out.logs);
    }

    #[test]
    fn download_status_table_retries_only_408_429_and_5xx() {
        for s in [408u16, 429, 500, 502, 503, 504, 599] {
            assert!(download_status_retryable(s), "{s}");
        }
        for s in [200u16, 301, 400, 401, 403, 404, 410, 422] {
            assert!(!download_status_retryable(s), "{s}");
        }
    }
}
