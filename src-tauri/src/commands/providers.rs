//! Model-discovery Tauri command, backing the credential panel and
//! node-config "Fetch Models" affordance. Thin wrapper: all provider
//! dispatch, base-url resolution, and SSRF gating live in
//! `aerini_engine::nodes::ai_prompt::list_models`, the same coordinating
//! function `AiPromptNode::execute()` mirrors.

#[tauri::command]
pub async fn list_provider_models(
    provider: String,
    base_url: String,
    api_key: String,
) -> Result<Vec<String>, String> {
    // AI Prompt's own schema defaults `provider` to "auto" (it's enum[0]),
    // and local-models.md recommends leaving it there — so the "Fetch
    // Models" button must work under that default too. `list_models`
    // itself takes an already-resolved provider_id (its own allowlist
    // guard rejects anything else), so "auto" is resolved here the same
    // way `AiPromptNode`/`AiAgentNode`'s own `execute()` resolve it, reusing
    // that single classifier rather than adding a second one.
    let provider_id: &str = match provider.as_str() {
        "anthropic" | "gemini" | "openai" | "local" => provider.as_str(),
        _ => aerini_engine::provider::ProviderRegistry::detect_from_url(&base_url),
    };
    aerini_engine::nodes::ai_prompt::list_models(provider_id, &base_url, &api_key)
        .await
        .map_err(|e| format!("{}: {}", e.code, e.message))
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
}
