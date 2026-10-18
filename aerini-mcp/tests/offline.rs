#![cfg(feature = "offline")]

mod common;

use aerini_mcp::{
    tools::{IdParams, NodeSchemaParams, SaveParams, ValidateParams},
    AerinMcp, Config,
};
use common::outcome;
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{json, Map, Value};
use std::{path::PathBuf, time::Duration};

fn object(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap()
}

fn offline(tag: &str) -> (AerinMcp, PathBuf) {
    let dir = std::env::temp_dir().join(format!("aerini-mcp-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = Config {
        url: None,
        token: None,
        allow_write: false,
        timeout: Duration::from_secs(5),
        max_output_bytes: 1024,
        out_dir: Some(dir.clone()),
    };
    (AerinMcp::new(cfg).unwrap(), dir)
}

fn node(id: &str, type_id: &str) -> Value {
    json!({"id": id, "node_type_id": type_id, "node_type": "utility", "name": id, "config": {}, "input_schema": {}, "output_schema": {}})
}

fn valid_workflow(id: &str) -> Map<String, Value> {
    object(json!({
        "id": id, "name": "Demo",
        "nodes": [node("t", "manual_trigger"), node("o", "output")],
        "edges": [{"id": "e1", "from_node": "t", "from_port": "output", "to_node": "o", "to_port": "input"}]
    }))
}

fn invalid_workflow(id: &str) -> Map<String, Value> {
    object(json!({"id": id, "name": "Bad", "nodes": [node("x", "no_such_type")], "edges": []}))
}

fn save(workflow: Map<String, Value>, allow_invalid: bool, overwrite: bool) -> Parameters<SaveParams> {
    Parameters(SaveParams { workflow, if_match_row_version: None, allow_invalid: Some(allow_invalid), overwrite: Some(overwrite) })
}

#[tokio::test]
async fn catalog_comes_from_the_linked_engine_sorted_without_a_server() {
    let (s, _) = offline("catalog");
    let list = outcome(s.list_nodes().await).json();
    let ids: Vec<&str> = list["nodes"].as_array().unwrap().iter().map(|n| n["type_id"].as_str().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted);
    assert!(ids.contains(&"manual_trigger") && ids.contains(&"http_request"));

    let one = outcome(s.get_node_schema(Parameters(NodeSchemaParams { type_id: "http_request".into() })).await).json();
    assert!(one["input_schema"]["properties"].is_object());
    assert!(one.get("icon").is_none());
}

#[tokio::test]
async fn validate_runs_locally_and_says_plugin_nodes_are_unknown() {
    let (s, _) = offline("validate");
    let ok = outcome(s.validate_workflow(Parameters(ValidateParams { workflow: valid_workflow("w") })).await).json();
    assert_eq!(ok["valid"], true, "{ok}");
    let bad = outcome(s.validate_workflow(Parameters(ValidateParams { workflow: invalid_workflow("w") })).await).json();
    assert_eq!(bad["valid"], false);
    assert!(bad["note"].as_str().unwrap().contains("built-in"));
}

#[tokio::test]
async fn save_writes_a_valid_file_once_and_refuses_invalid_or_existing_ones() {
    let (s, dir) = offline("save");

    let written = outcome(s.save_workflow(save(valid_workflow("my flow"), false, false)).await);
    assert!(!written.is_error, "{}", written.text);
    let path = PathBuf::from(written.json()["path"].as_str().unwrap());
    assert_eq!(path, dir.join("my_flow.aerini"));
    let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(on_disk["id"], "my flow");

    let again = outcome(s.save_workflow(save(valid_workflow("my flow"), false, false)).await);
    assert!(again.is_error && again.text.contains("overwrite=true"));
    assert!(!outcome(s.save_workflow(save(valid_workflow("my flow"), false, true)).await).is_error);

    let refused = outcome(s.save_workflow(save(invalid_workflow("bad"), false, false)).await);
    assert!(refused.is_error && refused.text.contains("allow_invalid"));
    assert!(!dir.join("bad.aerini").exists());
    assert!(!outcome(s.save_workflow(save(invalid_workflow("bad"), true, false)).await).is_error);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn server_only_tools_explain_that_they_need_online_mode() {
    let (s, _) = offline("online_only");
    let got = outcome(s.get_workflow(Parameters(IdParams { id: "x".into() })).await);
    assert!(got.is_error && got.text.contains("AERINI_URL"));
}
