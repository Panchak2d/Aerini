use super::*;

use serde_json::json;

use crate::node::Node;

use crate::nodes::ai_agent::AiAgentNode;

/// A 429 from an OpenAI-compatible provider is retry-eligible, not a permanent failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_429_is_recoverable() {
    let base_url = spawn_status_mock(
        "429 Too Many Requests",
        r#"{"error":{"message":"rate limited"}}"#,
    ).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "say hi", "provider": "openai", "base_url": base_url
    }))).await;
    assert!(!out.success);
    let err = out.error.expect("expected a NodeError");
    assert!(err.recoverable, "429 must be recoverable, got: {:?}", err);
    assert_eq!(err.code, "RATE_LIMITED");
}

/// Anthropic's 529 ("overloaded") is recoverable, matching ai_prompt/anthropic.rs's
/// 429-or-529 check; a plain 400 stays unrecoverable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_529_is_recoverable_400_is_not() {
    let overloaded_url = spawn_status_mock(
        "529 Overloaded",
        r#"{"error":{"message":"overloaded"}}"#,
    ).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "say hi", "provider": "anthropic", "base_url": overloaded_url
    }))).await;
    let err = out.error.expect("expected a NodeError");
    assert!(err.recoverable, "529 must be recoverable, got: {:?}", err);

    let bad_request_url = spawn_status_mock(
        "400 Bad Request",
        r#"{"error":{"message":"invalid request"}}"#,
    ).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "say hi", "provider": "anthropic", "base_url": bad_request_url
    }))).await;
    let err = out.error.expect("expected a NodeError");
    assert!(!err.recoverable, "400 must stay unrecoverable, got: {:?}", err);
    assert_eq!(err.code, "API_ERROR");
}

/// A gateway failure on a model call is retry-eligible (the call creates
/// nothing); a plain 500 is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_503_is_recoverable_500_is_not() {
    let unavailable_url = spawn_status_mock(
        "503 Service Unavailable",
        r#"{"error":{"message":"upstream unavailable"}}"#,
    ).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "say hi", "provider": "openai", "base_url": unavailable_url
    }))).await;
    let err = out.error.expect("expected a NodeError");
    assert!(err.recoverable, "503 must be recoverable, got: {:?}", err);
    assert_eq!(err.code, "UPSTREAM_UNAVAILABLE");

    let server_error_url = spawn_status_mock(
        "500 Internal Server Error",
        r#"{"error":{"message":"boom"}}"#,
    ).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "say hi", "provider": "openai", "base_url": server_error_url
    }))).await;
    let err = out.error.expect("expected a NodeError");
    assert!(!err.recoverable, "500 must stay unrecoverable, got: {:?}", err);
}

/// A model without tool support fails with TOOLS_NOT_SUPPORTED, whether the server
/// sends Ollama's native string error or the `/v1` object error, and the message
/// names the configured model, not the server's text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_model_without_tool_support_fails_with_tools_not_supported() {
    let bodies: [&'static str; 2] = [
        r#"{"error":"llama3 does not support tools"}"#,
        r#"{"error":{"message":"registry.ollama.ai/library/llama3:instruct Does Not Support Tools","type":"api_error"}}"#,
    ];
    for body in bodies {
        let base_url = spawn_status_mock("400 Bad Request", body).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "g", "provider": "local", "model": "llama3",
            "base_url": base_url, "tools": LOOKUP_TOOL
        }))).await;
        let err = out.error.expect("expected a NodeError");
        assert_eq!(err.code, "TOOLS_NOT_SUPPORTED", "body: {body}");
        assert!(!err.recoverable, "must not be retried");
        assert!(err.message.contains("Model 'llama3' does not support tools"), "got: {}", err.message);
        assert!(!err.message.contains("registry"), "must not echo the server text, got: {}", err.message);
    }
}

/// Only the one phrase, on a 400 that carried tools, gets the new code: any other
/// 400, and the same phrase with no tools declared, stays API_ERROR.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_bad_requests_stay_api_error() {
    let other = spawn_status_mock(
        "400 Bad Request",
        r#"{"error":{"message":"tools are not available for this model"}}"#,
    ).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "local", "model": "llama3", "base_url": other, "tools": LOOKUP_TOOL
    }))).await;
    assert_eq!(out.error.expect("expected a NodeError").code, "API_ERROR");

    let no_tools = spawn_status_mock(
        "400 Bad Request",
        r#"{"error":"llama3 does not support tools"}"#,
    ).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "local", "model": "llama3", "base_url": no_tools
    }))).await;
    assert_eq!(out.error.expect("expected a NodeError").code, "API_ERROR");
}
