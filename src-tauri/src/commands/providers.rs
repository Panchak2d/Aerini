//! Model-discovery and Ollama model-management Tauri commands, backing the
//! credential panel and node-config "Fetch Models", "Pull" and "Delete"
//! affordances. Thin wrappers: provider dispatch, base-url resolution and SSRF
//! gating live in `aerini_engine::nodes::ai_prompt` (`list_models`, `ollama`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use aerini_engine::error::NodeError;
use aerini_engine::executor::SecretString;
use aerini_engine::nodes::ai_prompt::ollama::{self, OllamaProbe, PullProgress};
use aerini_engine::provider::ProviderRegistry;
use aerini_engine::store::CredentialStore;
use serde::Deserialize;
use tauri::ipc::Channel;
use tokio_util::sync::CancellationToken;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const MAX_PULL_ID_LEN: usize = 128;

#[tauri::command]
pub async fn list_provider_models(
    provider: String,
    base_url: String,
    api_key: String,
) -> Result<Vec<String>, String> {
    let api_key = SecretString::new(api_key);
    models_for(&provider, &base_url, &api_key).await
}

/// AI Prompt's schema defaults `provider` to "auto", so every command that
/// takes a provider must resolve it the way `AiPromptNode`/`AiAgentNode`'s
/// `execute()` do. The engine functions take an already-resolved id.
fn resolve_provider<'a>(provider: &'a str, base_url: &str) -> &'a str {
    match provider {
        "anthropic" | "gemini" | "openai" | "local" => provider,
        _ => ProviderRegistry::detect_from_url(base_url),
    }
}

fn describe(e: NodeError) -> String {
    format!("{}: {}", e.code, e.message)
}

pub(crate) async fn models_for(
    provider: &str,
    base_url: &str,
    api_key: &SecretString,
) -> Result<Vec<String>, String> {
    let provider_id = resolve_provider(provider, base_url);
    aerini_engine::nodes::ai_prompt::list_models(provider_id, base_url, api_key)
        .await
        .map_err(describe)
}

/// The server a probe, pull or delete is aimed at. With `credential_id` the
/// secret is read in Rust and never reaches the webview; otherwise `api_key`
/// is used as sent. Deliberately not `Debug`, so the key cannot be logged.
#[derive(Deserialize)]
pub struct OllamaTarget {
    provider: String,
    base_url: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    credential_id: Option<String>,
}

struct ResolvedTarget {
    provider_id: String,
    base_url: String,
    secret: SecretString,
}

async fn resolve_target(
    target: OllamaTarget,
    store: &Arc<CredentialStore>,
) -> Result<ResolvedTarget, String> {
    let provider_id = resolve_provider(&target.provider, &target.base_url).to_string();
    let secret = match target.credential_id.filter(|id| !id.is_empty()) {
        Some(id) => crate::commands::credentials::credential_secret(store, id).await?,
        None => SecretString::new(target.api_key.unwrap_or_default()),
    };
    Ok(ResolvedTarget { provider_id, base_url: target.base_url, secret })
}

/// Pulls in flight: one cancellation token per pull id, and the set of
/// `(server root, model)` pairs being downloaded so the same model is never
/// pulled twice into the same server at once.
#[derive(Default)]
pub struct PullState {
    inner: Mutex<PullRegistry>,
}

#[derive(Default)]
struct PullRegistry {
    tokens: HashMap<String, CancellationToken>,
    busy: HashSet<(String, String)>,
}

impl PullState {
    fn registry(&self) -> MutexGuard<'_, PullRegistry> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Registers a pull. The returned guard frees both entries on drop,
    /// whichever way the pull ends.
    fn begin(
        &self,
        pull_id: &str,
        key: (String, String),
    ) -> Result<(PullGuard<'_>, CancellationToken), String> {
        let mut reg = self.registry();
        if reg.tokens.contains_key(pull_id) {
            return Err("PULL_IN_PROGRESS: This pull id is already in use.".to_string());
        }
        if !reg.busy.insert(key.clone()) {
            return Err("PULL_IN_PROGRESS: Already pulling this model.".to_string());
        }
        let token = CancellationToken::new();
        reg.tokens.insert(pull_id.to_string(), token.clone());
        Ok((PullGuard { state: self, pull_id: pull_id.to_string(), key }, token))
    }

    /// Cancels the pull with this id. An unknown or finished id is a no-op.
    fn cancel(&self, pull_id: &str) {
        if let Some(token) = self.registry().tokens.get(pull_id) {
            token.cancel();
        }
    }
}

struct PullGuard<'a> {
    state: &'a PullState,
    pull_id: String,
    key: (String, String),
}

impl Drop for PullGuard<'_> {
    fn drop(&mut self) {
        let mut reg = self.state.registry();
        reg.tokens.remove(&self.pull_id);
        reg.busy.remove(&self.key);
    }
}

/// Decides which progress steps go to the webview: the first of each phase,
/// then at most one per `PROGRESS_INTERVAL`.
struct Throttle {
    last_sent: Option<Instant>,
    last_phase: Option<&'static str>,
}

impl Throttle {
    fn due(&mut self, phase: &'static str) -> bool {
        let now = Instant::now();
        let fresh = self.last_phase != Some(phase)
            || self.last_sent.is_none_or(|t| now.duration_since(t) >= PROGRESS_INTERVAL);
        if fresh {
            self.last_phase = Some(phase);
            self.last_sent = Some(now);
        }
        fresh
    }
}

async fn run_pull(
    pulls: &PullState,
    pull_id: &str,
    target: &ResolvedTarget,
    model: &str,
    mut emit: impl FnMut(PullProgress) -> bool + Send,
) -> Result<&'static str, String> {
    if pull_id.is_empty() || pull_id.len() > MAX_PULL_ID_LEN {
        return Err("INVALID_PULL_ID: Pull id must be 1 to 128 characters.".to_string());
    }
    let name = ollama::validate_model_name(model).map_err(describe)?;
    let base = ProviderRegistry::resolve_base_url(&target.provider_id, &target.base_url);
    let root = ollama::native_root(&base).map_err(describe)?;
    let (_guard, token) = pulls.begin(pull_id, (root, name.clone()))?;

    let mut throttle = Throttle { last_sent: None, last_phase: None };
    let on_progress = {
        let token = token.clone();
        move |p: PullProgress| {
            if throttle.due(p.phase) && !emit(p) {
                token.cancel();
            }
        }
    };
    match ollama::pull_model(
        &target.provider_id,
        &target.base_url,
        &target.secret,
        &name,
        &token,
        on_progress,
    )
    .await
    {
        Ok(()) => Ok("done"),
        Err(e) if e.code == "CANCELLED" => Ok("cancelled"),
        Err(e) => Err(describe(e)),
    }
}

#[tauri::command]
pub async fn ollama_probe(
    target: OllamaTarget,
    store: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<OllamaProbe, String> {
    let t = resolve_target(target, &store).await?;
    Ok(ollama::probe(&t.provider_id, &t.base_url, &t.secret).await)
}

#[tauri::command]
pub async fn ollama_pull_model(
    pull_id: String,
    target: OllamaTarget,
    model: String,
    on_progress: Channel<PullProgress>,
    pulls: tauri::State<'_, PullState>,
    store: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<&'static str, String> {
    let t = resolve_target(target, &store).await?;
    run_pull(&pulls, &pull_id, &t, &model, move |p| on_progress.send(p).is_ok()).await
}

#[tauri::command]
pub fn ollama_cancel_pull(pull_id: String, pulls: tauri::State<'_, PullState>) {
    pulls.cancel(&pull_id);
}

#[tauri::command]
pub async fn ollama_delete_model(
    target: OllamaTarget,
    model: String,
    store: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<(), String> {
    let t = resolve_target(target, &store).await?;
    ollama::delete_model(&t.provider_id, &t.base_url, &t.secret, &model)
        .await
        .map_err(describe)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn spawn_status_mock(status_line: &'static str, response_body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            let (r, mut w) = stream.split();
            let mut reader = BufReader::new(r);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 { break; }
                if line.trim().is_empty() { break; }
            }
            let resp = format!(
                "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(), response_body
            );
            let _ = w.write_all(resp.as_bytes()).await;
        });
        format!("http://127.0.0.1:{}", port)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn success_case_returns_model_ids() {
        let base_url = spawn_status_mock(
            "HTTP/1.1 200 OK",
            r#"{"data":[{"id":"gpt-5.6"}]}"#,
        ).await;
        let ids = list_provider_models("openai".into(), base_url, "".into())
            .await
            .expect("well-formed response must succeed");
        assert_eq!(ids, vec!["gpt-5.6".to_string()]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bad_key_maps_to_bad_key_prefixed_string() {
        let base_url = spawn_status_mock(
            "HTTP/1.1 401 Unauthorized",
            r#"{"error":{"message":"Incorrect API key provided"}}"#,
        ).await;
        let err = list_provider_models("openai".into(), base_url, "bad-key".into())
            .await
            .expect_err("401 must surface as an error");
        assert!(err.starts_with("BAD_KEY:"), "err: {err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn catch_all_server_error_maps_to_api_error_prefixed_string() {
        let base_url = spawn_status_mock(
            "HTTP/1.1 500 Internal Server Error",
            r#"{"error":{"message":"internal server error"}}"#,
        ).await;
        let err = list_provider_models("openai".into(), base_url, "".into())
            .await
            .expect_err("500 must surface as an error");
        assert!(err.starts_with("API_ERROR:"), "err: {err}");
    }

    /// `provider: "auto"` — AI Prompt's own schema default — must resolve to
    /// a real provider_id via `detect_from_url` rather than reaching
    /// `list_models`'s allowlist guard with the literal string `"auto"`.
    /// `spawn_status_mock` binds `127.0.0.1`, so this exercises the
    /// loopback→`"local"` branch specifically (the common Ollama-on-`auto`
    /// case), not just "some resolution happened".
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn auto_provider_resolves_via_detect_from_url_instead_of_erroring() {
        let base_url = spawn_status_mock(
            "HTTP/1.1 200 OK",
            r#"{"data":[{"id":"llama3"}]}"#,
        ).await;
        let ids = list_provider_models("auto".into(), base_url, "".into())
            .await
            .expect("\"auto\" must resolve via detect_from_url, not error with UNKNOWN_PROVIDER");
        assert_eq!(ids, vec!["llama3".to_string()]);
    }

    /// Connection refused covers both "no server" and "unreachable host" —
    /// `send_and_parse` (aerini-engine) maps both to the same NETWORK_ERROR
    /// code via `reqwest::Error::is_connect()`, so one test represents both.
    #[tokio::test]
    async fn connection_refused_maps_to_network_error_prefixed_string() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        drop(listener);
        let err = list_provider_models("openai".into(), format!("http://127.0.0.1:{port}"), "".into())
            .await
            .expect_err("connection refused must surface as an error");
        assert!(err.starts_with("NETWORK_ERROR:"), "err: {err}");
    }

    #[test]
    fn resolve_provider_keeps_explicit_ids_and_detects_auto() {
        assert_eq!(resolve_provider("anthropic", "http://localhost:11434/v1"), "anthropic");
        assert_eq!(resolve_provider("auto", "http://localhost:11434/v1"), "local");
        assert_eq!(resolve_provider("auto", "http://myollama.local/v1"), "openai");
    }

    fn key(model: &str) -> (String, String) {
        ("http://localhost:11434".to_string(), model.to_string())
    }

    #[test]
    fn pull_state_rejects_duplicate_key_and_frees_it_on_drop() {
        let pulls = PullState::default();
        let (guard, _token) = pulls.begin("a", key("llama3.2")).expect("first pull registers");
        let err = pulls.begin("b", key("llama3.2")).err().expect("same key must be refused");
        assert!(err.contains("Already pulling this model"), "err: {err}");
        assert!(pulls.begin("a", key("qwen3")).is_err(), "a live pull id must not be reused");
        drop(guard);
        assert!(pulls.begin("b", key("llama3.2")).is_ok(), "key and id are free after drop");
    }

    #[test]
    fn pull_state_cancel_stops_known_id_and_ignores_unknown() {
        let pulls = PullState::default();
        pulls.cancel("missing");
        let (_guard, token) = pulls.begin("a", key("llama3.2")).expect("registers");
        assert!(!token.is_cancelled());
        pulls.cancel("a");
        assert!(token.is_cancelled());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_pull_cancel_resolves_cancelled_and_frees_the_key() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("mock bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        // Accepts the request and never answers, so the pull waits on the server.
        tokio::spawn(async move {
            let _conn = listener.accept().await;
            std::future::pending::<()>().await;
        });
        let pulls = Arc::new(PullState::default());
        let target = ResolvedTarget {
            provider_id: "local".to_string(),
            base_url: format!("http://127.0.0.1:{port}/v1"),
            secret: SecretString::new(String::new()),
        };
        let task = {
            let pulls = Arc::clone(&pulls);
            tokio::spawn(async move { run_pull(&pulls, "p1", &target, "llama3.2", |_| true).await })
        };
        for _ in 0..200 {
            if pulls.registry().tokens.contains_key("p1") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        pulls.cancel("p1");
        let outcome = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("pull must stop after cancel")
            .expect("pull task must not panic");
        assert_eq!(outcome, Ok("cancelled"));
        let reg = pulls.registry();
        assert!(reg.tokens.is_empty() && reg.busy.is_empty());
    }
}
