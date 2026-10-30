use super::*;

use serde_json::json;

use crate::node::Node;

use crate::nodes::ai_agent::AiAgentNode;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn response_schema_is_sent_in_each_providers_own_format() {
    let cases = [
        ("openai", PLAIN_OPENAI_REPLY),
        ("anthropic", r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"{}"}]}"#),
        ("gemini", r#"{"candidates":[{"content":{"parts":[{"text":"{}"}]},"finishReason":"STOP"}]}"#),
    ];
    let schema: Value = serde_json::from_str(RESPONSE_SCHEMA).expect("schema");
    for (provider, reply) in cases {
        let (base_url, bodies) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "g", "provider": provider, "base_url": base_url, "api_key": "k",
            "response_schema": RESPONSE_SCHEMA
        }))).await;
        assert!(out.success, "{provider}: {:?}", out.error);
        let sent: Value = serde_json::from_str(&bodies.lock().expect("mock lock")[0]).expect("json");
        match provider {
            "openai" => {
                assert_eq!(sent["response_format"]["type"], "json_schema");
                assert_eq!(sent["response_format"]["json_schema"]["name"], "response");
                assert_eq!(sent["response_format"]["json_schema"]["strict"], true);
                assert_eq!(sent["response_format"]["json_schema"]["schema"], schema);
            }
            "anthropic" => {
                assert_eq!(sent["output_config"]["format"]["type"], "json_schema");
                assert_eq!(sent["output_config"]["format"]["schema"], schema);
            }
            _ => {
                assert_eq!(sent["generationConfig"]["responseMimeType"], "application/json");
                assert_eq!(sent["generationConfig"]["responseJsonSchema"], schema);
                assert_eq!(sent["generationConfig"]["maxOutputTokens"], 2048);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reply_that_fits_response_schema_is_returned_parsed_even_in_a_code_fence() {
    let reply = r#"{"choices":[{"message":{"content":"```json\n{\"n\":7}\n```"},"finish_reason":"stop"}]}"#;
    let (base_url, _) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "openai", "base_url": base_url, "api_key": "k",
        "response_schema": RESPONSE_SCHEMA
    }))).await;
    let output = out.output.expect("output");
    assert_eq!(output["structured"], json!({ "n": 7 }));
    assert_eq!(output["structured_valid"], true);
    assert!(output["structured_error"].is_null());
    assert!(output["result"].as_str().expect("result").starts_with("```json"));
    assert_eq!(output["finished"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reply_that_does_not_fit_response_schema_is_flagged_without_failing_the_node() {
    let cases = [
        (r#"{"choices":[{"message":{"content":"not json"},"finish_reason":"stop"}]}"#, "not valid JSON"),
        (r#"{"choices":[{"message":{"content":"{\"n\":\"x\"}"},"finish_reason":"stop"}]}"#, "\"x\""),
        (r#"{"choices":[{"message":{"content":"{\"n\":"},"finish_reason":"length"}]}"#, "max_tokens"),
    ];
    for (reply, fragment) in cases {
        let (base_url, _) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "g", "provider": "openai", "base_url": base_url, "api_key": "k",
            "response_schema": RESPONSE_SCHEMA
        }))).await;
        let output = out.output.unwrap_or_else(|| panic!("{fragment}: {:?}", out.error));
        assert!(output["structured"].is_null(), "{fragment}");
        assert_eq!(output["structured_valid"], false, "{fragment}");
        let error = output["structured_error"].as_str().expect("structured_error");
        assert!(error.contains(fragment), "{fragment}: {error}");
        assert!(output["reasoning"].as_array().expect("reasoning").iter()
            .any(|l| l.as_str().unwrap_or("").starts_with("[Structured Output Warning]")), "{fragment}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_request_leaves_structured_empty_without_an_error() {
    let reply = r#"{"choices":[{"message":{"content":"","tool_calls":[{"id":"a","type":"function","function":{"name":"lookup","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
    let (base_url, _) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "openai", "base_url": base_url, "api_key": "k",
        "tools": r#"[{"name":"lookup"}]"#, "response_schema": RESPONSE_SCHEMA
    }))).await;
    let output = out.output.expect("output");
    assert_eq!(output["awaiting_tools"], true);
    assert!(output["structured"].is_null());
    assert_eq!(output["structured_valid"], false);
    assert!(output["structured_error"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_response_schema_nothing_is_sent_and_structured_is_empty() {
    let (base_url, bodies) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, PLAIN_OPENAI_REPLY)]).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "openai", "base_url": base_url, "api_key": "k", "response_schema": "  "
    }))).await;
    let output = out.output.expect("output");
    let sent: Value = serde_json::from_str(&bodies.lock().expect("mock lock")[0]).expect("json");
    assert!(sent.get("response_format").is_none());
    assert!(sent.get("web_search_options").is_none());
    assert!(sent.get("reasoning_effort").is_none());
    assert!(output["structured"].is_null());
    assert_eq!(output["structured_valid"], false);
    assert!(output["structured_error"].is_null());
    assert_eq!(output["sources"], json!([]));
    assert_eq!(output["search_queries"], json!([]));
}

#[tokio::test]
async fn a_response_schema_that_cannot_be_sent_fails_before_any_request() {
    let open_object = json!({ "type": "object", "properties": { "n": { "type": "integer" } } });
    let cases = [
        ("openai", json!("{oops"), "not valid JSON"),
        ("openai", json!("[1]"), "must be a JSON Schema object"),
        ("openai", json!(r#"{"type":"array","items":{"type":"string"}}"#), "\"type\": \"object\""),
        ("openai", json!({ "type": "object", "properties": { "n": { "type": "nope" } } }), "not a valid JSON Schema"),
        ("anthropic", open_object, "additionalProperties"),
    ];
    for (provider, schema, fragment) in cases {
        let err = AiAgentNode.execute(make_input(json!({
            "goal": "g", "provider": provider, "base_url": "http://127.0.0.1:9", "api_key": "k",
            "response_schema": schema
        }))).await.error.expect("error");
        assert_eq!(err.code, "INVALID_RESPONSE_SCHEMA", "{fragment}: {}", err.message);
        assert!(err.message.contains(fragment), "{fragment}: {}", err.message);
    }
}
