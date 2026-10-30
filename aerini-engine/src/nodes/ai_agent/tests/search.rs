use super::*;

use serde_json::json;

use crate::node::Node;

use crate::nodes::ai_agent::AiAgentNode;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_web_search_is_sent_as_a_server_tool_and_its_sources_come_back() {
    let reply = r#"{"stop_reason":"end_turn","content":[
            {"type":"text","text":"Searching. "},
            {"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{"query":"aerini release"}},
            {"type":"web_search_tool_result","tool_use_id":"srvtoolu_1","content":[
                {"type":"web_search_result","url":"https://a.example/x","title":"A","encrypted_content":"e"},
                {"type":"web_search_result","url":"https://b.example/y","title":"B","encrypted_content":"e"}]},
            {"type":"server_tool_use","id":"srvtoolu_2","name":"web_search","input":{"query":"second"}},
            {"type":"web_search_tool_result","tool_use_id":"srvtoolu_2","content":{"type":"web_search_tool_result_error","error_code":"max_uses_exceeded"}},
            {"type":"text","text":"Found it.","citations":[{"type":"web_search_result_location","url":"https://a.example/x","title":"A","cited_text":"c","encrypted_index":"i"}]}
        ]}"#;
    let (base_url, bodies) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "anthropic", "base_url": base_url, "api_key": "k",
        "tools": LOOKUP_TOOL, "web_search": true,
        "web_search_domains": "A.example\ndocs.b.example/guide, a.example"
    }))).await;
    let output = out.output.unwrap_or_else(|| panic!("{:?}", out.error));

    assert_eq!(output["result"], "Searching. Found it.");
    assert_eq!(output["search_queries"], json!(["aerini release", "second"]));
    assert_eq!(output["sources"], json!([
        { "title": "A", "url": "https://a.example/x" },
        { "title": "B", "url": "https://b.example/y" }
    ]));
    assert_eq!(output["awaiting_tools"], false, "server tool blocks are not requests for the workflow");
    assert_eq!(output["finished"], true);
    let trace = output["reasoning"].to_string();
    assert!(trace.contains("[Web Search] aerini release"), "{trace}");
    assert!(trace.contains("a web search failed: max_uses_exceeded"), "{trace}");

    let sent: Value = serde_json::from_str(&bodies.lock().expect("mock lock")[0]).expect("json");
    assert_eq!(sent["tools"][0]["name"], "lookup");
    assert_eq!(sent["tools"][1], json!({
        "type": "web_search_20250305", "name": "web_search", "max_uses": 5,
        "allowed_domains": ["a.example", "docs.b.example/guide"]
    }));
    assert!(sent.get("tool_choice").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gemini_web_search_is_sent_as_google_search_and_grounding_comes_back() {
    let reply = r#"{"candidates":[{"content":{"parts":[{"text":"Answer"}]},"finishReason":"STOP","groundingMetadata":{
            "webSearchQueries":["q1","q2"],
            "searchEntryPoint":{"renderedContent":"<div>chips</div>"},
            "groundingChunks":[{"web":{"uri":"https://v.example/1","title":"One"}},{"web":{"uri":"https://v.example/1","title":"One again"}},{"web":{"uri":"https://v.example/2","title":"Two"}}]}}]}"#;
    let (base_url, bodies) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
    let out = AiAgentNode.execute(make_input(json!({
        "goal": "g", "provider": "gemini", "base_url": base_url, "api_key": "k", "web_search": true
    }))).await;
    let output = out.output.unwrap_or_else(|| panic!("{:?}", out.error));
    assert_eq!(output["search_queries"], json!(["q1", "q2"]));
    assert_eq!(output["search_suggestions"], "<div>chips</div>");
    assert_eq!(output["sources"], json!([
        { "title": "One", "url": "https://v.example/1" },
        { "title": "Two", "url": "https://v.example/2" }
    ]));
    let sent: Value = serde_json::from_str(&bodies.lock().expect("mock lock")[0]).expect("json");
    assert_eq!(sent["tools"], json!([{ "google_search": {} }]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn effort_is_sent_in_each_providers_own_field_and_keeps_the_response_schema() {
    let cases = [
        ("openai", "none", r#"{"choices":[{"message":{"content":"{\"n\":1}"},"finish_reason":"stop"}]}"#),
        ("anthropic", "xhigh", r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"{\"n\":1}"}]}"#),
        ("gemini", "minimal", r#"{"candidates":[{"content":{"parts":[{"text":"{\"n\":1}"}]},"finishReason":"STOP"}]}"#),
    ];
    for (provider, effort, reply) in cases {
        let (base_url, bodies) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "g", "provider": provider, "base_url": base_url, "api_key": "k",
            "effort": effort, "response_schema": RESPONSE_SCHEMA
        }))).await;
        let output = out.output.unwrap_or_else(|| panic!("{provider}: {:?}", out.error));
        assert_eq!(output["structured_valid"], true, "{provider}");
        let sent: Value = serde_json::from_str(&bodies.lock().expect("mock lock")[0]).expect("json");
        match provider {
            "openai" => assert_eq!(sent["reasoning_effort"], effort),
            "anthropic" => {
                assert_eq!(sent["output_config"]["effort"], effort);
                assert_eq!(sent["output_config"]["format"]["type"], "json_schema");
            }
            _ => assert_eq!(sent["generationConfig"]["thinkingConfig"]["thinkingLevel"], effort),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_search_paused_by_anthropic_is_flagged_or_fails_when_nothing_was_written() {
    let partial = r#"{"stop_reason":"pause_turn","content":[{"type":"text","text":"So far"},{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{"query":"q"}}]}"#;
    let empty = r#"{"stop_reason":"pause_turn","content":[{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{"query":"q"}}]}"#;
    for (reply, text_written) in [(partial, true), (empty, false)] {
        let (base_url, _) = crate::nodes::ai_prompt::spawn_sequence_mock(vec![(200, reply)]).await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "g", "provider": "anthropic", "base_url": base_url, "api_key": "k", "web_search": true
        }))).await;
        if text_written {
            let output = out.output.unwrap_or_else(|| panic!("{:?}", out.error));
            assert_eq!(output["result"], "So far");
            assert_eq!(output["finished"], false);
            assert!(output["reasoning"].to_string().contains("paused"));
        } else {
            assert_eq!(out.error.expect("error").code, "SEARCH_PAUSED");
        }
    }
}

#[tokio::test]
async fn web_search_and_effort_settings_the_provider_cannot_honor_fail_before_any_request() {
    let cases = [
        ("anthropic", json!({ "web_search": true, "response_schema": RESPONSE_SCHEMA }), "INVALID_WEB_SEARCH", "citations"),
        ("anthropic", json!({ "web_search": true, "tools": r#"[{"name":"web_search"}]"# }), "INVALID_WEB_SEARCH", "clashes"),
        ("anthropic", json!({ "web_search": true, "tools": LOOKUP_TOOL, "tool_choice": "none" }), "INVALID_WEB_SEARCH", "switch off"),
        ("anthropic", json!({ "web_search": true, "web_search_max_uses": 0 }), "INVALID_WEB_SEARCH", "1 or more"),
        ("anthropic", json!({ "web_search": true, "web_search_domains": "https://x.com" }), "INVALID_WEB_SEARCH", "without http"),
        ("gemini", json!({ "web_search": true, "tools": LOOKUP_TOOL }), "INVALID_WEB_SEARCH", "Tools"),
        ("gemini", json!({ "web_search": true, "web_search_max_uses": 3 }), "INVALID_WEB_SEARCH", "only with Anthropic"),
        ("openai", json!({ "web_search": true }), "INVALID_WEB_SEARCH", "api.openai.com"),
        ("local", json!({ "web_search": true, "model": "llama3" }), "INVALID_WEB_SEARCH", "api.openai.com"),
        ("anthropic", json!({ "effort": "none" }), "INVALID_EFFORT", "Anthropic"),
        ("gemini", json!({ "effort": "max" }), "INVALID_EFFORT", "Gemini"),
        ("openai", json!({ "effort": "turbo" }), "INVALID_EFFORT", "not \"turbo\""),
    ];
    for (provider, extra, code, fragment) in cases {
        let cfg = with_fields(
            json!({ "goal": "g", "provider": provider, "base_url": "http://127.0.0.1:9", "api_key": "k" }),
            extra,
        );
        let err = AiAgentNode.execute(make_input(cfg)).await.error.expect("error");
        assert_eq!(err.code, code, "{provider}: {}", err.message);
        assert!(err.message.contains(fragment), "{provider}: {}", err.message);
    }
}
