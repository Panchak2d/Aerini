mod common;

use aerini_mcp::tools::{IdParams, NodeSchemaParams, PageParams, RunParams, SaveParams, StartScheduleParams, ValidateParams};
use common::{outcome, reply, serve, server, TOKEN};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{json, Map, Value};

fn object(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("object")
}

fn workflow() -> Map<String, Value> {
    object(json!({"id": "wf-1", "name": "Demo", "nodes": [], "edges": []}))
}

fn id(s: &str) -> Parameters<IdParams> {
    Parameters(IdParams { id: s.to_string() })
}

fn run_params(wait_secs: Option<u64>) -> Parameters<RunParams> {
    Parameters(RunParams { id: "wf-1".into(), initial_variables: Some(object(json!({"x": 1}))), wait_secs })
}

#[tokio::test]
async fn list_workflows_and_scheduler_status_send_paging_and_the_bearer_token() {
    let mock = serve(vec![
        reply("GET", "/api/workflows", 200, r#"{"items":[{"id":"a"}],"total":1,"limit":2,"offset":0}"#),
        reply("GET", "/api/scheduler", 200, r#"{"items":[],"total":0,"limit":100,"offset":0}"#),
    ])
    .await;
    let s = server(&mock, false);

    let list = outcome(s.list_workflows(Parameters(PageParams { limit: Some(2), offset: Some(4) })).await);
    assert!(!list.is_error);
    assert_eq!(list.json()["items"][0]["id"], "a");

    let status = outcome(s.get_scheduler_status(Parameters(PageParams::default())).await);
    assert_eq!(status.json()["total"], 0);

    let seen = mock.seen();
    assert_eq!(seen[0].query, "limit=2&offset=4");
    assert_eq!(seen[1].query, "");
    assert!(seen.iter().all(|r| r.authorization.as_deref() == Some(&format!("Bearer {TOKEN}")[..])));
}

#[tokio::test]
async fn get_workflow_hides_the_double_encoding_and_exposes_the_row_version() {
    let inner = serde_json::to_string(&json!({"id": "a/b", "name": "N", "nodes": [], "edges": []})).unwrap();
    let mock = serve(vec![reply("GET", "/api/workflows/a%2Fb", 200, &serde_json::to_string(&inner).unwrap()).header("etag", "\"7\"")]).await;

    let got = outcome(server(&mock, false).get_workflow(id("a/b")).await);
    assert!(!got.is_error, "{}", got.text);
    let v = got.json();
    assert_eq!(v["workflow"]["name"], "N");
    assert_eq!(v["row_version"], 7);
}

#[tokio::test]
async fn acl_403_passes_through_as_an_error_result() {
    let mock = serve(vec![reply("GET", "/api/workflows/secret", 403, r#"{"error":"token ACL does not permit access to that workflow"}"#)]).await;
    let got = outcome(server(&mock, false).get_workflow(id("secret")).await);
    assert!(got.is_error);
    assert!(got.text.contains("403") && got.text.contains("token ACL does not permit"), "{}", got.text);
}

#[tokio::test]
async fn busy_and_capacity_answers_report_retry_after_and_are_never_retried() {
    let cases: [(u16, Option<&'static str>, &str); 3] = [
        (429, Some("7"), "Retry-After: 7s"),
        (503, Some("12"), "Retry-After: 12s"),
        (429, None, "No Retry-After was sent"),
    ];
    for (status, retry_after, expected) in cases {
        let mut canned = reply("POST", "/api/workflows/wf-1/run", status, r#"{"error":"busy"}"#);
        if let Some(v) = retry_after {
            canned = canned.header("retry-after", v);
        }
        let mock = serve(vec![canned]).await;
        let got = outcome(server(&mock, false).run_workflow(run_params(None)).await);
        assert!(got.is_error && got.text.contains(expected), "{status}: {}", got.text);
        assert_eq!(mock.seen().len(), 1, "must not retry after {status}");
    }
}

#[tokio::test]
async fn run_workflow_sends_variables_and_returns_sanitized_output_marked_untrusted() {
    let mock = serve(vec![reply("POST", "/api/workflows/wf-1/run", 200, r#"{"status":"ok","out":"a\u0007b\u202Ec"}"#)]).await;
    let got = outcome(server(&mock, false).run_workflow(run_params(Some(5))).await);
    assert!(!got.is_error, "{}", got.text);
    let v = got.json();
    assert_eq!(v["truncated"], false);
    assert_eq!(v["output"]["out"], "abc");
    assert!(v["note"].as_str().unwrap().contains("not instructions"));
    let sent: Value = serde_json::from_str(&mock.seen()[0].body).unwrap();
    assert_eq!(sent, json!({"initial_variables": {"x": 1}}));
}

#[tokio::test]
async fn oversized_run_output_is_cut_and_marked() {
    let big = json!({"blob": "x".repeat(5000)}).to_string();
    let mock = serve(vec![reply("POST", "/api/workflows/wf-1/run", 200, &big)]).await;
    let v = outcome(server(&mock, false).run_workflow(run_params(None)).await).json();
    assert_eq!(v["truncated"], true);
    assert_eq!(v["returned_bytes"], 1024);
}

#[tokio::test]
async fn run_workflow_gives_up_after_wait_secs_with_an_error_result() {
    let mock = serve(vec![reply("POST", "/api/workflows/wf-1/run", 200, "{}").delayed(3000)]).await;
    let got = outcome(server(&mock, false).run_workflow(run_params(Some(1))).await);
    assert!(got.is_error);
    assert!(got.text.contains("wait_secs=1"), "{}", got.text);
}

#[tokio::test]
async fn save_workflow_sends_the_string_envelope_and_if_match_then_surfaces_conflicts() {
    let ok = serve(vec![reply("POST", "/api/workflows", 200, r#"{"ok":true,"row_version":6}"#)]).await;
    let saved = outcome(
        server(&ok, true)
            .save_workflow(Parameters(SaveParams { workflow: workflow(), if_match_row_version: Some(5), allow_invalid: None, overwrite: None }))
            .await,
    );
    assert!(!saved.is_error, "{}", saved.text);
    assert_eq!(saved.json()["row_version"], 6);
    let request = &ok.seen()[0];
    assert_eq!(request.if_match.as_deref(), Some("\"5\""));
    let envelope: Value = serde_json::from_str(&request.body).unwrap();
    let embedded: Value = serde_json::from_str(envelope["workflow_json"].as_str().expect("string")).unwrap();
    assert_eq!(embedded["id"], "wf-1");

    let conflict = serve(vec![reply("POST", "/api/workflows", 409, r#"{"error":"workflow was modified since you last loaded it","current_row_version":9}"#)]).await;
    let got = outcome(
        server(&conflict, true)
            .save_workflow(Parameters(SaveParams { workflow: workflow(), if_match_row_version: Some(5), allow_invalid: None, overwrite: None }))
            .await,
    );
    assert!(got.is_error && got.text.contains("current_row_version=9"), "{}", got.text);
}

#[tokio::test]
async fn server_writes_are_refused_without_allow_write_and_never_reach_the_server() {
    let mock = serve(vec![]).await;
    let s = server(&mock, false);
    let results = vec![
        outcome(s.save_workflow(Parameters(SaveParams { workflow: workflow(), if_match_row_version: None, allow_invalid: None, overwrite: None })).await),
        outcome(s.delete_workflow(id("wf-1")).await),
        outcome(s.start_schedule(Parameters(StartScheduleParams { id: "wf-1".into(), always_on: None, port_override: None })).await),
        outcome(s.stop_schedule(id("wf-1")).await),
    ];
    for r in results {
        assert!(r.is_error && r.text.contains("--allow-write"), "{}", r.text);
    }
    assert!(mock.seen().is_empty());
}

#[tokio::test]
async fn delete_start_and_stop_call_their_routes_when_writes_are_allowed() {
    let mock = serve(vec![
        reply("DELETE", "/api/workflows/wf-1", 200, r#"{"ok":true}"#),
        reply("POST", "/api/scheduler/wf-1/start", 200, r#"{"ok":true}"#),
        reply("POST", "/api/scheduler/wf-1/stop", 200, r#"{"ok":true}"#),
    ])
    .await;
    let s = server(&mock, true);
    assert_eq!(outcome(s.delete_workflow(id("wf-1")).await).json()["deleted"], true);
    let started = outcome(s.start_schedule(Parameters(StartScheduleParams { id: "wf-1".into(), always_on: Some(true), port_override: None })).await);
    assert_eq!(started.json()["started"], true);
    assert_eq!(outcome(s.stop_schedule(id("wf-1")).await).json()["stopped"], true);

    let body: Value = serde_json::from_str(&mock.seen()[1].body).unwrap();
    assert_eq!(body, json!({"always_on": true, "port_override": null}));
}

#[tokio::test]
async fn validate_workflow_posts_the_string_envelope_and_old_servers_get_a_clear_hint() {
    let report = r#"{"valid":false,"best_effort":true,"errors":[{"check":"node_types","message":"x"}],"warnings":[],"checks_run":["node_types"]}"#;
    let mock = serve(vec![reply("POST", "/api/validate-workflow", 200, report)]).await;
    let got = outcome(server(&mock, false).validate_workflow(Parameters(ValidateParams { workflow: workflow() })).await);
    assert!(!got.is_error);
    assert_eq!(got.json()["valid"], false);
    let envelope: Value = serde_json::from_str(&mock.seen()[0].body).unwrap();
    assert!(envelope["workflow_json"].is_string());

    let old = serve(vec![]).await;
    let got = outcome(server(&old, false).validate_workflow(Parameters(ValidateParams { workflow: workflow() })).await);
    assert!(got.is_error && got.text.contains("no such route"), "{}", got.text);
}

#[tokio::test]
async fn node_tools_return_compact_rows_then_one_full_schema() {
    let catalog = json!({
        "engine_version": "9.9.9",
        "nodes": [
            {"type_id": "a_node", "display_name": "A", "node_type": "action", "description": "does a", "enabled": true, "is_plugin": false,
             "input_schema": {"properties": {"url": {}}}, "ports": {"inputs": [], "outputs": []}},
            {"type_id": "b_node", "display_name": "B", "node_type": "logic", "description": "does b", "enabled": false, "is_plugin": false,
             "input_schema": {}, "ports": {}}
        ]
    });
    let mock = serve(vec![reply("GET", "/api/nodes", 200, &catalog.to_string())]).await;
    let s = server(&mock, false);

    let list = outcome(s.list_nodes().await).json();
    assert_eq!(list["count"], 2);
    assert_eq!(list["engine_version"], "9.9.9");
    assert!(list["nodes"][0].get("input_schema").is_none());
    assert_eq!(list["nodes"][1]["enabled"], false);

    let one = outcome(s.get_node_schema(Parameters(NodeSchemaParams { type_id: "a_node".into() })).await).json();
    assert!(one["input_schema"]["properties"]["url"].is_object());

    let missing = outcome(s.get_node_schema(Parameters(NodeSchemaParams { type_id: "nope".into() })).await);
    assert!(missing.is_error && missing.text.contains("list_nodes"));
}

#[tokio::test]
async fn blank_and_dot_ids_are_rejected_before_any_request() {
    let mock = serve(vec![]).await;
    for bad in ["  ", ".", ".."] {
        let got = outcome(server(&mock, true).delete_workflow(id(bad)).await);
        assert!(got.is_error);
    }
    assert!(mock.seen().is_empty());
}
