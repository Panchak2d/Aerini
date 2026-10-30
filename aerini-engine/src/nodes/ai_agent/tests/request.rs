use super::*;

use serde_json::json;

use crate::node::Node;

use crate::nodes::ai_agent::AiAgentNode;

#[tokio::test]
async fn missing_model_with_local_provider_returns_missing_model_error_without_request() {
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "say hi", "provider": "local"
    }))).await;
    assert!(!out.success);
    let err = out.error.expect("must carry NodeError");
    assert_eq!(err.code, "MISSING_MODEL");
    assert!(!err.recoverable);
}

/// An explicit "local" is used as given: re-deriving the provider from a
/// public-looking base_url would pick "openai", whose default model means
/// this error would never be raised.
#[tokio::test]
async fn explicit_local_provider_survives_public_looking_base_url() {
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "say hi", "provider": "local", "base_url": "https://api.example.com"
    }))).await;
    assert!(!out.success);
    let err = out.error.expect("must carry NodeError");
    assert_eq!(err.code, "MISSING_MODEL");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_provider_resolves_its_own_auth_record() {
    let base_url = spawn_minimal_openai_agent_mock().await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "say hi", "provider": "local", "model": "llama3", "base_url": base_url
    }))).await;
    assert!(out.success, "provider=\"local\" must resolve its own record, not panic or fail: {:?}", out.error);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregistered_provider_id_falls_back_to_openai_auth_without_panicking() {
    let base_url = spawn_minimal_openai_agent_mock().await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "say hi", "provider": "custom-proxy", "base_url": base_url
    }))).await;
    assert!(out.success, "an unrecognized provider_id must fall back to openai auth, not panic: {:?}", out.error);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_base_url_is_no_longer_ssrf_blocked() {
    let base_url = spawn_minimal_openai_agent_mock().await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "say hi",
        "provider": "openai",
        "base_url": base_url
    }))).await;
    if let Some(err) = &out.error {
        assert_ne!(err.code, "SSRF_BLOCKED", "loopback base_url must be allowed under SsrfPolicy::AllowLocal");
    }
    assert!(out.success, "request should reach the local mock server and succeed: {:?}", out.error);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn omitted_model_defaults_to_current_openai_flagship() {
    let (base_url, rx) = spawn_capturing_mock(
        r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"stop"}]}"#
    ).await;
    let _ = AiAgentNode.execute(make_input(json!({
        "goal": "say hi", "provider": "openai", "base_url": base_url
    }))).await;
    let req = rx.await.expect("mock never received a request");
    let v: serde_json::Value = serde_json::from_str(&req.body).expect("request body was not valid JSON");
    assert_eq!(v["model"].as_str(), Some("gpt-5.6"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn omitted_model_defaults_to_current_anthropic_default() {
    let (base_url, rx) = spawn_capturing_mock(
        r#"{"content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn"}"#
    ).await;
    let _ = AiAgentNode.execute(make_input(json!({
        "goal": "say hi", "provider": "anthropic", "base_url": base_url
    }))).await;
    let req = rx.await.expect("mock never received a request");
    let v: serde_json::Value = serde_json::from_str(&req.body).expect("request body was not valid JSON");
    assert_eq!(v["model"].as_str(), Some("claude-sonnet-5"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn omitted_model_defaults_to_current_gemini_flash() {
    let (base_url, rx) = spawn_capturing_mock(
        r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}]}"#
    ).await;
    let _ = AiAgentNode.execute(make_input(json!({
        "goal": "say hi", "provider": "gemini", "base_url": base_url, "api_key": "test-key"
    }))).await;
    let req = rx.await.expect("mock never received a request");
    // Gemini's model is part of the URL path, not the JSON body.
    assert!(
        req.request_line.contains("gemini-3.6-flash"),
        "expected default model 'gemini-3.6-flash' in request path, got: {}",
        req.request_line
    );
}

#[tokio::test]
async fn empty_goal_without_readable_attachment_is_missing_goal() {
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "",
        "attachments": [{ "filename": "x.exe", "mime_type": "application/octet-stream", "data": "zz" }]
    }))).await;
    assert!(!out.success);
    assert_eq!(out.error.expect("must carry NodeError").code, "MISSING_GOAL");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_goal_with_image_attachment_sends_text_and_image() {
    let (base_url, rx) = spawn_capturing_mock(
        r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#
    ).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "", "provider": "openai", "base_url": base_url,
        "attachments_expr": r#"[{"filename":"a.png","mime_type":"image/png","data":"AAAA"}]"#
    }))).await;
    assert!(out.success, "attachment-only goal must run: {:?}", out.error);
    let req = rx.await.expect("mock never received a request");
    let v: serde_json::Value = serde_json::from_str(&req.body).expect("request body was not valid JSON");
    let blocks = &v["messages"][1]["content"];
    assert!(!blocks[0]["text"].as_str().unwrap_or("").is_empty(), "user text block must not be empty");
    assert_eq!(blocks[1]["type"].as_str(), Some("image_url"));
}
