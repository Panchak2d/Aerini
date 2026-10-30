use super::*;

use serde_json::json;

use crate::node::Node;

use crate::nodes::ai_agent::AiAgentNode;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_request_is_returned_to_the_workflow_after_one_call() {
    let tool_reply = r#"{"choices":[{"message":{"role":"assistant","content":"","tool_calls":[{"id":"c1","type":"function","function":{"name":"lookup","arguments":"{\"q\":\"x\"}"}}]},"finish_reason":"tool_calls"}]}"#;
    let (base_url, bodies) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, tool_reply)]).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "openai", "base_url": base_url, "api_key": "k",
        "tools": r#"[{"name":"lookup","description":"d","parameters":{"type":"object","properties":{}}}]"#
    }))).await;
    assert!(out.success, "{:?}", out.error);
    let output = out.output.expect("output");
    assert_eq!(output["awaiting_tools"], true);
    assert_eq!(output["finished"], false);
    assert_eq!(output["iterations"], 1);
    assert_eq!(output["tool_calls"][0]["tool"], "lookup");
    assert_eq!(output["tool_calls"][0]["parsed_arguments"]["q"], "x");
    let seen = bodies.lock().expect("mock lock");
    assert_eq!(seen.len(), 1);
    let first: Value = serde_json::from_str(&seen[0]).expect("json");
    assert_eq!(first["tools"][0]["type"], "function");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_reply_is_a_finished_answer_with_no_tool_requests() {
    let reply = r#"{"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}]}"#;
    let (base_url, _) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "openai", "base_url": base_url, "api_key": "k"
    }))).await;
    let output = out.output.expect("output");
    assert_eq!(output["result"], "done");
    assert_eq!(output["finished"], true);
    assert_eq!(output["awaiting_tools"], false);
}

#[tokio::test]
async fn malformed_tools_fail_before_any_request() {
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "openai", "base_url": "http://127.0.0.1:9", "tools": "not json"
    }))).await;
    assert_eq!(out.error.expect("error").code, "INVALID_TOOLS");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_requests_are_flagged_when_the_tool_or_its_arguments_are_invalid() {
    let reply = r#"{"choices":[{"message":{"role":"assistant","content":"","tool_calls":[
            {"id":"a","type":"function","function":{"name":"lookup","arguments":"{\"q\":\"x\"}"}},
            {"id":"b","type":"function","function":{"name":"lookup","arguments":"{oops"}},
            {"id":"c","type":"function","function":{"name":"nope","arguments":"{}"}},
            {"id":"d","type":"function","function":{"name":"lookup","arguments":"{}"}}
        ]},"finish_reason":"tool_calls"}]}"#;
    let (base_url, _) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "openai", "base_url": base_url, "api_key": "k",
        "tools": r#"[{"name":"lookup","description":"d","parameters":{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}}]"#
    }))).await;
    let output = out.output.expect("output");
    let calls = &output["tool_calls"];
    assert_eq!(calls[0]["arguments_valid"], true);
    assert_eq!(calls[0]["known_tool"], true);
    assert!(calls[0]["error"].is_null());

    assert_eq!(calls[1]["arguments_valid"], false);
    assert!(calls[1]["parsed_arguments"].is_null());
    assert!(calls[1]["error"].as_str().expect("error").contains("not valid JSON"));

    assert_eq!(calls[2]["known_tool"], false);
    assert!(calls[2]["error"].as_str().expect("error").contains("nope"));

    assert_eq!(calls[3]["arguments_valid"], false);
    assert!(calls[3]["error"].as_str().expect("error").contains("\"q\""));

    assert_eq!(output["tool_calls_valid"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_provider_and_token_usage_are_reported_for_every_provider() {
    let cases: [(&str, &str, &str, &str, u64, u64); 3] = [
        (
            "openai",
            r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"stop"}],"model":"gpt-x","usage":{"prompt_tokens":11,"completion_tokens":5}}"#,
            "openai_compatible", "gpt-x", 11, 5,
        ),
        (
            "anthropic",
            r#"{"content":[{"type":"text","text":"hi"}],"model":"claude-x","stop_reason":"end_turn","usage":{"input_tokens":12,"output_tokens":6}}"#,
            "anthropic", "claude-x", 12, 6,
        ),
        (
            "gemini",
            r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}],"modelVersion":"gem-x","usageMetadata":{"promptTokenCount":13,"candidatesTokenCount":4,"thoughtsTokenCount":3}}"#,
            "gemini", "gem-x", 13, 7,
        ),
    ];
    for (provider, reply, label, model, input_tokens, output_tokens) in cases {
        let (base_url, _) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "g", "provider": provider, "base_url": base_url, "api_key": "k"
        }))).await;
        let output = out.output.unwrap_or_else(|| panic!("{provider}: {:?}", out.error));
        assert_eq!(output["provider"], label, "{provider}");
        assert_eq!(output["model"], model, "{provider}");
        assert_eq!(output["input_tokens"], input_tokens, "{provider}");
        assert_eq!(output["output_tokens"], output_tokens, "{provider}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_tool_request_is_returned_in_the_shared_tool_calls_shape() {
    let reply = r#"{"model":"claude-x","stop_reason":"tool_use","usage":{"input_tokens":20,"output_tokens":9},"content":[
            {"type":"thinking","thinking":"","signature":"s"},
            {"type":"text","text":"Checking."},
            {"type":"tool_use","id":"toolu_1","name":"lookup","input":{"q":"x"}}
        ]}"#;
    let (base_url, bodies) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "anthropic", "base_url": base_url, "api_key": "k",
        "tools": r#"[{"name":"lookup","description":"d","parameters":{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}}]"#
    }))).await;
    let output = out.output.unwrap_or_else(|| panic!("{:?}", out.error));
    assert_eq!(output["result"], "Checking.");
    assert_eq!(output["awaiting_tools"], true);
    assert_eq!(output["finished"], false);
    assert_eq!(output["tool_calls_valid"], true);
    assert_eq!(output["tool_calls"][0]["id"], "toolu_1");
    assert_eq!(output["tool_calls"][0]["tool"], "lookup");
    assert_eq!(output["tool_calls"][0]["parsed_arguments"]["q"], "x");
    assert_eq!(output["input_tokens"], 20);

    let seen = bodies.lock().expect("mock lock");
    let sent: Value = serde_json::from_str(&seen[0]).expect("json");
    assert_eq!(sent["tools"][0]["name"], "lookup");
    assert_eq!(sent["tools"][0]["input_schema"]["required"], json!(["q"]));
    assert!(sent.get("tool_choice").is_none(), "forced tool choice is rejected by current Claude models");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_plain_reply_with_tools_declared_is_a_finished_answer() {
    let reply = r#"{"stop_reason":"end_turn","content":[{"type":"thinking","thinking":"","signature":"s"},{"type":"text","text":"done"}]}"#;
    let (base_url, _) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "anthropic", "base_url": base_url, "api_key": "k",
        "tools": r#"[{"name":"lookup"}]"#
    }))).await;
    let output = out.output.expect("output");
    assert_eq!(output["result"], "done");
    assert_eq!(output["finished"], true);
    assert_eq!(output["awaiting_tools"], false);
    assert!(output["reasoning"].as_array().expect("reasoning").iter().all(|l| !l.as_str().unwrap_or("").contains("Warning")));
}

#[tokio::test]
async fn anthropic_tool_with_an_unsendable_name_fails_before_any_request() {
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "anthropic", "base_url": "http://127.0.0.1:9",
        "tools": r#"[{"name":"get weather"}]"#
    }))).await;
    let err = out.error.expect("error");
    assert_eq!(err.code, "INVALID_TOOLS");
    assert!(err.message.contains("get weather"), "{}", err.message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_request_cut_off_at_max_tokens_is_flagged_as_possibly_incomplete() {
    let reply = r#"{"stop_reason":"max_tokens","content":[{"type":"tool_use","id":"t","name":"lookup","input":{}}]}"#;
    let (base_url, _) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "anthropic", "base_url": base_url, "api_key": "k",
        "tools": r#"[{"name":"lookup","parameters":{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}}]"#
    }))).await;
    let output = out.output.expect("output");
    assert_eq!(output["truncated"], true);
    assert_eq!(output["tool_calls"][0]["arguments_valid"], false);
    assert!(output["reasoning"].as_array().expect("reasoning").iter().any(|l| l.as_str().unwrap_or("").contains("cut off")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_tool_controls_are_sent_as_request_fields() {
    let tools = r#"[{"name":"lookup","parameters":{"type":"object","properties":{}}}]"#;
    let cases = [
        (json!({}), "auto", None, None),
        (json!({ "tool_choice": "required", "single_tool_call": true, "strict_tools": true }), "required", Some(false), Some(true)),
        (json!({ "tool_choice": "none" }), "none", None, None),
    ];
    for (extra, choice, parallel, strict) in cases {
        let (base_url, bodies) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, PLAIN_OPENAI_REPLY)]).await;
        let cfg = with_fields(
            json!({ "goal": "g", "provider": "openai", "base_url": base_url, "api_key": "k", "tools": tools }),
            extra,
        );
        let out = AiAgentNode.execute(make_input(cfg)).await;
        assert!(out.success, "{:?}", out.error);
        let sent: Value = serde_json::from_str(&bodies.lock().expect("mock lock")[0]).expect("json");
        assert_eq!(sent["tool_choice"], choice);
        assert_eq!(sent.get("parallel_tool_calls").and_then(Value::as_bool), parallel);
        assert_eq!(sent["tools"][0]["function"].get("strict").and_then(Value::as_bool), strict);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_tool_controls_map_to_tool_choice_and_strict() {
    let reply = r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"ok"}]}"#;
    let tools = r#"[{"name":"lookup","parameters":{"type":"object","properties":{"q":{"type":"string"}},"required":["q"],"additionalProperties":false}},{"name":"ping"}]"#;
    let cases = [
        (json!({}), None, false),
        (json!({ "tool_choice": "none" }), Some(json!({ "type": "none" })), false),
        (json!({ "single_tool_call": true }), Some(json!({ "type": "auto", "disable_parallel_tool_use": true })), false),
        (json!({ "strict_tools": true }), None, true),
    ];
    for (extra, choice, strict) in cases {
        let (base_url, bodies) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
        let cfg = with_fields(
            json!({ "goal": "g", "provider": "anthropic", "base_url": base_url, "api_key": "k", "tools": tools }),
            extra,
        );
        let out = AiAgentNode.execute(make_input(cfg)).await;
        assert!(out.success, "{:?}", out.error);
        let sent: Value = serde_json::from_str(&bodies.lock().expect("mock lock")[0]).expect("json");
        assert_eq!(sent.get("tool_choice").cloned(), choice);
        assert_eq!(sent["tools"][0].get("strict").and_then(Value::as_bool), strict.then_some(true));
        if strict {
            assert_eq!(sent["tools"][1]["strict"], true);
            assert_eq!(sent["tools"][1]["input_schema"]["additionalProperties"], false);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gemini_tool_choice_maps_to_function_calling_mode_and_declarations_stay_clean() {
    let reply = r#"{"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"STOP"}]}"#;
    let tools = r#"[{"type":"function","name":"lookup","strict":true,"parameters":{"type":"object","properties":{}}}]"#;
    let cases = [
        (json!({}), None),
        (json!({ "tool_choice": "required" }), Some("ANY")),
        (json!({ "tool_choice": "none" }), Some("NONE")),
    ];
    for (extra, mode) in cases {
        let (base_url, bodies) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
        let cfg = with_fields(
            json!({ "goal": "g", "provider": "gemini", "base_url": base_url, "api_key": "k", "tools": tools }),
            extra,
        );
        let out = AiAgentNode.execute(make_input(cfg)).await;
        assert!(out.success, "{:?}", out.error);
        let sent: Value = serde_json::from_str(&bodies.lock().expect("mock lock")[0]).expect("json");
        assert_eq!(sent["toolConfig"]["functionCallingConfig"]["mode"].as_str(), mode);
        let declaration = &sent["tools"][0]["functionDeclarations"][0];
        assert_eq!(declaration["name"], "lookup");
        assert!(declaration.get("strict").is_none() && declaration.get("type").is_none());
    }
}

#[tokio::test]
async fn tool_controls_the_provider_cannot_honor_fail_before_any_request() {
    let tools = r#"[{"name":"lookup","parameters":{"type":"object","properties":{"q":{"type":"string"}}}}]"#;
    let cases = [
        ("anthropic", json!({ "tool_choice": "required", "tools": tools }), "Anthropic"),
        ("anthropic", json!({ "strict_tools": true, "tools": tools }), "additionalProperties"),
        ("gemini", json!({ "single_tool_call": true, "tools": tools }), "Gemini"),
        ("gemini", json!({ "strict_tools": true, "tools": tools }), "Gemini"),
        ("openai", json!({ "tool_choice": "required" }), "at least one tool"),
        ("openai", json!({ "tool_choice": "sometimes", "tools": tools }), "auto, required or none"),
    ];
    for (provider, extra, fragment) in cases {
        let cfg = with_fields(
            json!({ "goal": "g", "provider": provider, "base_url": "http://127.0.0.1:9", "api_key": "k" }),
            extra,
        );
        let err = AiAgentNode.execute(make_input(cfg)).await.error.expect("error");
        assert_eq!(err.code, "INVALID_TOOL_CONTROLS", "{provider}: {}", err.message);
        assert!(err.message.contains(fragment), "{provider}: {}", err.message);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn required_tool_choice_with_no_tool_request_is_flagged() {
    let (base_url, _) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, PLAIN_OPENAI_REPLY)]).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "openai", "base_url": base_url, "api_key": "k",
        "tools": r#"[{"name":"lookup"}]"#, "tool_choice": "required"
    }))).await;
    let output = out.output.expect("output");
    assert_eq!(output["awaiting_tools"], false);
    assert!(output["reasoning"].as_array().expect("reasoning").iter().any(|l| l.as_str().unwrap_or("").contains("tool_choice")));
}
