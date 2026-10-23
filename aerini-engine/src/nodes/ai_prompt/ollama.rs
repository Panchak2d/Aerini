//! Ollama-only model management for the editor: probe, pull and delete.
//!
//! Aerini's Base URL is the OpenAI-compatible `/v1` path; these calls use
//! Ollama's native `/api/*` endpoints on the same host. Nothing here runs at
//! workflow run time, and every call goes through the same `AllowLocal` SSRF
//! gate as `list_models`.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::error::NodeError;
use crate::nodes::util::{
    check_host_ssrf_from_url, guarded_client_builder, read_json_response_capped, reqwest_err_msg,
    SsrfPolicy,
};
use crate::provider::ProviderRegistry;

use super::extract_provider_error;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const DELETE_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest silence tolerated while pulling. A pull has no total time limit,
/// because a large model can legitimately take hours.
const PULL_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_MODEL_NAME_LEN: usize = 200;
const MAX_SEGMENT_LEN: usize = 64;
const MAX_REMOTE_TEXT_CHARS: usize = 300;

/// What a probe learned about the configured server.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OllamaProbe {
    pub ollama: bool,
    pub version: Option<String>,
    /// True only for `localhost` and loopback addresses; the UI asks for
    /// confirmation before changing a server that is not on this machine.
    pub loopback: bool,
    /// Why `ollama` is false: `not_local`, `no_base_url`, `invalid_url`,
    /// `blocked`, `unreachable` or `not_ollama`.
    pub reason: Option<&'static str>,
}

impl OllamaProbe {
    fn no(reason: &'static str, loopback: bool) -> Self {
        Self { ollama: false, version: None, loopback, reason: Some(reason) }
    }
}

/// One progress step of a pull, summed over every layer announced so far.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PullProgress {
    /// `manifest`, `downloading`, `verifying`, `finalizing` or `working`.
    pub phase: &'static str,
    pub completed: u64,
    pub total: u64,
}

/// The server's native root: Base URL without its `/v1` suffix.
pub fn native_root(base_url: &str) -> Result<String, NodeError> {
    let bad = |m: &str| NodeError::unrecoverable("INVALID_BASE_URL", m.to_string());
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err(bad("Enter a Base URL first."));
    }
    let parsed = url::Url::parse(trimmed).map_err(|_| bad("Base URL is not a valid URL."))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(bad("Base URL must start with http:// or https://."));
    }
    if parsed.host().is_none() {
        return Err(bad("Base URL has no host."));
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(bad("Base URL must not contain credentials, a query or a fragment."));
    }
    let path = parsed.path().trim_end_matches('/');
    let path = path.strip_suffix("/v1").unwrap_or(path);
    let mut root = parsed.clone();
    root.set_path(path);
    Ok(root.as_str().trim_end_matches('/').to_string())
}

fn is_name_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_SEGMENT_LEN
        && s.starts_with(|c: char| c.is_ascii_alphanumeric())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Accepts `name[:tag]` and `namespace/name[:tag]`. Names that carry a
/// registry host (`hf.co/org/repo`, `host:5000/name`) are refused: Ollama
/// would fetch them from a server outside Aerini's SSRF policy.
pub fn validate_model_name(raw: &str) -> Result<String, NodeError> {
    let name = raw.trim();
    let (path, tag) = match name.split_once(':') {
        Some((p, t)) => (p, Some(t)),
        None => (name, None),
    };
    let segments: Vec<&str> = path.split('/').collect();
    let valid = !name.is_empty()
        && name.len() <= MAX_MODEL_NAME_LEN
        && (segments.len() == 1 || segments.len() == 2)
        && segments.iter().all(|s| is_name_segment(s))
        && tag.is_none_or(is_name_segment);
    if valid {
        Ok(name.to_string())
    } else {
        Err(NodeError::unrecoverable(
            "INVALID_MODEL_NAME",
            "Use a name like llama3.2 or llama3.2:3b. Names from other registries are not supported.",
        ))
    }
}

fn root_is_loopback(root: &str) -> bool {
    let Ok(u) = url::Url::parse(root) else { return false };
    match u.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Text from the remote server, made safe to show: control characters become
/// spaces and the length is capped.
fn clean_remote_text(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_REMOTE_TEXT_CHARS)
        .collect();
    cleaned.trim().to_string()
}

/// Resolves the native root and applies the shared SSRF gate. Only the
/// `local` provider is accepted.
async fn prepare(provider_id: &str, user_base_url: &str) -> Result<String, NodeError> {
    if provider_id != "local" {
        return Err(NodeError::unrecoverable(
            "NOT_LOCAL",
            "Model management is only available for local servers.",
        ));
    }
    let base_url = ProviderRegistry::resolve_base_url(provider_id, user_base_url);
    let root = native_root(&base_url)?;
    check_host_ssrf_from_url(&root, SsrfPolicy::AllowLocal)
        .await
        .map_err(|e| NodeError::unrecoverable("SSRF_BLOCKED", e))?;
    Ok(root)
}

fn client() -> Result<reqwest::Client, NodeError> {
    guarded_client_builder(SsrfPolicy::AllowLocal)
        .redirect(reqwest::redirect::Policy::none())
        .read_timeout(PULL_IDLE_TIMEOUT)
        .build()
        .map_err(|e| NodeError::unrecoverable("NETWORK_ERROR", reqwest_err_msg(&e)))
}

fn authed(req: reqwest::RequestBuilder, api_key: &str) -> reqwest::RequestBuilder {
    let record = ProviderRegistry::global()
        .get("local")
        .expect("the local provider is always registered");
    ProviderRegistry::apply_auth(record, req, api_key)
}

fn send_error(e: &reqwest::Error) -> NodeError {
    if e.is_timeout() && !e.is_connect() {
        return NodeError::unrecoverable(
            "NETWORK_ERROR",
            "The server stopped responding. Try again; a download resumes where it left off.",
        );
    }
    NodeError::unrecoverable("NETWORK_ERROR", reqwest_err_msg(e))
}

fn cancelled() -> NodeError {
    NodeError::unrecoverable("CANCELLED", "Cancelled.")
}

/// Maps a non-success reply to an error, using the server's own message when
/// it sent one.
async fn status_error(response: reqwest::Response) -> NodeError {
    let status = response.status().as_u16();
    let message = match read_json_response_capped(response).await {
        Ok(json) => extract_provider_error(&json, "").map(|m| clean_remote_text(&m)),
        Err(_) => None,
    }
    .filter(|m| !m.is_empty())
    .unwrap_or_else(|| format!("The server answered HTTP {status}."));
    match status {
        401 | 403 => NodeError::unrecoverable("BAD_KEY", message),
        404 => NodeError::unrecoverable("MODEL_NOT_FOUND", message),
        _ => NodeError::unrecoverable("API_ERROR", message),
    }
}

/// Asks the server whether it speaks the Ollama API. Never fails: every
/// problem becomes `ollama: false` with a reason. No request is sent unless
/// the provider is `local` and the URL passes the SSRF gate.
pub async fn probe(provider_id: &str, user_base_url: &str, api_key: &str) -> OllamaProbe {
    if provider_id != "local" {
        return OllamaProbe::no("not_local", false);
    }
    if user_base_url.trim().is_empty() {
        return OllamaProbe::no("no_base_url", false);
    }
    let root = match prepare(provider_id, user_base_url).await {
        Ok(r) => r,
        Err(e) if e.code == "SSRF_BLOCKED" => return OllamaProbe::no("blocked", false),
        Err(_) => return OllamaProbe::no("invalid_url", false),
    };
    let loopback = root_is_loopback(&root);
    let Ok(client) = client() else { return OllamaProbe::no("unreachable", loopback) };
    let req = authed(client.get(format!("{root}/api/version")), api_key).timeout(PROBE_TIMEOUT);
    let response = match req.send().await {
        Ok(r) => r,
        Err(_) => return OllamaProbe::no("unreachable", loopback),
    };
    if !response.status().is_success() {
        return OllamaProbe::no("not_ollama", loopback);
    }
    match read_json_response_capped(response).await {
        Ok(json) => match json.get("version").and_then(Value::as_str) {
            Some(v) => OllamaProbe {
                ollama: true,
                version: Some(clean_remote_text(v)),
                loopback,
                reason: None,
            },
            None => OllamaProbe::no("not_ollama", loopback),
        },
        Err(_) => OllamaProbe::no("not_ollama", loopback),
    }
}

/// Removes an installed model from the server.
pub async fn delete_model(
    provider_id: &str,
    user_base_url: &str,
    api_key: &str,
    model: &str,
) -> Result<(), NodeError> {
    let name = validate_model_name(model)?;
    let root = prepare(provider_id, user_base_url).await?;
    let req = authed(client()?.delete(format!("{root}/api/delete")), api_key)
        .json(&json!({ "model": name }))
        .timeout(DELETE_TIMEOUT);
    let response = req.send().await.map_err(|e| send_error(&e))?;
    if response.status().is_success() {
        return Ok(());
    }
    let mut err = status_error(response).await;
    if err.code == "MODEL_NOT_FOUND" {
        err.message = "That model isn't installed on this server.".to_string();
    }
    Err(err)
}

enum Step {
    Progress(PullProgress),
    Done,
    Skip,
}

/// Folds Ollama's NDJSON pull lines into one running total across layers.
#[derive(Default)]
struct PullAggregator {
    layers: HashMap<String, (u64, u64)>,
}

impl PullAggregator {
    fn fold(&mut self, line: &str) -> Result<Step, String> {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return Ok(Step::Skip) };
        if let Some(e) = v.get("error") {
            let msg = e.as_str().map(clean_remote_text).unwrap_or_default();
            return Err(if msg.is_empty() { "The server reported an error.".to_string() } else { msg });
        }
        let status = v.get("status").and_then(Value::as_str).unwrap_or("");
        if status == "success" {
            return Ok(Step::Done);
        }
        if status.is_empty() {
            return Ok(Step::Skip);
        }
        if let Some(digest) = v.get("digest").and_then(Value::as_str) {
            let total = v.get("total").and_then(Value::as_u64).unwrap_or(0);
            let completed = v.get("completed").and_then(Value::as_u64).unwrap_or(0);
            let layer = self.layers.entry(digest.to_string()).or_insert((0, 0));
            layer.0 = layer.0.max(total);
            layer.1 = layer.1.max(completed);
        }
        let total: u64 = self.layers.values().map(|l| l.0).sum();
        let completed: u64 = self.layers.values().map(|l| l.1.min(l.0)).sum();
        Ok(Step::Progress(PullProgress { phase: phase_of(status), completed, total }))
    }
}

fn phase_of(status: &str) -> &'static str {
    let s = status.to_ascii_lowercase();
    if s.starts_with("pulling manifest") {
        "manifest"
    } else if s.starts_with("verifying") {
        "verifying"
    } else if s.starts_with("writing") || s.starts_with("removing") {
        "finalizing"
    } else if s.starts_with("downloading") || s.starts_with("pulling") {
        "downloading"
    } else {
        "working"
    }
}

/// Downloads a model, reporting progress until the server says `success`.
/// Cancelling closes the connection; the server keeps what it already has, so
/// pulling again resumes.
pub async fn pull_model<F>(
    provider_id: &str,
    user_base_url: &str,
    api_key: &str,
    model: &str,
    cancel: &CancellationToken,
    mut on_progress: F,
) -> Result<(), NodeError>
where
    F: FnMut(PullProgress) + Send,
{
    let name = validate_model_name(model)?;
    let root = prepare(provider_id, user_base_url).await?;
    let req = authed(client()?.post(format!("{root}/api/pull")), api_key)
        .json(&json!({ "model": name, "stream": true }));

    let mut response = tokio::select! {
        _ = cancel.cancelled() => return Err(cancelled()),
        r = req.send() => r.map_err(|e| send_error(&e))?,
    };
    if !response.status().is_success() {
        return Err(status_error(response).await);
    }

    let mut agg = PullAggregator::default();
    let mut buf: Vec<u8> = Vec::new();
    let mut handle = |raw: &[u8]| -> Result<bool, NodeError> {
        let text = String::from_utf8_lossy(raw);
        let line = text.trim();
        if line.is_empty() {
            return Ok(false);
        }
        match agg.fold(line) {
            Ok(Step::Done) => Ok(true),
            Ok(Step::Progress(p)) => {
                on_progress(p);
                Ok(false)
            }
            Ok(Step::Skip) => Ok(false),
            Err(msg) => Err(NodeError::unrecoverable("PULL_FAILED", msg)),
        }
    };

    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled()),
            c = response.chunk() => c.map_err(|e| send_error(&e))?,
        };
        let Some(bytes) = chunk else { break };
        buf.extend_from_slice(&bytes);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            if handle(&line)? {
                return Ok(());
            }
        }
        if buf.len() > MAX_LINE_BYTES {
            return Err(NodeError::unrecoverable(
                "PULL_FAILED",
                "The server sent an unexpected response.",
            ));
        }
    }
    if handle(&buf)? {
        return Ok(());
    }
    Err(NodeError::unrecoverable(
        "PULL_INCOMPLETE",
        "The connection closed before the download finished. Pull again to resume.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    type Captured = Arc<Mutex<Vec<String>>>;

    /// Serves one connection: records the request, writes `chunks` with a
    /// pause between them, then closes. With `hold`, the connection stays
    /// open until the client closes it, and `hold` is signalled on that close.
    async fn spawn_mock(
        status: u16,
        chunks: Vec<&'static str>,
        hold: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> (String, Captured) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("mock bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        let seen: Captured = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else { return };
            let (mut r, mut w) = stream.into_split();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if r.read(&mut byte).await.unwrap_or(0) == 0 { return; }
                head.push(byte[0]);
            }
            let head_text = String::from_utf8_lossy(&head).to_string();
            let length = head_text
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.trim().eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            let mut body = vec![0u8; length];
            let _ = r.read_exact(&mut body).await;
            record.lock().expect("mock lock").push(format!("{head_text}{}", String::from_utf8_lossy(&body)));

            let _ = w
                .write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n").as_bytes())
                .await;
            for c in chunks {
                let _ = w.write_all(c.as_bytes()).await;
                let _ = w.flush().await;
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            if let Some(tx) = hold {
                let mut sink = [0u8; 64];
                loop {
                    match r.read(&mut sink).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
                let _ = tx.send(());
            }
        });
        (format!("http://127.0.0.1:{port}/v1"), seen)
    }

    async fn run_pull(base: &str, key: &str, model: &str) -> (Result<(), NodeError>, Vec<PullProgress>) {
        let mut seen = Vec::new();
        let token = CancellationToken::new();
        let res = pull_model("local", base, key, model, &token, |p| seen.push(p)).await;
        (res, seen)
    }

    #[test]
    fn native_root_strips_v1_and_keeps_any_path_prefix() {
        let root = |u: &str| native_root(u).unwrap();
        assert_eq!(root("http://localhost:11434/v1"), "http://localhost:11434");
        assert_eq!(root("http://localhost:11434/v1/"), "http://localhost:11434");
        assert_eq!(root("http://localhost:11434"), "http://localhost:11434");
        assert_eq!(root("https://host.example/ollama/v1"), "https://host.example/ollama");
    }

    #[test]
    fn native_root_rejects_credentials_queries_and_other_schemes() {
        for bad in ["", "   ", "not a url", "ftp://h/v1", "http://u:p@h/v1", "http://h/v1?x=1", "http://h/v1#f"] {
            assert_eq!(native_root(bad).unwrap_err().code, "INVALID_BASE_URL", "input: {bad:?}");
        }
    }

    #[test]
    fn model_name_accepts_library_namespace_and_tagged_names() {
        for ok in ["llama3.2", " llama3.2:3b ", "library/qwen2.5-coder:7b-q4_K_M", "a/b"] {
            assert!(validate_model_name(ok).is_ok(), "input: {ok:?}");
        }
        assert_eq!(validate_model_name(" llama3.2:3b ").unwrap(), "llama3.2:3b");
    }

    #[test]
    fn model_name_rejects_hosts_spaces_traversal_and_oversize() {
        let long = "a".repeat(201);
        for bad in [
            "", "hf.co/org/repo:Q4", "host:5000/name", "a b", "../x", "a//b", "/a", "a/", "-a", "a:", "a:b:c",
            "a;b", "a\nb", long.as_str(),
        ] {
            assert_eq!(validate_model_name(bad).unwrap_err().code, "INVALID_MODEL_NAME", "input: {bad:?}");
        }
    }

    #[test]
    fn aggregator_sums_layers_and_finishes_on_success() {
        let mut a = PullAggregator::default();
        let phase = |s: Step| match s { Step::Progress(p) => p, _ => panic!("expected progress") };
        let p = phase(a.fold(r#"{"status":"pulling manifest"}"#).unwrap());
        assert_eq!((p.phase, p.total), ("manifest", 0));
        a.fold(r#"{"status":"pulling aaa","digest":"sha256:aaa","total":100,"completed":40}"#).unwrap();
        let p = phase(a.fold(r#"{"status":"pulling bbb","digest":"sha256:bbb","total":300,"completed":60}"#).unwrap());
        assert_eq!((p.phase, p.completed, p.total), ("downloading", 100, 400));
        let p = phase(a.fold(r#"{"status":"pulling aaa","digest":"sha256:aaa","total":100,"completed":10}"#).unwrap());
        assert_eq!(p.completed, 100, "a layer's progress never moves backwards");
        assert_eq!(phase(a.fold(r#"{"status":"verifying sha256 digest"}"#).unwrap()).phase, "verifying");
        assert_eq!(phase(a.fold(r#"{"status":"writing manifest"}"#).unwrap()).phase, "finalizing");
        assert!(matches!(a.fold(r#"{"status":"success"}"#).unwrap(), Step::Done));
    }

    #[test]
    fn aggregator_error_line_fails_and_noise_is_skipped() {
        let mut a = PullAggregator::default();
        assert!(matches!(a.fold("not json").unwrap(), Step::Skip));
        assert!(matches!(a.fold(r#"{"status":""}"#).unwrap(), Step::Skip));
        assert_eq!(a.fold("{\"error\":\"pull model manifest:\\nfile does not exist\"}").err().unwrap(), "pull model manifest: file does not exist");
        assert_eq!(a.fold(r#"{"error":{"x":1}}"#).err().unwrap(), "The server reported an error.");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pull_reports_progress_across_split_lines_and_sends_the_exact_body() {
        let (base, seen) = spawn_mock(200, vec![
            "{\"status\":\"pulling manifest\"}\n{\"status\":\"pulling aaa\",\"digest\":\"sha256:aaa\",\"total\":10,\"comp",
            "leted\":5}\n{\"status\":\"verifying sha256 digest\"}\n{\"status\":\"success\"}\n",
        ], None).await;
        let (res, progress) = run_pull(&base, "secret-key", " llama3.2 ").await;
        res.expect("a stream ending in success must succeed");
        assert!(progress.iter().any(|p| p.phase == "downloading" && p.completed == 5 && p.total == 10), "{progress:?}");
        let request = seen.lock().unwrap()[0].clone();
        assert!(request.starts_with("POST /api/pull "), "{request}");
        assert!(request.contains(r#"{"model":"llama3.2","stream":true}"#), "{request}");
        assert!(!request.contains("insecure"), "{request}");
        assert!(request.to_ascii_lowercase().contains("authorization: bearer secret-key"), "{request}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pull_fails_on_a_mid_stream_error_line() {
        let (base, _) = spawn_mock(200, vec![
            "{\"status\":\"pulling manifest\"}\n{\"error\":\"pull model manifest: file does not exist\"}\n",
        ], None).await;
        let (res, _) = run_pull(&base, "", "nope").await;
        let err = res.expect_err("an error line must fail the pull");
        assert_eq!((err.code.as_str(), err.message.as_str()), ("PULL_FAILED", "pull model manifest: file does not exist"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pull_fails_when_the_stream_ends_without_success() {
        let (base, _) = spawn_mock(200, vec!["{\"status\":\"pulling manifest\"}\n"], None).await;
        let (res, _) = run_pull(&base, "", "llama3.2").await;
        assert_eq!(res.expect_err("no success line").code, "PULL_INCOMPLETE");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pull_maps_http_failures_to_bad_key_and_api_error() {
        let (base, _) = spawn_mock(401, vec!["{\"error\":\"unauthorized\"}"], None).await;
        let (res, _) = run_pull(&base, "", "llama3.2").await;
        assert_eq!(res.expect_err("401").code, "BAD_KEY");
        let (base, _) = spawn_mock(500, vec!["{\"error\":\"boom\"}"], None).await;
        let (res, _) = run_pull(&base, "", "llama3.2").await;
        let err = res.expect_err("500");
        assert_eq!((err.code.as_str(), err.message.as_str()), ("API_ERROR", "boom"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pull_cancel_returns_cancelled_and_closes_the_connection() {
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
        let (base, _) = spawn_mock(200, vec!["{\"status\":\"pulling manifest\"}\n"], Some(closed_tx)).await;
        let token = CancellationToken::new();
        let (first_tx, mut first_rx) = tokio::sync::mpsc::unbounded_channel();
        let task_token = token.clone();
        let task = tokio::spawn(async move {
            pull_model("local", &base, "", "llama3.2", &task_token, move |p| { let _ = first_tx.send(p); }).await
        });
        first_rx.recv().await.expect("the first progress step must arrive");
        token.cancel();
        let err = task.await.unwrap().expect_err("cancel must stop the pull");
        assert_eq!(err.code, "CANCELLED");
        tokio::time::timeout(Duration::from_secs(5), closed_rx)
            .await
            .expect("the server must see the connection close")
            .expect("signal");
    }

    #[tokio::test]
    async fn pull_refuses_link_local_roots_and_non_local_providers() {
        let token = CancellationToken::new();
        let blocked = pull_model("local", "http://169.254.169.254/v1", "", "llama3.2", &token, |_| {}).await;
        assert_eq!(blocked.expect_err("metadata address").code, "SSRF_BLOCKED");
        let wrong = pull_model("openai", "http://127.0.0.1:1/v1", "", "llama3.2", &token, |_| {}).await;
        assert_eq!(wrong.expect_err("not local").code, "NOT_LOCAL");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn probe_recognises_a_version_reply_and_rejects_everything_else() {
        let (base, seen) = spawn_mock(200, vec!["{\"version\":\"0.30.0\"}"], None).await;
        let found = probe("local", &base, "").await;
        assert_eq!((found.ollama, found.version.as_deref(), found.loopback), (true, Some("0.30.0"), true));
        assert!(seen.lock().unwrap()[0].starts_with("GET /api/version "));

        let (base, _) = spawn_mock(404, vec!["{}"], None).await;
        assert_eq!(probe("local", &base, "").await.reason, Some("not_ollama"));
        let (base, _) = spawn_mock(200, vec!["<html>hi</html>"], None).await;
        assert_eq!(probe("local", &base, "").await.reason, Some("not_ollama"));
        assert_eq!(probe("openai", "http://127.0.0.1:1/v1", "").await.reason, Some("not_local"));
        assert_eq!(probe("local", "  ", "").await.reason, Some("no_base_url"));
        assert_eq!(probe("local", "http://169.254.169.254/v1", "").await.reason, Some("blocked"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delete_sends_the_model_and_maps_a_missing_model() {
        let (base, seen) = spawn_mock(200, vec![""], None).await;
        delete_model("local", &base, "k", "llama3.2:3b").await.expect("200 means deleted");
        let request = seen.lock().unwrap()[0].clone();
        assert!(request.starts_with("DELETE /api/delete "), "{request}");
        assert!(request.contains(r#"{"model":"llama3.2:3b"}"#), "{request}");

        let (base, _) = spawn_mock(404, vec!["{\"error\":\"model 'x' not found\"}"], None).await;
        let err = delete_model("local", &base, "", "x").await.expect_err("404");
        assert_eq!((err.code.as_str(), err.message.as_str()), ("MODEL_NOT_FOUND", "That model isn't installed on this server."));
        assert_eq!(delete_model("local", &base, "", "a b").await.unwrap_err().code, "INVALID_MODEL_NAME");
    }
}
